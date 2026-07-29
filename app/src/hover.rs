//! Hover price-check popup state: hotkey fires, clipboard text (already
//! injected and read by `inject::Injector`) is parsed and priced, and a
//! popup model is produced for the renderer. Pure logic, no I/O.

use std::time::{Duration, Instant};

use poe2_lens_core::{item, ninja::PriceTable, trade, value};

use crate::pricing::Denom;

#[derive(Debug, Clone, PartialEq)]
pub struct PopupLine {
    pub text: String,
    pub denom: Denom,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Popup {
    pub title: String,
    pub lines: Vec<PopupLine>,
    pub expires: Instant,
}

#[derive(Default)]
pub struct HoverState {
    pub current: Option<Popup>,
    pub last_error: Option<String>,
    /// A rare item parsed by `trigger`, waiting for the trade worker to
    /// appraise it (the popup shows "searching..." meanwhile).
    pub pending_appraisal: Option<item::Item>,
    /// A stackable currency (e.g. an omen) not in the local price table,
    /// waiting for a trade-exchange price lookup by this display name.
    pub pending_currency: Option<String>,
}

const POPUP_TTL: Duration = Duration::from_secs(6);
/// Appraisals hold longer: reading ten listings takes a while.
const APPRAISAL_TTL: Duration = Duration::from_secs(20);

impl HoverState {
    /// Parses and prices `clipboard`, replacing any existing popup (a
    /// trigger always starts a fresh 6s countdown, even if the previous
    /// popup had not yet expired).
    /// Brief status note at the cursor (e.g. "overlay off"): every
    /// hotkey press gets visible feedback, because a silent toggle reads
    /// as a dead key (live finding, 2026-07-23 test session).
    pub fn show_note(&mut self, text: &str) {
        self.current = Some(Popup {
            title: text.into(),
            lines: Vec::new(),
            expires: Instant::now() + Duration::from_millis(1500),
        });
    }

    /// Shows a brief "no item under cursor" popup so an F7 over empty
    /// space gives feedback rather than silence.
    pub fn show_no_item(&mut self) {
        self.last_error = Some("no item under cursor".into());
        self.current = Some(Popup {
            title: "no item".into(),
            lines: vec![PopupLine { text: "hover an item, then F7".into(), denom: Denom::None }],
            expires: Instant::now() + Duration::from_secs(3),
        });
    }

    pub fn trigger(
        &mut self,
        clipboard: &str,
        table: &PriceTable,
        uniques: &std::collections::HashMap<String, f64>,
        divine_threshold: f64,
    ) {
        self.last_error = None;
        let parsed = match item::parse_item(clipboard) {
            Ok(i) => i,
            Err(_) => {
                self.current = None;
                self.last_error = Some("no item under cursor".into());
                return;
            }
        };
        let title = if parsed.name.is_empty() {
            parsed.base_type.clone().unwrap_or_default()
        } else {
            parsed.name.clone()
        };
        let count = parsed.stack_size.map(|(n, _)| n).unwrap_or(1);
        let lines = match parsed.rarity {
            item::Rarity::Rare => {
                self.pending_appraisal = Some(parsed.clone());
                vec![PopupLine {
                    text: "searching trade...".into(),
                    denom: Denom::None,
                }]
            }
            // Uniques answer from the poe2scout map (exalted, by name);
            // an unknown name (map empty, unlisted unique) stays "?".
            // The divine value derives through the currency table's
            // Divine Orb rate so denom_amount can promote expensive
            // uniques to a divine display.
            item::Rarity::Unique => match uniques.get(&parsed.name).copied() {
                Some(ex) => {
                    let div_rate = table
                        .lookup("Divine Orb")
                        .map(|p| p.exalted)
                        .filter(|v| *v > 0.0);
                    let price = poe2_lens_core::ninja::Price {
                        divine: div_rate.map(|r| ex / r).unwrap_or(0.0),
                        exalted: ex,
                        chaos: 0.0,
                    };
                    let (denom, amount) =
                        crate::pricing::denom_amount(&price, count, divine_threshold);
                    vec![PopupLine { text: amount, denom }]
                }
                None => vec![PopupLine {
                    text: value::UNKNOWN.into(),
                    denom: Denom::None,
                }],
            },
            _ => match table.lookup(&title) {
                Some(price) => {
                    let (denom, amount) = crate::pricing::denom_amount(price, count, divine_threshold);
                    vec![PopupLine { text: amount, denom }]
                }
                // Stackable currency the local table doesn't carry (omens and
                // other exchange items poe.ninja doesn't track): price it via
                // the trade exchange instead of showing "?".
                None if parsed.stack_size.is_some() => {
                    self.pending_currency = Some(title.clone());
                    vec![PopupLine { text: "checking exchange...".into(), denom: Denom::None }]
                }
                None => vec![PopupLine {
                    text: value::UNKNOWN.into(),
                    denom: Denom::None,
                }],
            },
        };
        self.current = Some(Popup {
            title,
            lines,
            expires: Instant::now() + POPUP_TTL,
        });
    }

    /// Replaces the "searching..." popup with real listings (or the
    /// error) once the trade worker reports back.
    pub fn appraisal_done(&mut self, title: &str, outcome: Result<Vec<trade::Listing>, String>) {
        let lines = match outcome {
            Ok(listings) if listings.is_empty() => vec![PopupLine {
                text: "no online matches".into(),
                denom: Denom::None,
            }],
            Ok(listings) => listings
                .iter()
                .take(8)
                .map(|l| PopupLine {
                    text: format!("{} {} ({})", l.price_amount, l.price_currency, l.account),
                    denom: Denom::None,
                })
                .collect(),
            Err(e) => vec![PopupLine { text: e, denom: Denom::None }],
        };
        self.current = Some(Popup {
            title: title.to_string(),
            lines,
            expires: Instant::now() + APPRAISAL_TTL,
        });
    }

    /// Shows a currency's trade-exchange price (in exalted), or a not-found
    /// note, replacing the "checking exchange..." popup.
    pub fn show_exchange(&mut self, title: &str, exalted: Option<f64>) {
        let line = match exalted {
            Some(ex) if ex >= 1.0 => PopupLine {
                text: format!("{} exalted", (ex * 10.0).round() / 10.0),
                denom: Denom::None,
            },
            Some(ex) => PopupLine {
                text: format!("{:.2} exalted", ex),
                denom: Denom::None,
            },
            None => PopupLine { text: "no exchange price".into(), denom: Denom::None },
        };
        self.current = Some(Popup {
            title: title.to_string(),
            lines: vec![line],
            expires: Instant::now() + POPUP_TTL,
        });
    }

    pub fn tick(&mut self) {
        if let Some(p) = &self.current {
            if Instant::now() >= p.expires {
                self.current = None;
            }
        }
    }
}

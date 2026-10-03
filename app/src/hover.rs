//! Hover price-check popup state: hotkey fires, clipboard text (already
//! injected and read by `inject::Injector`) is parsed and priced, and a
//! popup model is produced for the renderer. Pure logic, no I/O.

use std::time::{Duration, Instant};

use khaloni_poe2_core::{item, ninja::PriceTable, value};

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
    /// waiting for a trade-exchange price lookup: its display name and the
    /// size of the hovered stack, which the answer is multiplied by.
    pub pending_currency: Option<(String, u32)>,
    /// The current popup is a notice (see [`HoverState::show_notice`]):
    /// it stays for its full time however the cursor moves.
    pub sticky: bool,
}

const POPUP_TTL: Duration = Duration::from_secs(6);

/// How old the data behind a local price is. A price from a table that
/// failed its last refresh is shown, and says so: hours-old exchange rates
/// look exactly like fresh ones.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Freshness {
    /// `Snapshot::stale`: the currency table.
    pub table_stale: bool,
    /// `Snapshot::uniques_stale`: the unique price lines.
    pub uniques_stale: bool,
}

const STALE_LINE: &str = "old price data: the last refresh failed";

fn plain(text: impl Into<String>) -> PopupLine {
    PopupLine { text: text.into(), denom: Denom::None }
}

/// Whether the copied text says the item cannot be changed any more. Both
/// clipboard formats (Ctrl+C and the advanced Ctrl+Alt+C) print the state
/// as a line of its own; "Twice Corrupted" and "Unmodifiable" trade as
/// corrupted too (the same reading `ee2::parse` makes).
pub fn is_corrupted(raw: &str) -> bool {
    raw.lines().any(|l| matches!(l.trim(), "Corrupted" | "Twice Corrupted" | "Unmodifiable"))
}

impl HoverState {
    /// Parses and prices `clipboard`, replacing any existing popup (a
    /// trigger always starts a fresh 6s countdown, even if the previous
    /// popup had not yet expired).
    /// Brief status note at the cursor (e.g. "already searching this
    /// item"): every hotkey press gets visible feedback, because a silent
    /// press reads as a dead key (live finding, 2026-07-23 test session).
    pub fn show_note(&mut self, text: &str) {
        self.sticky = false;
        self.current = Some(Popup {
            title: text.into(),
            lines: Vec::new(),
            expires: Instant::now() + Duration::from_millis(1500),
        });
    }

    /// A message the user has to get to read: something stopped working,
    /// or a setting was changed for them. Unlike a note it is not dismissed
    /// by moving the cursor away, and it stays long enough to be read in
    /// the middle of a fight.
    pub fn show_notice(&mut self, text: &str) {
        self.sticky = true;
        self.current = Some(Popup {
            title: text.into(),
            lines: Vec::new(),
            expires: Instant::now() + Duration::from_secs(6),
        });
    }

    /// Shows a brief "no item under cursor" popup so an F7 over empty
    /// space gives feedback rather than silence.
    pub fn show_no_item(&mut self) {
        self.sticky = false;
        self.last_error = Some("no item under cursor".into());
        self.current = Some(Popup {
            title: "no item".into(),
            lines: vec![PopupLine { text: "hover an item, then F7".into(), denom: Denom::None }],
            expires: Instant::now() + Duration::from_secs(3),
        });
    }

    /// [`trigger_priced`](Self::trigger_priced) over a name-only unique map
    /// (the poe2scout shape), which can only ever speak for uncorrupted
    /// items.
    pub fn trigger(
        &mut self,
        clipboard: &str,
        table: &PriceTable,
        uniques: &std::collections::HashMap<String, f64>,
        divine_threshold: f64,
    ) {
        let uniques = khaloni_poe2_core::ninja::UniquePrices::from_names(uniques.clone());
        self.trigger_priced(clipboard, table, &uniques, divine_threshold, Freshness::default());
    }

    pub fn trigger_priced(
        &mut self,
        clipboard: &str,
        table: &PriceTable,
        uniques: &khaloni_poe2_core::ninja::UniquePrices,
        divine_threshold: f64,
        fresh: Freshness,
    ) {
        self.last_error = None;
        self.sticky = false;
        // A new check owns the popup: a request left over from the last
        // one must not be sent on its behalf.
        self.pending_appraisal = None;
        self.pending_currency = None;
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
        // Sekhemas relics trade by their mods at every rarity (magic is the
        // common case), so they appraise like rares; the name-lookup path
        // below could only ever answer "?" for them (live finding).
        let relic_with_mods = parsed.item_class == "Relics" && !parsed.explicits.is_empty();
        let lines = match parsed.rarity {
            item::Rarity::Rare => {
                self.pending_appraisal = Some(parsed.clone());
                vec![PopupLine {
                    text: "searching trade...".into(),
                    denom: Denom::None,
                }]
            }
            // Uniques answer from the bulk price data only on an exact
            // (name, base type, corrupted) line: the same name on another
            // base or corrupted is a different market (3.0 div against
            // 0.235 div, live), so anything short of exact is searched.
            // The divine value derives through the currency table's
            // Divine Orb rate so denom_amount can promote expensive
            // uniques to a divine display.
            item::Rarity::Unique => match uniques.lookup(
                &parsed.name,
                parsed.base_type.as_deref(),
                is_corrupted(&parsed.raw),
            ) {
                khaloni_poe2_core::ninja::UniqueMatch::Exact(ex) => {
                    let price = table.price_from_exalted(ex);
                    let (denom, amount) =
                        crate::pricing::denom_amount(&price, count, divine_threshold);
                    // The number is poe.ninja's figure for every copy of
                    // this unique on this base, not a reading of this
                    // copy's rolls, and the popup says which it is.
                    let mut lines = vec![
                        PopupLine { text: amount, denom },
                        plain("poe.ninja average for this unique; its rolls are not compared"),
                    ];
                    if fresh.uniques_stale || fresh.table_stale {
                        lines.push(plain(STALE_LINE));
                    }
                    lines
                }
                // Not in the data (poe2scout publishes no unique prices for
                // some leagues, live 2026-09-08) or not as this variant: the
                // trade site prices it by name and base, exactly like a
                // rare, instead of "?".
                _ => {
                    self.pending_appraisal = Some(parsed.clone());
                    vec![PopupLine {
                        text: "searching trade...".into(),
                        denom: Denom::None,
                    }]
                }
            },
            _ if relic_with_mods => {
                self.pending_appraisal = Some(parsed.clone());
                vec![PopupLine {
                    text: "searching trade...".into(),
                    denom: Denom::None,
                }]
            }
            _ => match table.lookup(&title) {
                Some(price) => {
                    let (denom, amount) = crate::pricing::denom_amount(price, count, divine_threshold);
                    let mut lines = vec![PopupLine { text: amount, denom }];
                    if fresh.table_stale {
                        lines.push(plain(STALE_LINE));
                    }
                    lines
                }
                // Stackable currency the local table doesn't carry (omens and
                // other exchange items poe.ninja doesn't track): price it via
                // the trade exchange instead of showing "?".
                None if parsed.stack_size.is_some() => {
                    self.pending_currency = Some((title.clone(), count));
                    vec![PopupLine { text: "checking exchange...".into(), denom: Denom::None }]
                }
                // A cut gem (one skill at one level), or magic/normal gear
                // with mods (waystones above all): worth is in the specifics,
                // which only the trade site prices. Same path as a rare.
                None if parsed.rarity == item::Rarity::Gem
                    || !parsed.explicits.is_empty()
                    || parsed.item_class.eq_ignore_ascii_case("waystones") =>
                {
                    self.pending_appraisal = Some(parsed.clone());
                    vec![PopupLine { text: "searching trade...".into(), denom: Denom::None }]
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

    /// Shows a currency's trade-exchange answer in place of the "checking
    /// exchange..." popup: the stack's total with the per-unit price, as
    /// the local-table path shows it; a note when nobody offers it; or the
    /// reason the lookup failed (a cooldown is not "no price"). The rate is
    /// the median of the cheapest offers, the same figure the bulk view's
    /// rows are read from; `offers` says how many stood behind it when the
    /// answer came with them.
    #[allow(clippy::too_many_arguments)]
    pub fn show_exchange(
        &mut self,
        title: &str,
        outcome: &Result<Option<f64>, String>,
        offers: Option<usize>,
        count: u32,
        table: &khaloni_poe2_core::ninja::PriceTable,
        divine_threshold: f64,
        fresh: Freshness,
    ) {
        let lines = match outcome {
            Ok(Some(ex)) => {
                let (denom, text) =
                    crate::pricing::denom_amount(&table.price_from_exalted(*ex), count, divine_threshold);
                let source = match offers {
                    Some(n) => format!("trade exchange: {n} offers, median of the cheapest"),
                    None => "trade exchange: median of the cheapest offers".to_string(),
                };
                let mut lines = vec![PopupLine { text, denom }, plain(source)];
                // The offers are live; turning them into chaos or divine
                // goes through the table.
                if fresh.table_stale {
                    lines.push(plain(STALE_LINE));
                }
                lines
            }
            Ok(None) => vec![plain("no exchange offers right now")],
            Err(why) => vec![plain(format!("not priced: {why}"))],
        };
        self.sticky = false;
        self.current = Some(Popup {
            title: title.to_string(),
            lines,
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

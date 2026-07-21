//! Hover price-check popup state: hotkey fires, clipboard text (already
//! injected and read by `inject::Injector`) is parsed and priced, and a
//! popup model is produced for the renderer. Pure logic, no I/O.

use std::time::{Duration, Instant};

use poe2_lens_core::{item, ninja::PriceTable, value};

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
}

const POPUP_TTL: Duration = Duration::from_secs(6);

impl HoverState {
    /// Parses and prices `clipboard`, replacing any existing popup (a
    /// trigger always starts a fresh 6s countdown, even if the previous
    /// popup had not yet expired).
    pub fn trigger(&mut self, clipboard: &str, table: &PriceTable, divine_threshold: f64) {
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
            item::Rarity::Rare => vec![PopupLine {
                text: "rare appraisal in Stage B".into(),
                denom: Denom::None,
            }],
            _ => match table.lookup(&title) {
                Some(price) => {
                    let (denom, amount) = crate::pricing::denom_amount(price, count, divine_threshold);
                    vec![PopupLine { text: amount, denom }]
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

    pub fn tick(&mut self) {
        if let Some(p) = &self.current {
            if Instant::now() >= p.expires {
                self.current = None;
            }
        }
    }
}

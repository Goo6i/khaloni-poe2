//! Hover price-check popup state: hotkey fires, clipboard text (already
//! injected and read by `inject::Injector`) is parsed and priced, and a
//! popup model is produced for the renderer. Pure logic, no I/O.

use std::time::{Duration, Instant};

use poe2_lens_core::{item, ninja::Price, ninja::PriceTable, value};

use crate::pricing::Denom;

pub struct PopupLine {
    pub text: String,
    pub denom: Denom,
}

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
                    let (amount, denom) = amount_and_denom(price, count, divine_threshold);
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

/// Same divine-vs-exalted choice and formatting as `pricing::denom_amount`
/// (that function is private to its module, so this mirrors it against the
/// public `Price` fields and `value::format_amount` rather than reaching
/// into `pricing` directly); kept in sync by the shared behavior the two
/// are tested against, not by a shared implementation.
fn amount_and_denom(price: &Price, count: u32, divine_threshold: f64) -> (String, Denom) {
    let count = count.max(1);
    let total_divine = price.divine * f64::from(count);
    let total_exalted = price.exalted * f64::from(count);
    if total_divine >= divine_threshold {
        let amount = if count == 1 {
            value::format_amount(total_divine)
        } else {
            format!(
                "{} ({} each)",
                value::format_amount(total_divine),
                value::format_amount(price.divine)
            )
        };
        (amount, Denom::Divine)
    } else {
        let amount = if count == 1 {
            value::format_amount(total_exalted)
        } else {
            format!(
                "{} ({} each)",
                value::format_amount(total_exalted),
                value::format_amount(price.exalted)
            )
        };
        (amount, Denom::Exalted)
    }
}

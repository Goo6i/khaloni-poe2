//! The currency-exchange response as the bulk view shows it: one row per
//! offer with what the seller gives, what they want for it, their stock,
//! and the price per unit, cheapest first (the columns of EE2's
//! TradeBulk.vue). The going rate the popup quotes is the same median of
//! the five cheapest that [`crate::trade::exchange_rate_of`] has always
//! drawn, kept here so the rows and the rate never disagree.

use crate::listing::SellerState;
use serde_json::Value;

/// How many of the cheapest offers the quoted rate is drawn from; the same
/// sample `trade::exchange_rate_of` takes, and a test holds them equal.
const CHEAPEST: usize = 5;

/// One exchange offer as a row: `have` is what the seller gives (the
/// `item` side, the currency being bought), `want` what they ask for it
/// (the `exchange` side), each as (currency id, amount).
#[derive(Debug, Clone, PartialEq)]
pub struct Offer {
    pub seller: String,
    pub state: SellerState,
    pub have: (String, f64),
    pub want: (String, f64),
    /// How many the seller has listed; the API leaves it out sometimes.
    pub stock: Option<u32>,
    /// `want.1 / have.1`: what one unit of `have` costs.
    pub per_unit: f64,
}

/// Every priced offer in a body, cheapest per unit first.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct BulkView {
    pub offers: Vec<Offer>,
    /// Offers left out because an amount was missing, not a number or not
    /// positive: nothing is sold for nothing, and nothing divides by zero.
    pub skipped: usize,
}

/// Reads an exchange body (`{"result": {"<id>": {"listing": {"account",
/// "offers": [...]}}}}`) into rows. A listing without offers contributes
/// nothing; an offer with a zero or missing amount on either side is
/// counted in `skipped` and left out.
pub fn parse_exchange(v: &Value) -> BulkView {
    let mut view = BulkView::default();
    let Some(result) = v.get("result").and_then(Value::as_object) else {
        return view;
    };
    for entry in result.values() {
        let Some(offers) = entry.pointer("/listing/offers").and_then(Value::as_array) else {
            continue;
        };
        let account = entry.pointer("/listing/account").unwrap_or(&Value::Null);
        let seller = account["name"].as_str().unwrap_or("").to_string();
        let state = match account["online"].as_object() {
            None => SellerState::Offline,
            Some(online) if online.get("status").and_then(Value::as_str) == Some("afk") => SellerState::Afk,
            Some(_) => SellerState::Online,
        };
        for offer in offers {
            let have_amount = offer.pointer("/item/amount").and_then(Value::as_f64);
            let want_amount = offer.pointer("/exchange/amount").and_then(Value::as_f64);
            let (Some(have_amount), Some(want_amount)) = (have_amount, want_amount) else {
                view.skipped += 1;
                continue;
            };
            if have_amount <= 0.0 || want_amount <= 0.0 {
                view.skipped += 1;
                continue;
            }
            let currency = |side: &str| {
                offer.pointer(&format!("/{side}/currency")).and_then(Value::as_str).unwrap_or("").to_string()
            };
            view.offers.push(Offer {
                seller: seller.clone(),
                state,
                have: (currency("item"), have_amount),
                want: (currency("exchange"), want_amount),
                stock: offer.pointer("/item/stock").and_then(Value::as_u64).map(|s| s as u32),
                per_unit: want_amount / have_amount,
            });
        }
    }
    view.offers.sort_by(|a, b| a.per_unit.total_cmp(&b.per_unit));
    view
}

impl BulkView {
    /// The going rate: the median of the [`CHEAPEST`] offers' per-unit
    /// prices, `None` with no offers. Equal to `trade::exchange_rate_of`
    /// over the same body.
    pub fn median_rate(&self) -> Option<f64> {
        let rates: Vec<f64> = self.offers.iter().take(CHEAPEST).map(|o| o.per_unit).collect();
        match rates.len() {
            0 => None,
            n if n % 2 == 1 => Some(rates[n / 2]),
            n => Some((rates[n / 2 - 1] + rates[n / 2]) / 2.0),
        }
    }

    /// One line per offer, e.g. "12 in stock · 3.5 exalted each · sellerA
    /// (online)"; a bulk offer also names its lot, "7 exalted for 2 omen",
    /// since that is what the seller will fill. `unit_have` names what is
    /// sold, `unit_want` what it costs.
    pub fn text_rows(&self, unit_have: &str, unit_want: &str) -> Vec<String> {
        self.offers
            .iter()
            .map(|o| {
                let stock = match o.stock {
                    Some(n) => format!("{n} in stock"),
                    None => "stock unknown".to_string(),
                };
                let mut parts = vec![stock, format!("{} {unit_want} each", number(o.per_unit))];
                if o.have.1 != 1.0 {
                    parts.push(format!("{} {unit_want} for {} {unit_have}", number(o.want.1), number(o.have.1)));
                }
                parts.push(format!("{} ({})", o.seller, o.state.as_str()));
                parts.join(" · ")
            })
            .collect()
    }
}

/// A price to four decimals with the trailing zeros dropped, as EE2's bulk
/// table prints it: 3.5, 3.3333, 2.
fn number(x: f64) -> String {
    let s = format!("{x:.4}");
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

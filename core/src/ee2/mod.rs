//! The price-check search exactly as Exiled Exchange 2 builds it.
//!
//! A port of EE2's parser, default filter selection and request builder,
//! driven by the same `stats.ndjson` / `items.ndjson` it ships. The owner
//! trades off these searches, so the bar is the identical request for the
//! identical clipboard text; `tools/ee2-parity` and `tests/ee2_parity.rs`
//! hold it to that.

pub mod data;
pub mod filters;
pub mod parse;
pub mod request;

pub use data::Ee2Data;
pub use request::{build, Built};

//! tinytrader's extensions to Databento's DBN format.
//!
//! The official `dbn` crate defines Databento's records; this crate adds the
//! records tinytrader needs that DBN doesn't define yet, and re-exports `dbn` so
//! a consumer uses one dependency for both. Each extension is meant to be
//! retired once Databento ships an equivalent (for trade conditions, see
//! Databento's roadmap items for OPRA and US equity trade conditions).
//!
//! Extension records use DBN framing (a length byte in 4-byte words and an
//! rtype) with rtypes DBN doesn't allocate, so a stock DBN reader can skip
//! them by length.

pub use dbn;

pub mod merge;
pub mod trade_conditions;

pub use merge::{Class, DbnSource, Delivery, Key, Merge, MergeError, Timing, index_ts, timing};
pub use trade_conditions::{TradeConditions, TradeUpdate};

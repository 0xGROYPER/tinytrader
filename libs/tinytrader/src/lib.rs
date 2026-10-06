//! tinytrader: simple tools for trading on financial markets, built on
//! Databento's DBN format.
//!
//! This crate is the entry point; it re-exports tinytrader's libraries as
//! they're published. Today that's [`dbn`], tinytrader's extensions to DBN
//! (which in turn re-exports the official `dbn` crate).

pub use tinytrader_dbn as dbn;
pub use tinytrader_dbn::{TradeConditions, TradeUpdate};

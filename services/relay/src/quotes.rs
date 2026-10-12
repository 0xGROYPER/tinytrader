//! The latest quote of each instrument the relay carries (OPRA's consolidated
//! best bid and offer from `cmbp-1`, or `mbp-1`'s top of book), kept in
//! memory and served at `/quotes` so a chain can be priced from the real
//! market instead of an indicative feed. Like the bars, it's a view derived
//! from the records the relay forwards: nothing is recorded (decision 0006).
//!
//! `GET /quotes?symbols=SPY261016C00775000,...` answers for those contracts;
//! `?underlying=SPY` for every contract of that root, optionally only those
//! expiring `from` / `to` (YYYY-MM-DD). The answer is
//! `{"quotes": {OCC: {bid, ask, bid_size, ask_size, ts}}, "as_of", "last_record"}`: symbols
//! in compact OCC form (the provider's padding removed), prices in dollars
//! (`null` when a side is empty), `ts` the provider's receive time in Unix
//! seconds. A quote changes only when its book does, so a quiet contract's
//! can be old and still current: `last_record` (Unix seconds, null before
//! any) says whether the feed itself is live.

use std::collections::{HashMap, HashSet};
use std::mem::{offset_of, size_of};
use std::sync::{Arc, Mutex, PoisonError};

use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::routing::get;
use axum::{Json, Router};
use databento::dbn::{
    BidAskPair, Cmbp1Msg, ConsolidatedBidAskPair, Mbp1Msg, RecordHeader, UNDEF_PRICE, rtype,
};
use serde::Deserialize;
use serde_json::{Map, Value, json};

use crate::hub::Hub;

/// One instrument's top of book.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Quote {
    pub bid: Option<f64>,
    pub ask: Option<f64>,
    pub bid_size: u32,
    pub ask_size: u32,
    /// The provider's receive time, ns since the epoch.
    pub ts: u64,
}

/// Latest quotes by instrument id.
#[derive(Default)]
pub struct Quotes {
    by_id: Mutex<HashMap<u32, Quote>>,
}

fn at<const N: usize>(rec: &[u8], at: usize) -> Option<[u8; N]> {
    rec.get(at..at + N)?.try_into().ok()
}

fn px(raw: i64) -> Option<f64> {
    (raw != UNDEF_PRICE && raw > 0).then(|| raw as f64 / 1e9)
}

impl Quotes {
    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<u32, Quote>> {
        self.by_id.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Take the top of book from every `cmbp-1` or `mbp-1` record in a chunk
    /// of DBN records: each carries the book after its event, trades included.
    pub fn absorb(&self, chunk: &[u8]) {
        let mut by_id = self.lock();
        let mut off = 0;
        while off < chunk.len() {
            let len = usize::from(chunk[off]) * 4;
            let Some(rec) = chunk.get(off..off + len).filter(|r| r.len() >= 16) else {
                return;
            };
            off += len;
            let (levels, bid, ask, bsz, asz, recv) = match rec[1] {
                rtype::CMBP_1 if len >= size_of::<Cmbp1Msg>() => (
                    offset_of!(Cmbp1Msg, levels),
                    offset_of!(ConsolidatedBidAskPair, bid_px),
                    offset_of!(ConsolidatedBidAskPair, ask_px),
                    offset_of!(ConsolidatedBidAskPair, bid_sz),
                    offset_of!(ConsolidatedBidAskPair, ask_sz),
                    offset_of!(Cmbp1Msg, ts_recv),
                ),
                rtype::MBP_1 if len >= size_of::<Mbp1Msg>() => (
                    offset_of!(Mbp1Msg, levels),
                    offset_of!(BidAskPair, bid_px),
                    offset_of!(BidAskPair, ask_px),
                    offset_of!(BidAskPair, bid_sz),
                    offset_of!(BidAskPair, ask_sz),
                    offset_of!(Mbp1Msg, ts_recv),
                ),
                _ => continue,
            };
            let (Some(id), Some(b), Some(a), Some(bs), Some(as_), Some(ts)) = (
                at::<4>(rec, offset_of!(RecordHeader, instrument_id)).map(u32::from_le_bytes),
                at::<8>(rec, levels + bid).map(i64::from_le_bytes),
                at::<8>(rec, levels + ask).map(i64::from_le_bytes),
                at::<4>(rec, levels + bsz).map(u32::from_le_bytes),
                at::<4>(rec, levels + asz).map(u32::from_le_bytes),
                at::<8>(rec, recv).map(u64::from_le_bytes),
            ) else {
                continue;
            };
            by_id.insert(
                id,
                Quote {
                    bid: px(b),
                    ask: px(a),
                    bid_size: bs,
                    ask_size: as_,
                    ts,
                },
            );
        }
    }

    /// The quotes whose symbols pass `keep`, by compact symbol, forgetting
    /// instruments that no longer have a definition (expired).
    pub fn matching(
        &self,
        symbol_of: impl Fn(u32) -> Option<String>,
        keep: impl Fn(&str) -> bool,
    ) -> Vec<(String, Quote)> {
        let mut by_id = self.lock();
        let mut out = Vec::new();
        by_id.retain(|id, q| {
            let Some(raw) = symbol_of(*id) else {
                return false;
            };
            let s = compact(&raw);
            if keep(&s) {
                out.push((s, *q));
            }
            true
        });
        out
    }

    pub fn len(&self) -> usize {
        self.lock().len()
    }
}

/// OPRA's padded `SPY   261016C00775000` as `SPY261016C00775000`.
#[must_use]
pub fn compact(raw: &str) -> String {
    raw.chars().filter(|c| !c.is_whitespace()).collect()
}

/// An OCC symbol's root and expiry (`YYMMDD`), if it is one.
fn occ(s: &str) -> Option<(&str, &str)> {
    let n = s.len();
    (n >= 16 && s.is_ascii()).then(|| (&s[..n - 15], &s[n - 15..n - 9]))
}

#[derive(Deserialize)]
struct Ask {
    symbols: Option<String>,
    underlying: Option<String>,
    from: Option<String>,
    to: Option<String>,
}

/// `YYYY-MM-DD` as `YYMMDD`.
fn yymmdd(d: &str) -> Option<String> {
    (d.len() == 10).then(|| format!("{}{}{}", &d[2..4], &d[5..7], &d[8..10]))
}

async fn serve(State(hub): State<Arc<Hub>>, Query(ask): Query<Ask>) -> (StatusCode, Json<Value>) {
    let wanted: Option<HashSet<String>> = ask.symbols.as_deref().map(|s| {
        s.split(',')
            .map(|x| compact(x).to_uppercase())
            .filter(|x| !x.is_empty())
            .collect()
    });
    let root = ask.underlying.as_deref().map(str::to_uppercase);
    if wanted.is_none() && root.is_none() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "symbols=OCC,... or underlying=SPY" })),
        );
    }
    let from = ask.from.as_deref().and_then(yymmdd);
    let to = ask.to.as_deref().and_then(yymmdd);
    let keep = |s: &str| {
        if let Some(w) = &wanted {
            return w.contains(s);
        }
        let Some((r, exp)) = occ(s) else {
            return false;
        };
        root.as_deref() == Some(r)
            && from.as_deref().is_none_or(|f| exp >= f)
            && to.as_deref().is_none_or(|t| exp <= t)
    };
    let found = hub.quotes_matching(keep);
    let mut quotes = Map::new();
    for (s, q) in found {
        quotes.insert(
            s,
            json!({ "bid": q.bid, "ask": q.ask, "bid_size": q.bid_size, "ask_size": q.ask_size, "ts": q.ts as f64 / 1e9 }),
        );
    }
    (
        StatusCode::OK,
        Json(json!({
            "quotes": quotes,
            "as_of": crate::protocol::now_ns() as f64 / 1e9,
            "last_record": match hub.last_record_ns.load(std::sync::atomic::Ordering::Relaxed) {
                0 => None,
                ns => Some(ns as f64 / 1e9),
            },
        })),
    )
}

pub fn router(hub: Arc<Hub>) -> Router {
    Router::new().route("/quotes", get(serve)).with_state(hub)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compacts_and_reads_occ() {
        assert_eq!(compact("SPY   261016C00775000"), "SPY261016C00775000");
        assert_eq!(occ("SPY261016C00775000"), Some(("SPY", "261016")));
        assert_eq!(occ("SPXW261016P05742500"), Some(("SPXW", "261016")));
        assert_eq!(occ("SPY"), None);
        assert_eq!(yymmdd("2026-10-16").as_deref(), Some("261016"));
    }
}

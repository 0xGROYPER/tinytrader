//! The tape: the latest trades and top-of-book changes of each of the
//! relay's own stock symbols (IEX's `mbp-1`), kept in memory and served at
//! `/tape` for the console's time and sales. Like the quotes and bars, it's a
//! view derived from the records the relay forwards; nothing is recorded
//! (decision 0006), and it's empty after a restart until the feed runs.
//!
//! OPRA's `cmbp-1` isn't kept: a tape of every SPY option's quotes isn't one
//! anybody reads.
//!
//! `GET /tape?symbol=SPY&after=<seq>&limit=<n>` answers
//! `{"symbol", "ticks": [...], "last", "symbols", "last_record"}`, oldest
//! first: the ticks after `after` (or the latest `limit`, at most 1000,
//! without it). Each tick is `{"s": seq, "t": ts_event (Unix seconds), "k":
//! "T" or "Q", "b", "a", "bz", "az"}`, with `"p"` and `"z"` (price and size)
//! on a trade. A trade's book is the one before it (as the IEX encoder
//! writes it), so a trade at the ask was a buy. `last` is the newest
//! sequence number held for any symbol, for the next `after`.
//!
//! With `&watch=1`, a symbol beyond the relay's own is watched for the tape:
//! a relay whose provider can add symbols to its open stream (Alpaca IEX)
//! subscribes it there, without reconnecting, and its ticks go to the tape
//! only, never to the relay's clients or anything recorded. A watched symbol
//! no one has read for ten minutes is dropped; at most twenty are watched.
//! The answer's `watching` lists them, and `watch` says why one couldn't be.
//!
//! A symbol asked for with nothing held (after a restart, overnight, or
//! just watched) is filled from history: the source's latest trades and
//! quotes, at most once per symbol every five minutes, and only into an empty
//! tape so history never lands among live ticks. Those ticks carry `"h": true`.

use std::collections::{HashMap, VecDeque};
use std::mem::offset_of;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use axum::extract::{Query, State};
use axum::routing::get;
use axum::{Json, Router};
use databento::dbn::{BidAskPair, Mbp1Msg, RecordHeader, UNDEF_PRICE, rtype};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::hub::Hub;

/// Ticks kept per symbol: trades and quote changes, each.
const KEEP: usize = 4000;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Tick {
    pub seq: u64,
    pub ts: u64,
    /// Price and size: a trade's; None for a quote change.
    pub trade: Option<(f64, u32)>,
    pub bid: Option<f64>,
    pub ask: Option<f64>,
    pub bid_size: u32,
    pub ask_size: u32,
    /// From history, not the live feed.
    pub hist: bool,
}

#[derive(Default)]
struct Book {
    trades: VecDeque<Tick>,
    quotes: VecDeque<Tick>,
    /// The last top of book seen, so only changes are kept.
    top: Option<(Option<f64>, Option<f64>, u32, u32)>,
}

#[derive(Default)]
pub struct Tape {
    inner: Mutex<(u64, HashMap<String, Book>)>,
    pub watch: Watch,
}

/// Symbols watched for the tape beyond the relay's own: when each was last
/// asked for.
#[derive(Default)]
pub struct Watch {
    enabled: AtomicBool,
    own: Mutex<Vec<String>>,
    asked: Mutex<HashMap<String, Instant>>,
    /// Woken when a symbol is added, for the source to subscribe it.
    pub changed: tokio::sync::Notify,
    /// Symbols to fill from history, and when each was last asked for.
    history: Mutex<(HashMap<String, Instant>, Vec<String>)>,
    /// Woken when a symbol needs history.
    pub history_wanted: tokio::sync::Notify,
}

/// History for a symbol's tape is fetched at most this often.
const HISTORY_EVERY: Duration = Duration::from_secs(300);

fn ticker(symbol: &str) -> bool {
    symbol.len() <= 10
        && symbol.starts_with(|c: char| c.is_ascii_uppercase())
        && symbol
            .chars()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '.')
}

/// At most this many watched symbols (Alpaca's free stream takes 30 in all).
const WATCH_MAX: usize = 20;
/// A watched symbol unread this long is dropped.
const WATCH_IDLE: Duration = Duration::from_secs(600);

impl Watch {
    /// The source can add symbols to its stream; `own` are the relay's own.
    pub fn enable(&self, own: &[String]) {
        *self.own.lock().unwrap_or_else(PoisonError::into_inner) = own.to_vec();
        self.enabled.store(true, Ordering::Relaxed);
    }

    /// Watch `symbol` (or keep watching it). `Err` says why not.
    pub fn ask(&self, symbol: &str) -> Result<(), &'static str> {
        if !self.enabled.load(Ordering::Relaxed) {
            return Err("this relay can't add symbols");
        }
        if !ticker(symbol) {
            return Err("not a ticker");
        }
        if self
            .own
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .any(|s| s == symbol)
        {
            return Ok(());
        }
        let mut asked = self.asked.lock().unwrap_or_else(PoisonError::into_inner);
        asked.retain(|_, t| t.elapsed() < WATCH_IDLE);
        let new = !asked.contains_key(symbol);
        if new && asked.len() >= WATCH_MAX {
            return Err("already watching the most symbols it can");
        }
        asked.insert(symbol.to_owned(), Instant::now());
        drop(asked);
        if new {
            self.changed.notify_one();
        }
        Ok(())
    }

    /// Ask for `symbol`'s tape to be filled from history (where the source
    /// can, and not within five minutes of the last time).
    pub fn want_history(&self, symbol: &str) {
        if !self.enabled.load(Ordering::Relaxed) || !ticker(symbol) {
            return;
        }
        let mut h = self.history.lock().unwrap_or_else(PoisonError::into_inner);
        if h.0.get(symbol).is_some_and(|t| t.elapsed() < HISTORY_EVERY) {
            return;
        }
        h.0.insert(symbol.to_owned(), Instant::now());
        h.1.push(symbol.to_owned());
        drop(h);
        self.history_wanted.notify_one();
    }

    /// The symbols waiting for history.
    pub fn take_history(&self) -> Vec<String> {
        std::mem::take(
            &mut self
                .history
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .1,
        )
    }

    /// The symbols watched now (dropping those unread too long), sorted.
    pub fn wanted(&self) -> Vec<String> {
        let mut asked = self.asked.lock().unwrap_or_else(PoisonError::into_inner);
        asked.retain(|_, t| t.elapsed() < WATCH_IDLE);
        let mut v: Vec<String> = asked.keys().cloned().collect();
        v.sort();
        v
    }
}

fn at<const N: usize>(rec: &[u8], at: usize) -> Option<[u8; N]> {
    rec.get(at..at + N)?.try_into().ok()
}
fn px(raw: i64) -> Option<f64> {
    (raw != UNDEF_PRICE && raw > 0).then(|| raw as f64 / 1e9)
}

impl Tape {
    fn lock(&self) -> std::sync::MutexGuard<'_, (u64, HashMap<String, Book>)> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Take every `mbp-1` trade, and every change to the top of book, from
    /// a chunk of DBN records; `symbol_of` names an instrument.
    pub fn absorb(&self, chunk: &[u8], symbol_of: impl Fn(u32) -> Option<String>) {
        let mut guard = self.lock();
        let (seq, books) = &mut *guard;
        take(seq, books, chunk, &symbol_of, false);
    }

    /// Fill `symbol`'s tape from history (encoded `mbp-1`, oldest first),
    /// only if it holds nothing yet. Whether it was taken.
    pub fn seed(&self, chunk: &[u8], symbol: &str) -> bool {
        let mut guard = self.lock();
        let (seq, books) = &mut *guard;
        if books
            .get(symbol)
            .is_some_and(|b| !b.trades.is_empty() || !b.quotes.is_empty())
        {
            return false;
        }
        take(seq, books, chunk, &|_| Some(symbol.to_owned()), true);
        true
    }

    /// Take a watched symbol's records (encoded `mbp-1`, not forwarded).
    pub fn absorb_watched(&self, chunk: &[u8], symbol: &str) {
        self.absorb(chunk, |_| Some(symbol.to_owned()));
    }

    /// A symbol's ticks after `after` (or its latest `limit`), oldest first,
    /// the newest sequence number held, and the symbols held.
    pub fn read(
        &self,
        symbol: &str,
        after: Option<u64>,
        limit: usize,
    ) -> (Vec<Tick>, u64, Vec<String>) {
        let guard = self.lock();
        let (seq, books) = &*guard;
        let mut symbols: Vec<String> = books.keys().cloned().collect();
        symbols.sort();
        let Some(book) = books.get(symbol) else {
            return (Vec::new(), *seq, symbols);
        };
        let mut out: Vec<Tick> = book
            .trades
            .iter()
            .chain(book.quotes.iter())
            .filter(|t| after.is_none_or(|a| t.seq > a))
            .copied()
            .collect();
        out.sort_by_key(|t| t.seq);
        let skip = out.len().saturating_sub(limit);
        (out.split_off(skip), *seq, symbols)
    }
}

/// Take a chunk's `mbp-1` trades and top-of-book changes into the books.
fn take(
    seq: &mut u64,
    books: &mut HashMap<String, Book>,
    chunk: &[u8],
    symbol_of: &dyn Fn(u32) -> Option<String>,
    hist: bool,
) {
    let lv = offset_of!(Mbp1Msg, levels);
    let mut off = 0;
    while off < chunk.len() {
        let len = usize::from(chunk[off]) * 4;
        let Some(rec) = chunk.get(off..off + len).filter(|r| r.len() >= 16) else {
            return;
        };
        off += len;
        if rec[1] != rtype::MBP_1 || len < std::mem::size_of::<Mbp1Msg>() {
            continue;
        }
        let num = |o: usize| at::<8>(rec, o).map(i64::from_le_bytes);
        let small = |o: usize| at::<4>(rec, o).map(u32::from_le_bytes);
        let (Some(id), Some(ts), Some(b), Some(a), Some(bz), Some(az)) = (
            small(offset_of!(RecordHeader, instrument_id)),
            at::<8>(rec, offset_of!(RecordHeader, ts_event)).map(u64::from_le_bytes),
            num(lv + offset_of!(BidAskPair, bid_px)),
            num(lv + offset_of!(BidAskPair, ask_px)),
            small(lv + offset_of!(BidAskPair, bid_sz)),
            small(lv + offset_of!(BidAskPair, ask_sz)),
        ) else {
            continue;
        };
        let Some(symbol) = symbol_of(id) else {
            continue;
        };
        let book = books.entry(symbol).or_default();
        let top = (px(b), px(a), bz, az);
        let trade = if rec.get(offset_of!(Mbp1Msg, action)) == Some(&b'T') {
            match (
                num(offset_of!(Mbp1Msg, price)).and_then(px),
                small(offset_of!(Mbp1Msg, size)),
            ) {
                (Some(p), Some(z)) => Some((p, z)),
                _ => continue,
            }
        } else {
            // A quote: kept only when the top of book changed.
            if book.top == Some(top) {
                continue;
            }
            book.top = Some(top);
            None
        };
        *seq += 1;
        let tick = Tick {
            seq: *seq,
            ts,
            trade,
            bid: top.0,
            ask: top.1,
            bid_size: bz,
            ask_size: az,
            hist,
        };
        let ring = if trade.is_some() {
            &mut book.trades
        } else {
            &mut book.quotes
        };
        if ring.len() == KEEP {
            ring.pop_front();
        }
        ring.push_back(tick);
    }
}

#[derive(Deserialize)]
struct Ask {
    symbol: Option<String>,
    after: Option<u64>,
    limit: Option<usize>,
    #[serde(default)]
    watch: Option<u8>,
}

fn tick_json(t: &Tick) -> Value {
    let mut v = json!({ "s": t.seq, "t": t.ts as f64 / 1e9, "k": if t.trade.is_some() { "T" } else { "Q" },
        "b": t.bid, "a": t.ask, "bz": t.bid_size, "az": t.ask_size });
    if t.hist {
        v["h"] = json!(true);
    }
    if let Some((p, z)) = t.trade {
        v["p"] = json!(p);
        v["z"] = json!(z);
    }
    v
}

async fn serve(State(hub): State<Arc<Hub>>, Query(ask): Query<Ask>) -> Json<Value> {
    let limit = ask.limit.unwrap_or(500).clamp(1, 1000);
    let symbol = ask.symbol.unwrap_or_default().trim().to_uppercase();
    let refused = if ask.watch == Some(1) {
        hub.tape.watch.ask(&symbol).err()
    } else {
        None
    };
    let (ticks, last, symbols) = hub.tape.read(&symbol, ask.after, limit);
    if ticks.is_empty() && ask.after.is_none() {
        hub.tape.watch.want_history(&symbol);
    }
    Json(json!({
        "symbol": symbol,
        "watching": hub.tape.watch.wanted(),
        "watch": refused,
        "ticks": ticks.iter().map(tick_json).collect::<Vec<_>>(),
        "last": last,
        "symbols": symbols,
        "last_record": match hub.last_record_ns.load(std::sync::atomic::Ordering::Relaxed) {
            0 => None,
            ns => Some(ns as f64 / 1e9),
        },
    }))
}

pub fn router(hub: Arc<Hub>) -> Router {
    Router::new().route("/tape", get(serve)).with_state(hub)
}

#[cfg(test)]
mod tests {
    use super::*;
    use databento::dbn::RecordRef;

    fn mbp(id: u32, ts: u64, action: u8, price: i64, size: u32, bid: i64, ask: i64) -> Vec<u8> {
        let m = Mbp1Msg {
            hd: RecordHeader::new::<Mbp1Msg>(rtype::MBP_1, 1, id, ts),
            action: action as std::ffi::c_char,
            price,
            size,
            levels: [BidAskPair {
                bid_px: bid,
                ask_px: ask,
                bid_sz: 3,
                ask_sz: 4,
                ..Default::default()
            }],
            ..Default::default()
        };
        RecordRef::from(&m).as_ref().to_vec()
    }

    #[test]
    fn history_fills_only_an_empty_tape_and_is_marked() {
        let tape = Tape::default();
        let old = mbp(
            7,
            1_000,
            b'T',
            500_000_000_000,
            10,
            499_000_000_000,
            501_000_000_000,
        );
        assert!(tape.seed(&old, "GLD"));
        let (ticks, _, _) = tape.read("GLD", None, 10);
        assert!(ticks.len() == 1 && ticks[0].hist);
        assert!(!tape.seed(&old, "GLD"));
        let w = Watch::default();
        w.want_history("GLD");
        assert!(w.take_history().is_empty());
        w.enable(&[]);
        w.want_history("GLD");
        w.want_history("GLD");
        assert_eq!(w.take_history(), vec!["GLD".to_owned()]);
    }

    #[test]
    fn watches_symbols_only_where_enabled_and_within_bounds() {
        let w = Watch::default();
        assert!(w.ask("GLD").is_err());
        w.enable(&["SPY".to_owned()]);
        assert_eq!(w.ask("GLD"), Ok(()));
        assert_eq!(w.ask("SPY"), Ok(()));
        assert!(w.ask("gld; rm").is_err());
        assert_eq!(w.wanted(), vec!["GLD".to_owned()]);
        for i in 0..WATCH_MAX - 1 {
            assert_eq!(w.ask(&format!("X{i}")), Ok(()));
        }
        assert!(w.ask("ONEMORE").is_err());
        assert_eq!(w.ask("GLD"), Ok(()));
    }

    #[test]
    fn keeps_trades_and_quote_changes_in_order() {
        let tape = Tape::default();
        let name = |id: u32| (id == 7).then(|| "SPY".to_owned());
        let d = 1_000_000_000i64;
        let mut chunk = mbp(7, 1, b'M', UNDEF_PRICE, 0, 777 * d, 778 * d);
        chunk.extend(mbp(7, 2, b'M', UNDEF_PRICE, 0, 777 * d, 778 * d)); // unchanged: dropped
        chunk.extend(mbp(7, 3, b'T', 778 * d, 100, 777 * d, 778 * d));
        chunk.extend(mbp(9, 4, b'T', 5 * d, 1, 4 * d, 6 * d)); // no symbol: dropped
        tape.absorb(&chunk, name);
        let (ticks, last, symbols) = tape.read("SPY", None, 10);
        assert_eq!(symbols, vec!["SPY".to_owned()]);
        assert_eq!(last, 2);
        assert_eq!(ticks.len(), 2);
        assert_eq!(
            (ticks[0].trade, ticks[0].bid, ticks[0].ask),
            (None, Some(777.0), Some(778.0))
        );
        assert_eq!(ticks[1].trade, Some((778.0, 100)));
        assert_eq!(tape.read("SPY", Some(1), 10).0.len(), 1);
        assert_eq!(tape.read("SPY", None, 1).0[0].seq, 2);
        assert!(tape.read("QQQ", None, 10).0.is_empty());
    }

    #[test]
    fn keeps_a_bounded_window() {
        let tape = Tape::default();
        let mut chunk = Vec::new();
        for i in 0..(KEEP as u64 + 10) {
            chunk.extend(mbp(7, i, b'T', 1_000_000_000, 1, 0, 0));
        }
        tape.absorb(&chunk, |_| Some("SPY".to_owned()));
        let (ticks, ..) = tape.read("SPY", Some(0), usize::MAX);
        assert_eq!(ticks.len(), KEEP);
        assert_eq!(ticks[0].seq, 11);
    }
}

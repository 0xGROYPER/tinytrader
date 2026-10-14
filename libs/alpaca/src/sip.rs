//! Alpaca SIP (consolidated) quotes and trades as DBN `CMBP-1`, with trade
//! conditions as `tinytrader.trade-conditions/1` records. For history
//! (`alpaca-history`, decision 0014); the conventions are IEX's ([`crate::iex`])
//! where they apply:
//!
//! - A quote is an `M` record with side `N`, `price` undefined and `size` 0.
//!   Its header's publisher is the consolidated feed ([`PUBLISHER`]); the
//!   book is in `levels[0]`, each side naming the exchange quoting it
//!   (`bid_pb`, `ask_pb`).
//! - A trade is a `T` record with side `N`, its own price and size; its
//!   header's publisher is the exchange that executed it, and it carries the
//!   consolidated book as it stood before the trade.
//! - Exchanges are Alpaca's one-letter codes, each a publisher id
//!   `40100 + (letter - 'A')`; an unknown code is refused, not guessed.
//! - Prices, timestamps, the round-lot rule for quote sizes, `F_LAST` and the
//!   order of a day (a trade before a quote with the same time) are as for IEX.

use dbn::{
    Cmbp1Msg, ConsolidatedBidAskPair, FlagSet, InstrumentDefMsg, RecordHeader, UNDEF_PRICE,
    UNDEF_TIMESTAMP, rtype,
};
use serde::Deserialize;
use serde_json::value::RawValue;
use tinytrader_dbn::dbn;
use tinytrader_dbn::{TradeConditions, TradeUpdate};

use crate::iex::{ABSENT, Side, price, quote_lot, side, timestamp};

/// The dataset name.
pub const DATASET: &str = "ALPACA.SIP";
/// Publisher id of the consolidated feed itself (a quote's header). Not a
/// Databento allocation.
pub const PUBLISHER: u16 = 40002;
/// Publisher ids of the exchanges start here: `A` is 40100, `Z` 40125.
pub const EXCHANGE_BASE: u16 = 40100;

/// An exchange's publisher id from Alpaca's one-letter code.
///
/// # Errors
/// Anything but one capital letter.
pub fn exchange(code: &str) -> Result<u16, String> {
    match code.as_bytes() {
        [c @ b'A'..=b'Z'] => Ok(EXCHANGE_BASE + u16::from(c - b'A')),
        _ => Err(format!("unknown exchange code {code:?}")),
    }
}

/// A stable instrument id for a symbol: FNV-1a of `ALPACA.SIP/<symbol>`,
/// avoiding the values DBN reserves.
#[must_use]
pub fn instrument_id(symbol: &str) -> u32 {
    let mut h: u32 = 0x811c_9dc5;
    for b in format!("{DATASET}/{symbol}").bytes() {
        h ^= u32::from(b);
        h = h.wrapping_mul(0x0100_0193);
    }
    match h {
        0 => 1,
        u32::MAX => u32::MAX - 1,
        h => h,
    }
}

/// A SIP quote row from Alpaca's history.
#[derive(Debug, Deserialize)]
pub struct RawQuote<'a> {
    #[serde(borrow)]
    pub t: &'a str,
    #[serde(borrow)]
    pub bp: &'a RawValue,
    pub bs: u64,
    #[serde(default)]
    pub bx: String,
    #[serde(borrow)]
    pub ap: &'a RawValue,
    #[serde(rename = "as")]
    pub as_: u64,
    #[serde(default)]
    pub ax: String,
}

/// A SIP trade row from Alpaca's history.
#[derive(Debug, Deserialize)]
pub struct RawTrade<'a> {
    #[serde(borrow)]
    pub t: &'a str,
    pub x: String,
    #[serde(borrow)]
    pub p: &'a RawValue,
    pub s: u64,
    pub i: u64,
    #[serde(default)]
    pub c: Vec<String>,
    #[serde(default)]
    pub z: String,
    #[serde(default)]
    pub u: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Quote {
    pub ts: u64,
    pub bid: Side,
    pub ask: Side,
    /// Quoting exchanges' publisher ids; 0 for an absent side.
    pub bid_pb: u16,
    pub ask_pb: u16,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Trade {
    pub ts: u64,
    pub px: i64,
    pub sz: u32,
    pub id: u64,
    pub exchange: u16,
    pub codes: Vec<String>,
    pub tape: u8,
    pub update: TradeUpdate,
}

/// A side's exchange, or 0 for an absent side.
fn side_exchange(s: Side, code: &str) -> Result<u16, String> {
    if s == ABSENT { Ok(0) } else { exchange(code) }
}

impl RawQuote<'_> {
    /// # Errors
    /// Refuses rows the mapping can't represent.
    pub fn parse(&self) -> Result<Quote, String> {
        let ts = timestamp(self.t)?;
        let lot = quote_lot(ts);
        let (bid, ask) = (side(self.bp, self.bs, lot)?, side(self.ap, self.as_, lot)?);
        Ok(Quote {
            ts,
            bid,
            ask,
            bid_pb: side_exchange(bid, &self.bx)?,
            ask_pb: side_exchange(ask, &self.ax)?,
        })
    }
}

impl RawTrade<'_> {
    /// # Errors
    /// Refuses rows the mapping can't represent.
    pub fn parse(&self) -> Result<Trade, String> {
        let update = match self.u.as_deref() {
            None => TradeUpdate::None,
            Some("canceled") => TradeUpdate::Canceled,
            Some("incorrect") => TradeUpdate::Incorrect,
            Some("corrected") => TradeUpdate::Corrected,
            Some(other) => return Err(format!("unknown trade update {other:?}")),
        };
        Ok(Trade {
            ts: timestamp(self.t)?,
            px: price(self.p.get())?,
            sz: u32::try_from(self.s).map_err(|_| format!("size {} doesn't fit", self.s))?,
            id: self.i,
            exchange: exchange(&self.x)?,
            codes: self.c.clone(),
            tape: self.z.bytes().next().unwrap_or(0),
            update,
        })
    }
}

/// A row of one day, in tape order.
pub enum Row {
    Quote(Quote),
    Trade(Trade),
}

impl Row {
    fn ts(&self) -> u64 {
        match self {
            Self::Quote(q) => q.ts,
            Self::Trade(t) => t.ts,
        }
    }
}

/// Merge a day's trades and quotes (each already in time order) into tape
/// order: by time, a trade before a quote with the same time.
///
/// # Errors
/// Refuses a lane that isn't in time order.
pub fn merge(trades: Vec<Trade>, quotes: Vec<Quote>) -> Result<Vec<Row>, String> {
    for (name, ordered) in [
        ("trades", trades.windows(2).all(|w| w[0].ts <= w[1].ts)),
        ("quotes", quotes.windows(2).all(|w| w[0].ts <= w[1].ts)),
    ] {
        if !ordered {
            return Err(format!("{name} are not in time order"));
        }
    }
    let mut out = Vec::with_capacity(trades.len() + quotes.len());
    let (mut t, mut q) = (trades.into_iter().peekable(), quotes.into_iter().peekable());
    loop {
        let take_trade = match (t.peek(), q.peek()) {
            (Some(a), Some(b)) => a.ts <= b.ts,
            (Some(_), None) => true,
            (None, Some(_)) => false,
            (None, None) => break,
        };
        out.push(if take_trade {
            Row::Trade(t.next().expect("peeked"))
        } else {
            Row::Quote(q.next().expect("peeked"))
        });
    }
    Ok(out)
}

fn level(q: &Quote) -> ConsolidatedBidAskPair {
    ConsolidatedBidAskPair {
        bid_px: q.bid.px,
        ask_px: q.ask.px,
        bid_sz: q.bid.sz,
        ask_sz: q.ask.sz,
        bid_pb: q.bid_pb,
        ask_pb: q.ask_pb,
        ..Default::default()
    }
}

const EMPTY: Quote = Quote {
    ts: 0,
    bid: ABSENT,
    ask: ABSENT,
    bid_pb: 0,
    ask_pb: 0,
};

/// Encode a day from history: its CMBP-1 records and their trade conditions
/// (the nth for the nth trade). Rows from history have no receive time.
///
/// # Errors
/// Refuses a trade whose conditions the layout can't hold.
pub fn encode_day(
    symbol: &str,
    rows: &[Row],
) -> Result<(Vec<Cmbp1Msg>, Vec<TradeConditions>), String> {
    let instrument = instrument_id(symbol);
    let mut book = EMPTY;
    let mut records = Vec::with_capacity(rows.len());
    let mut conditions = Vec::new();
    let mut seq: u32 = 0;
    for (i, row) in rows.iter().enumerate() {
        let (publisher, ts, action, px, sz, lv) = match row {
            Row::Quote(q) => {
                book = *q;
                (PUBLISHER, q.ts, b'M', UNDEF_PRICE, 0, level(q))
            }
            // Ties go to trades, so the book here is the one before the trade.
            Row::Trade(t) => {
                let (codes, count) = TradeConditions::codes_from(&t.codes)
                    .map_err(|e| format!("trade {} at {}: {e:?}", t.id, t.ts))?;
                let c = TradeConditions {
                    publisher_id: t.exchange,
                    instrument_id: instrument,
                    ts_event: t.ts,
                    price: t.px,
                    trade_id: t.id,
                    size: t.sz,
                    codes,
                    count,
                    tape: t.tape,
                    update: t.update,
                    trade_seq: seq,
                };
                c.encode()
                    .map_err(|e| format!("trade {} at {}: {e:?}", t.id, t.ts))?;
                conditions.push(c);
                seq = seq.wrapping_add(1);
                (t.exchange, t.ts, b'T', t.px, t.sz, level(&book))
            }
        };
        let last = rows.get(i + 1).is_none_or(|next| next.ts() != row.ts());
        let mut rec = Cmbp1Msg::default_for_schema(dbn::Schema::Cmbp1);
        rec.hd = RecordHeader::new::<Cmbp1Msg>(rtype::CMBP_1, publisher, instrument, ts);
        rec.price = px;
        rec.size = sz;
        rec.action = action as std::ffi::c_char;
        rec.side = b'N' as std::ffi::c_char;
        rec.flags = if last {
            FlagSet::empty().set_last()
        } else {
            FlagSet::empty()
        };
        rec.ts_recv = UNDEF_TIMESTAMP;
        rec.ts_in_delta = 0;
        rec.levels = [lv];
        records.push(rec);
    }
    Ok((records, conditions))
}

/// The day's instrument definition: a US equity in USD, one-cent tick.
#[must_use]
pub fn definition(symbol: &str) -> InstrumentDefMsg {
    let mut d = InstrumentDefMsg {
        hd: RecordHeader::new::<InstrumentDefMsg>(
            rtype::INSTRUMENT_DEF,
            PUBLISHER,
            instrument_id(symbol),
            UNDEF_TIMESTAMP,
        ),
        min_price_increment: 10_000_000,
        raw_symbol: dbn::record::str_to_c_chars(symbol).unwrap_or([0; 71]),
        security_update_action: b'A' as std::ffi::c_char,
        user_defined_instrument: b'N' as std::ffi::c_char,
        ..Default::default()
    };
    d.asset = dbn::record::str_to_c_chars(symbol).unwrap_or_default();
    d.currency = dbn::record::str_to_c_chars("USD").unwrap_or_default();
    d.security_type = dbn::record::str_to_c_chars("EQUITY").unwrap_or_default();
    d
}

#[cfg(test)]
mod tests {
    use super::*;

    fn q(ts: u64, bid: i64, bx: u16) -> Quote {
        Quote {
            ts,
            bid: Side { px: bid, sz: 100 },
            ask: ABSENT,
            bid_pb: bx,
            ask_pb: 0,
        }
    }
    fn t(ts: u64, x: u16) -> Trade {
        Trade {
            ts,
            px: 5,
            sz: 10,
            id: ts,
            exchange: x,
            codes: vec!["@".into()],
            tape: b'B',
            update: TradeUpdate::None,
        }
    }

    #[test]
    fn exchanges_map_to_fixed_publishers() {
        assert_eq!(exchange("A"), Ok(40100));
        assert_eq!(exchange("V"), Ok(40121));
        assert!(exchange("").is_err() && exchange("NY").is_err() && exchange("a").is_err());
    }

    #[test]
    fn parses_rows_with_their_exchanges() {
        let row: RawQuote = serde_json::from_str(
            r#"{"t":"2026-10-01T13:30:00.5Z","bp":774.5,"bs":2,"bx":"Q","ap":0,"as":0,"ax":" ","c":["R"],"z":"B"}"#,
        )
        .unwrap();
        let p = row.parse().unwrap();
        assert_eq!((p.bid.sz, p.bid_pb, p.ask, p.ask_pb), (2, 40116, ABSENT, 0));
        let tr: RawTrade = serde_json::from_str(
            r#"{"t":"2026-10-01T13:30:00.5Z","x":"V","p":774.51,"s":100,"i":7,"c":["@","I"],"z":"B"}"#,
        )
        .unwrap();
        assert_eq!(tr.parse().unwrap().exchange, 40121);
    }

    #[test]
    fn trades_carry_the_prior_book_and_their_exchange() {
        let rows = merge(vec![t(2, 40121)], vec![q(1, 7, 40116), q(2, 9, 40116)]).unwrap();
        let (recs, conds) = encode_day("SPY", &rows).unwrap();
        assert_eq!(recs.len(), 3);
        let trade = &recs[1];
        assert_eq!(trade.action as u8, b'T');
        assert_eq!(trade.hd.publisher_id, 40121);
        // The book before the trade: the quote at time 1, not the one at 2.
        assert_eq!(trade.levels[0].bid_px, 7);
        assert_eq!(trade.levels[0].bid_pb, 40116);
        assert_eq!(recs[2].hd.publisher_id, PUBLISHER);
        // F_LAST only where the next record's time differs.
        assert!(recs[0].flags.is_last() && !recs[1].flags.is_last() && recs[2].flags.is_last());
        assert_eq!((conds.len(), conds[0].trade_seq, conds[0].publisher_id), (1, 0, 40121));
    }
}

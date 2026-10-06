//! Alpaca IEX quotes and trades as DBN `MBP-1`, with trade conditions as
//! `tinytrader.trade-conditions/1` records.
//!
//! The mapping matches the `ALPACA.IEX` history that models are trained on
//! (Hyphae's stored days), which follows Databento's conventions where they
//! have one:
//!
//! - A quote is an `M` record with side `N` (Alpaca sends both sides in one
//!   update), `price` undefined and `size` 0; the book is in `levels[0]`.
//! - A trade is a `T` record with side `N`, its own price and size, carrying
//!   the book as it stood before the trade.
//! - A side is absent when its price is exactly `0` and its size 0; it is
//!   written as an undefined price and size 0.
//! - Prices are parsed exactly from the JSON text and scaled by 10^9; quote
//!   sizes are shares (round lots, times 100, before 2025-11-03).
//! - `F_LAST` is set unless the next record has the same `ts_event`.
//! - `ts_recv` is when the websocket frame arrived, or undefined for rows
//!   from history; `ts_in_delta`, `depth`, `sequence` and order counts are 0.
//! - In history, a trade comes before a quote with the same timestamp.

use dbn::{
    BidAskPair, FlagSet, InstrumentDefMsg, Mbp1Msg, RecordHeader, UNDEF_PRICE, UNDEF_TIMESTAMP,
    rtype,
};
use serde::Deserialize;
use serde_json::value::RawValue;
use tinytrader_dbn::dbn;
use tinytrader_dbn::{TradeConditions, TradeUpdate};

/// The dataset name.
pub const DATASET: &str = "ALPACA.IEX";
/// Publisher id for Alpaca's IEX feed in tinytrader's data. Not a Databento
/// allocation.
pub const PUBLISHER: u16 = 40001;
/// From this day on, Alpaca reports quote sizes in shares; before it, in
/// round lots of 100.
const SHARES_FROM: (i32, u8, u8) = (2025, 11, 3);

/// A stable instrument id for a symbol: FNV-1a of `ALPACA.IEX/<symbol>`,
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

/// A quote row, as Alpaca sends it on the websocket and in history.
#[derive(Debug, Deserialize)]
pub struct RawQuote<'a> {
    #[serde(borrow)]
    pub t: &'a str,
    #[serde(borrow)]
    pub bp: &'a RawValue,
    pub bs: u64,
    #[serde(borrow)]
    pub ap: &'a RawValue,
    #[serde(rename = "as")]
    pub as_: u64,
}

/// A trade row, as Alpaca sends it on the websocket and in history.
#[derive(Debug, Deserialize)]
pub struct RawTrade<'a> {
    #[serde(borrow)]
    pub t: &'a str,
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

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Side {
    pub px: i64,
    pub sz: u32,
}

const ABSENT: Side = Side {
    px: UNDEF_PRICE,
    sz: 0,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Quote {
    pub ts: u64,
    pub bid: Side,
    pub ask: Side,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Trade {
    pub ts: u64,
    pub px: i64,
    pub sz: u32,
    pub id: u64,
    pub codes: Vec<String>,
    pub tape: u8,
    pub update: TradeUpdate,
}

/// An exact decimal (`774.75`) as a DBN fixed-point price (1e-9 units).
///
/// # Errors
/// Refuses anything but plain digits with an optional fraction, more
/// precision than 1e-9 (unless the extra digits are zero), and overflow.
pub fn price(text: &str) -> Result<i64, String> {
    let bad = || format!("unreadable price {text:?}");
    let (int, frac) = text.split_once('.').unwrap_or((text, ""));
    if int.is_empty()
        || !int.bytes().all(|b| b.is_ascii_digit())
        || !frac.bytes().all(|b| b.is_ascii_digit())
        || (text.contains('.') && frac.is_empty())
    {
        return Err(bad());
    }
    let (kept, extra) = frac.split_at(frac.len().min(9));
    if extra.bytes().any(|b| b != b'0') {
        return Err(format!("price {text} is finer than 1e-9"));
    }
    let mut v: i64 = int.parse().map_err(|_| bad())?;
    for b in kept.bytes() {
        v = v.checked_mul(10).ok_or_else(bad)? + i64::from(b - b'0');
    }
    v = v
        .checked_mul(10_i64.pow(9 - u32::try_from(kept.len()).unwrap_or(9)))
        .ok_or_else(bad)?;
    if v == UNDEF_PRICE {
        return Err(bad());
    }
    Ok(v)
}

/// An Alpaca timestamp (`2026-10-01T12:00:01.1299614Z`) as nanoseconds.
///
/// # Errors
/// Refuses anything but UTC (`Z` or `+00:00`) with up to nine fractional digits.
pub fn timestamp(text: &str) -> Result<u64, String> {
    let bad = || format!("unreadable timestamp {text:?}");
    let body = text
        .strip_suffix('Z')
        .or_else(|| text.strip_suffix("+00:00"))
        .ok_or_else(bad)?;
    let (whole, frac) = body.split_once('.').unwrap_or((body, ""));
    if frac.len() > 9
        || !frac.bytes().all(|b| b.is_ascii_digit())
        || (body.contains('.') && frac.is_empty())
    {
        return Err(bad());
    }
    let b = whole.as_bytes();
    if b.len() != 19
        || b[4] != b'-'
        || b[7] != b'-'
        || b[10] != b'T'
        || b[13] != b':'
        || b[16] != b':'
    {
        return Err(bad());
    }
    let num = |s: &str| s.parse::<i64>().map_err(|_| bad());
    let (y, m, d) = (num(&whole[0..4])?, num(&whole[5..7])?, num(&whole[8..10])?);
    let (hh, mm, ss) = (
        num(&whole[11..13])?,
        num(&whole[14..16])?,
        num(&whole[17..19])?,
    );
    if !(1..=12).contains(&m) || !(1..=31).contains(&d) || hh > 23 || mm > 59 || ss > 60 {
        return Err(bad());
    }
    let mut nanos = 0i64;
    for i in 0..9 {
        nanos = nanos * 10 + frac.as_bytes().get(i).map_or(0, |b| i64::from(b - b'0'));
    }
    let secs = days_from_civil(y, m, d) * 86_400 + hh * 3600 + mm * 60 + ss;
    u64::try_from(secs * 1_000_000_000 + nanos).map_err(|_| bad())
}

/// Days since 1970-01-01 (Howard Hinnant's algorithm).
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// Quote sizes on this day are multiplied by this to give shares.
#[must_use]
pub fn quote_lot(ts: u64) -> u32 {
    let (y, m, d) = SHARES_FROM;
    let from = u64::try_from(days_from_civil(i64::from(y), i64::from(m), i64::from(d)))
        .unwrap_or(0)
        * 86_400_000_000_000;
    if ts < from { 100 } else { 1 }
}

fn side(px: &RawValue, sz: u64, lot: u32) -> Result<Side, String> {
    let text = px.get();
    match (text == "0", sz == 0) {
        (true, true) => Ok(ABSENT),
        (false, false) => Ok(Side {
            px: price(text)?,
            sz: u32::try_from(sz.checked_mul(u64::from(lot)).ok_or("size overflows")?)
                .map_err(|_| format!("size {sz} doesn't fit"))?,
        }),
        _ => Err(format!("inconsistent side: price {text}, size {sz}")),
    }
}

impl RawQuote<'_> {
    /// # Errors
    /// Refuses rows the mapping can't represent.
    pub fn parse(&self) -> Result<Quote, String> {
        let ts = timestamp(self.t)?;
        let lot = quote_lot(ts);
        Ok(Quote {
            ts,
            bid: side(self.bp, self.bs, lot)?,
            ask: side(self.ap, self.as_, lot)?,
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
            codes: self.c.clone(),
            tape: self.z.bytes().next().unwrap_or(0),
            update,
        })
    }
}

/// Turns one symbol's quotes and trades into records, keeping its book.
///
/// A trade carries the book as it stood before any quote with the trade's
/// own time, whatever order the stream delivers them in. In history, where
/// trades come before quotes at equal times, that is simply the last quote.
pub struct Encoder {
    pub publisher: u16,
    pub instrument: u32,
    book: (Side, Side),
    /// Time of the latest quote, and the book before the first quote at
    /// that time.
    book_ts: u64,
    before: (Side, Side),
    trades: u32,
}

impl Encoder {
    #[must_use]
    pub fn new(symbol: &str) -> Self {
        Self {
            publisher: PUBLISHER,
            instrument: instrument_id(symbol),
            book: (ABSENT, ABSENT),
            book_ts: 0,
            before: (ABSENT, ABSENT),
            trades: 0,
        }
    }

    fn record(
        &self,
        ts: u64,
        ts_recv: u64,
        action: u8,
        price: i64,
        size: u32,
        book: (Side, Side),
    ) -> Mbp1Msg {
        let (bid, ask) = book;
        Mbp1Msg {
            hd: RecordHeader::new::<Mbp1Msg>(rtype::MBP_1, self.publisher, self.instrument, ts),
            price,
            size,
            action: action as std::ffi::c_char,
            side: b'N' as std::ffi::c_char,
            flags: FlagSet::empty(),
            depth: 0,
            ts_recv,
            ts_in_delta: 0,
            sequence: 0,
            levels: [BidAskPair {
                bid_px: bid.px,
                ask_px: ask.px,
                bid_sz: bid.sz,
                ask_sz: ask.sz,
                bid_ct: 0,
                ask_ct: 0,
            }],
        }
    }

    /// A quote's record; the book then holds the quote.
    pub fn quote(&mut self, q: &Quote, ts_recv: u64) -> Mbp1Msg {
        if q.ts != self.book_ts {
            self.before = self.book;
            self.book_ts = q.ts;
        }
        self.book = (q.bid, q.ask);
        self.record(q.ts, ts_recv, b'M', UNDEF_PRICE, 0, self.book)
    }

    /// A trade's record, carrying the book before it, and its conditions.
    ///
    /// # Errors
    /// The conditions are refused if the layout can't hold them (more than
    /// four codes, a multi-character code, a tape other than A, B or C).
    pub fn trade(
        &mut self,
        t: &Trade,
        ts_recv: u64,
    ) -> (
        Mbp1Msg,
        Result<TradeConditions, tinytrader_dbn::trade_conditions::Invalid>,
    ) {
        let book = if t.ts > self.book_ts {
            self.book
        } else {
            self.before
        };
        let rec = self.record(t.ts, ts_recv, b'T', t.px, t.sz, book);
        let seq = self.trades;
        self.trades = self.trades.wrapping_add(1);
        let cond = TradeConditions::codes_from(&t.codes).and_then(|(codes, count)| {
            let c = TradeConditions {
                publisher_id: self.publisher,
                instrument_id: self.instrument,
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
            c.encode().map(|_| c)
        });
        (rec, cond)
    }

    /// The instrument definition for this encoder's symbol: a US equity in
    /// USD with a one-cent tick.
    #[must_use]
    pub fn definition(&self, symbol: &str) -> InstrumentDefMsg {
        let text = |s: &str| dbn::record::str_to_c_chars(s).unwrap_or([0; 71]);
        let mut d = InstrumentDefMsg {
            hd: RecordHeader::new::<InstrumentDefMsg>(
                rtype::INSTRUMENT_DEF,
                self.publisher,
                self.instrument,
                UNDEF_TIMESTAMP,
            ),
            min_price_increment: 10_000_000,
            raw_instrument_id: 0,
            raw_symbol: text(symbol),
            security_update_action: b'A' as std::ffi::c_char,
            user_defined_instrument: b'N' as std::ffi::c_char,
            leg_count: 0,
            leg_index: 0,
            ..Default::default()
        };
        d.asset = dbn::record::str_to_c_chars(symbol).unwrap_or_default();
        d.currency = dbn::record::str_to_c_chars("USD").unwrap_or_default();
        d.security_type = dbn::record::str_to_c_chars("EQUITY").unwrap_or_default();
        d
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
/// order: by time, a trade before a quote with the same time, and source
/// order otherwise.
///
/// # Errors
/// Refuses a lane that isn't in time order.
pub fn merge(trades: Vec<Trade>, quotes: Vec<Quote>) -> Result<Vec<Row>, String> {
    for (name, times) in [
        ("trades", trades.iter().map(|t| t.ts).collect::<Vec<_>>()),
        ("quotes", quotes.iter().map(|q| q.ts).collect()),
    ] {
        if times.windows(2).any(|w| w[1] < w[0]) {
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

/// Encode a day from history: its MBP-1 records and their trade conditions.
/// Rows from history have no receive time.
///
/// # Errors
/// Refuses a trade whose conditions the layout can't hold.
pub fn encode_day(
    symbol: &str,
    rows: &[Row],
) -> Result<(Vec<Mbp1Msg>, Vec<TradeConditions>), String> {
    let mut enc = Encoder::new(symbol);
    let mut records = Vec::with_capacity(rows.len());
    let mut conditions = Vec::new();
    for (i, row) in rows.iter().enumerate() {
        let mut rec = match row {
            Row::Quote(q) => enc.quote(q, UNDEF_TIMESTAMP),
            Row::Trade(t) => {
                let (rec, cond) = enc.trade(t, UNDEF_TIMESTAMP);
                conditions.push(cond.map_err(|e| format!("trade {} at {}: {e}", t.id, t.ts))?);
                rec
            }
        };
        if rows.get(i + 1).is_none_or(|next| next.ts() != row.ts()) {
            rec.flags = FlagSet::empty().set_last();
        }
        records.push(rec);
    }
    Ok((records, conditions))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prices_are_exact() {
        assert_eq!(price("774.75"), Ok(774_750_000_000));
        assert_eq!(price("765.7"), Ok(765_700_000_000));
        assert_eq!(price("1"), Ok(1_000_000_000));
        assert_eq!(price("0.0001"), Ok(100_000));
        assert_eq!(price("1.0000000010"), Ok(1_000_000_001));
        assert!(price("1.00000000001").is_err());
        assert!(price("-1").is_err() && price("1e3").is_err() && price("1.").is_err());
    }

    #[test]
    fn timestamps_pad_short_fractions() {
        assert_eq!(
            timestamp("2025-11-03T09:00:00.00233883Z"),
            Ok(1_762_160_400_002_338_830)
        );
        assert_eq!(
            timestamp("2026-10-01T12:00:01.1299614Z"),
            Ok(timestamp("2026-10-01T12:00:01Z").unwrap() + 129_961_400)
        );
        assert!(timestamp("2026-10-01T12:00:01+01:00").is_err());
    }

    #[test]
    fn quote_sizes_switch_to_shares() {
        assert_eq!(quote_lot(timestamp("2025-11-02T23:59:59Z").unwrap()), 100);
        assert_eq!(quote_lot(timestamp("2025-11-03T00:00:00Z").unwrap()), 1);
    }

    #[test]
    fn trades_carry_the_prior_book_and_ties_go_to_trades() {
        let q = |ts, bid| Quote {
            ts,
            bid: Side { px: bid, sz: 1 },
            ask: ABSENT,
        };
        let t = |ts| Trade {
            ts,
            px: 5,
            sz: 1,
            id: ts,
            codes: vec!["@".into()],
            tape: b'B',
            update: TradeUpdate::None,
        };
        let rows = merge(vec![t(1), t(2)], vec![q(2, 10), q(3, 20)]).unwrap();
        let (recs, conds) = encode_day("SPY", &rows).unwrap();
        let kinds: Vec<(u8, u64, i64, bool)> = recs
            .iter()
            .map(|r| {
                (
                    r.action as u8,
                    r.hd.ts_event,
                    r.levels[0].bid_px,
                    r.flags.is_last(),
                )
            })
            .collect();
        assert_eq!(
            kinds,
            [
                (b'T', 1, UNDEF_PRICE, true),
                (b'T', 2, UNDEF_PRICE, false), // before the quote at the same time
                (b'M', 2, 10, true),
                (b'M', 3, 20, true),
            ]
        );
        assert_eq!(conds.len(), 2);
        assert_eq!(conds[1].trade_seq, 1);
    }
}

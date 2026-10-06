//! `glacier.trade-conditions/1`: the sale conditions of one trade.
//!
//! Standard DBN has no field for trade condition codes. This record carries
//! them beside a trade record, keeping the venue's codes exactly as sent, so
//! they can be converted to Databento's representation losslessly once one
//! exists. The byte layout is identical to Hyphae's `TradeConditions`
//! (rtype `0xD4`, layout revision 1), so stored Hyphae data and Glacier's live
//! data read the same way.
//!
//! | Offset | Type | Field |
//! |---:|---|---|
//! | 0 | u8 | length in 4-byte words: 12 |
//! | 1 | u8 | rtype `0xD4` |
//! | 2 | u16 | publisher id of the joined trade |
//! | 4 | u32 | instrument id of the joined trade |
//! | 8 | u64 | `ts_event` of the joined trade |
//! | 16 | i64 | price of the joined trade (fixed point, 1e-9) |
//! | 24 | u64 | the venue's trade id |
//! | 32 | u32 | size of the joined trade |
//! | 36 | [u8; 4] | condition codes, one printable ASCII byte each, in source order, NUL after `count` |
//! | 40 | u8 | tape: `A`, `B` or `C` |
//! | 41 | u8 | update status (see [`TradeUpdate`]) |
//! | 42 | u8 | number of codes, 0 to 4 |
//! | 43 | u8 | layout revision: 1 |
//! | 44 | u32 | ordinal of the joined trade among the trades of its stream |
//!
//! All integers are little-endian.

use dbn::{UNDEF_PRICE, UNDEF_TIMESTAMP};

/// The record type. Not allocated by DBN.
pub const RTYPE: u8 = 0xD4;
/// Record size in bytes.
pub const LEN: usize = 48;
/// Layout revision, at byte 43.
pub const REVISION: u8 = 1;
/// Most condition codes one record holds. US equity sale conditions have at
/// most four single-character codes.
pub const MAX_CODES: usize = 4;
/// The record's name and version, as used in documentation and headers.
pub const NAME: &str = "glacier.trade-conditions/1";

/// Whether a reported trade was later changed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TradeUpdate {
    /// An ordinary trade.
    None = 0,
    /// The print was canceled.
    Canceled = 1,
    /// The print was reported in error; a corrected print replaces it.
    Incorrect = 2,
    /// The replacement for an incorrect print.
    Corrected = 3,
}

impl TradeUpdate {
    fn from_byte(b: u8) -> Option<Self> {
        Some(match b {
            0 => Self::None,
            1 => Self::Canceled,
            2 => Self::Incorrect,
            3 => Self::Corrected,
            _ => return None,
        })
    }
}

/// One trade's conditions, as fields.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TradeConditions {
    pub publisher_id: u16,
    pub instrument_id: u32,
    pub ts_event: u64,
    pub price: i64,
    /// The venue's trade id.
    pub trade_id: u64,
    pub size: u32,
    /// Condition codes in source order; only the first `count` are used.
    pub codes: [u8; MAX_CODES],
    pub count: u8,
    /// `b'A'`, `b'B'` or `b'C'`.
    pub tape: u8,
    pub update: TradeUpdate,
    /// Ordinal of the joined trade among its stream's trades.
    pub trade_seq: u32,
}

/// Why a record can't be built or read.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Invalid {
    Framing,
    /// More than four codes, or a code that isn't one printable ASCII byte.
    Codes,
    Tape,
    Update,
    Revision,
    /// An undefined time, price or size, or a reserved id.
    Value,
    /// Padding after the codes isn't NUL.
    Padding,
}

impl std::fmt::Display for Invalid {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Framing => "not a 48-byte trade conditions record",
            Self::Codes => "condition codes must be at most four single printable ASCII bytes",
            Self::Tape => "tape must be A, B or C",
            Self::Update => "unknown update status",
            Self::Revision => "unknown layout revision",
            Self::Value => "undefined time, price or size, or a reserved id",
            Self::Padding => "non-NUL padding after the condition codes",
        })
    }
}

impl std::error::Error for Invalid {}

impl TradeConditions {
    /// Build from a trade's fields and its codes as the venue sent them
    /// (each code a string, as Alpaca's `c` array).
    ///
    /// # Errors
    /// Refuses more than four codes or a code that isn't one printable
    /// ASCII character.
    pub fn codes_from<S: AsRef<str>>(codes: &[S]) -> Result<([u8; MAX_CODES], u8), Invalid> {
        if codes.len() > MAX_CODES {
            return Err(Invalid::Codes);
        }
        let mut out = [0; MAX_CODES];
        for (slot, code) in out.iter_mut().zip(codes) {
            match code.as_ref().as_bytes() {
                [b] if (0x20..=0x7E).contains(b) => *slot = *b,
                _ => return Err(Invalid::Codes),
            }
        }
        Ok((out, u8::try_from(codes.len()).unwrap_or(u8::MAX)))
    }

    /// The codes in use.
    #[must_use]
    pub fn codes(&self) -> &[u8] {
        &self.codes[..usize::from(self.count).min(MAX_CODES)]
    }

    fn check(&self) -> Result<(), Invalid> {
        let n = usize::from(self.count);
        if n > MAX_CODES || !self.codes[..n].iter().all(|b| (0x20..=0x7E).contains(b)) {
            return Err(Invalid::Codes);
        }
        if self.codes[n..].iter().any(|b| *b != 0) {
            return Err(Invalid::Padding);
        }
        if !(b'A'..=b'C').contains(&self.tape) {
            return Err(Invalid::Tape);
        }
        if self.ts_event == UNDEF_TIMESTAMP
            || self.price == UNDEF_PRICE
            || self.size == u32::MAX
            || self.publisher_id == 0
            || self.publisher_id == u16::MAX
            || self.instrument_id == 0
            || self.instrument_id == u32::MAX
        {
            return Err(Invalid::Value);
        }
        Ok(())
    }

    /// The record's bytes.
    ///
    /// # Errors
    /// Refuses fields the layout can't represent.
    pub fn encode(&self) -> Result<[u8; LEN], Invalid> {
        self.check()?;
        let mut b = [0u8; LEN];
        b[0] = (LEN / 4) as u8;
        b[1] = RTYPE;
        b[2..4].copy_from_slice(&self.publisher_id.to_le_bytes());
        b[4..8].copy_from_slice(&self.instrument_id.to_le_bytes());
        b[8..16].copy_from_slice(&self.ts_event.to_le_bytes());
        b[16..24].copy_from_slice(&self.price.to_le_bytes());
        b[24..32].copy_from_slice(&self.trade_id.to_le_bytes());
        b[32..36].copy_from_slice(&self.size.to_le_bytes());
        b[36..40].copy_from_slice(&self.codes);
        b[40] = self.tape;
        b[41] = self.update as u8;
        b[42] = self.count;
        b[43] = REVISION;
        b[44..48].copy_from_slice(&self.trade_seq.to_le_bytes());
        Ok(b)
    }

    /// Read a record. Only bytes that re-encode identically are accepted.
    ///
    /// # Errors
    /// Refuses anything that isn't a well-formed revision 1 record.
    pub fn decode(b: &[u8]) -> Result<Self, Invalid> {
        let b: &[u8; LEN] = b.try_into().map_err(|_| Invalid::Framing)?;
        if usize::from(b[0]) * 4 != LEN || b[1] != RTYPE {
            return Err(Invalid::Framing);
        }
        if b[43] != REVISION {
            return Err(Invalid::Revision);
        }
        let u16_at = |i: usize| u16::from_le_bytes([b[i], b[i + 1]]);
        let u32_at = |i: usize| u32::from_le_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]]);
        let u64_at = |i: usize| {
            let mut a = [0; 8];
            a.copy_from_slice(&b[i..i + 8]);
            u64::from_le_bytes(a)
        };
        let rec = Self {
            publisher_id: u16_at(2),
            instrument_id: u32_at(4),
            ts_event: u64_at(8),
            price: i64::from_le_bytes(u64_at(16).to_le_bytes()),
            trade_id: u64_at(24),
            size: u32_at(32),
            codes: [b[36], b[37], b[38], b[39]],
            tape: b[40],
            update: TradeUpdate::from_byte(b[41]).ok_or(Invalid::Update)?,
            count: b[42],
            trade_seq: u32_at(44),
        };
        rec.check()?;
        Ok(rec)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> TradeConditions {
        let (codes, count) = TradeConditions::codes_from(&["@", "F", "I"]).unwrap();
        TradeConditions {
            publisher_id: 23946,
            instrument_id: 425_464_287,
            ts_event: 1_791_207_000_000_000_000,
            price: 774_750_000_000,
            trade_id: 52_983_525_029_461,
            size: 100,
            codes,
            count,
            tape: b'B',
            update: TradeUpdate::None,
            trade_seq: 7,
        }
    }

    #[test]
    fn round_trips_with_the_documented_layout() {
        let rec = sample();
        let b = rec.encode().unwrap();
        assert_eq!(b.len(), 48);
        assert_eq!((b[0], b[1]), (12, 0xD4));
        assert_eq!(&b[36..40], b"@FI\0");
        assert_eq!((b[40], b[41], b[42], b[43]), (b'B', 0, 3, 1));
        assert_eq!(u32::from_le_bytes(b[44..48].try_into().unwrap()), 7);
        assert_eq!(TradeConditions::decode(&b).unwrap(), rec);
        assert_eq!(rec.codes(), b"@FI");
    }

    #[test]
    fn refuses_what_the_layout_cannot_hold() {
        assert_eq!(
            TradeConditions::codes_from(&["@", "F", "I", "T", "X"]),
            Err(Invalid::Codes)
        );
        assert_eq!(TradeConditions::codes_from(&["@@"]), Err(Invalid::Codes));
        let mut r = sample();
        r.tape = b'D';
        assert_eq!(r.encode(), Err(Invalid::Tape));
        let mut b = sample().encode().unwrap();
        b[39] = b'Z'; // padding after three codes
        assert_eq!(TradeConditions::decode(&b), Err(Invalid::Padding));
        let mut b = sample().encode().unwrap();
        b[43] = 2;
        assert_eq!(TradeConditions::decode(&b), Err(Invalid::Revision));
    }
}

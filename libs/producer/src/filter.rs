//! One file's records for one day: those of the asked schema inside the day's
//! window. A batch file can hold other days (a range file listed on each of its
//! days) and a folder can hold sidecars, so neither is trusted to hold only the
//! day. Companion records (an extension record after its trade) go with the
//! record before them. Symbol mappings, system and error messages in the
//! stream (a recording carries them in-stream, as the relay sends them) pass
//! too, inside the window.
//!
//! With an expiry horizon, options expiring after it (or before the day) are
//! dropped: their symbol mappings and their records. An instrument is judged by
//! its OCC symbol, from the file's mappings or a mapping in the stream; one with
//! no mapping, or a symbol that isn't an option's, passes.
use dbn::decode::DecodeRecordRef;
use dbn::{RType, RecordBuf, RecordRef, SymbolMappingMsg, rtype};
use std::collections::HashSet;
use tinytrader_dbn::Timing;

pub(crate) struct DayFilter<D> {
    inner: D,
    rtype: u8,
    window: (u64, u64),
    /// Whether the last record that had a time was kept.
    keeping: bool,
    buf: Option<RecordBuf>,
    /// The expiry horizon: the day's start and the days after it allowed.
    horizon: Option<(u64, u32)>,
    /// Instruments outside the horizon.
    denied: HashSet<u32>,
}

impl<D> DayFilter<D> {
    pub(crate) fn new(inner: D, rtype: RType, window: (u64, u64)) -> Self {
        Self {
            inner,
            rtype: rtype as u8,
            window,
            keeping: false,
            buf: None,
            horizon: None,
            denied: HashSet::new(),
        }
    }

    /// Drops options expiring more than `days` after the day starting at `day`,
    /// and `denied`, the file's mapped instruments already judged outside it.
    pub(crate) fn horizon(mut self, day: u64, days: u32, denied: HashSet<u32>) -> Self {
        self.horizon = Some((day, days));
        self.denied = denied;
        self
    }
}

/// Whether `raw`, an instrument's raw symbol, is inside the horizon: not an OCC
/// option symbol, or one expiring from the day to `days` after it.
pub(crate) fn admitted(raw: &str, horizon: Option<(u64, u32)>) -> bool {
    let Some((day, days)) = horizon else {
        return true;
    };
    let Some(expiry) = occ_expiry(raw) else {
        return true;
    };
    expiry >= day && expiry - day <= u64::from(days) * 86_400_000_000_000
}

/// The expiry day (UTC midnight, ns) of an OCC symbol: a root padded to six, then
/// YYMMDD, C or P, and eight digits of strike.
fn occ_expiry(raw: &str) -> Option<u64> {
    let b = raw.as_bytes();
    if b.len() != 21 || !matches!(b[12], b'C' | b'P') || !b[6..12].iter().all(u8::is_ascii_digit) {
        return None;
    }
    crate::day_ns(&format!(
        "20{}-{}-{}",
        &raw[6..8],
        &raw[8..10],
        &raw[10..12]
    ))
}

impl<D: DecodeRecordRef> DecodeRecordRef for DayFilter<D> {
    fn decode_record_ref(&mut self) -> dbn::Result<Option<RecordRef<'_>>> {
        loop {
            let Some(rec) = self.inner.decode_record_ref()? else {
                self.buf = None;
                return Ok(None);
            };
            let keep = match tinytrader_dbn::timing(&rec, 0).map_err(dbn::Error::decode)? {
                Timing::Companion => self.keeping,
                Timing::At(ts) | Timing::Flagged(ts) => {
                    let rtype = rec.header().rtype;
                    let iid = rec.header().instrument_id;
                    if self.horizon.is_some()
                        && let Some(m) = rec.get::<SymbolMappingMsg>()
                    {
                        // A mapping in the stream judges its instrument afresh (an id
                        // can be reused for another contract).
                        if admitted(m.stype_out_symbol().unwrap_or(""), self.horizon) {
                            self.denied.remove(&iid);
                        } else {
                            self.denied.insert(iid);
                        }
                    }
                    let wanted = !self.denied.contains(&iid)
                        && (rtype == self.rtype
                            || rtype == rtype::SYMBOL_MAPPING
                            || rtype == rtype::SYSTEM
                            || rtype == rtype::ERROR);
                    self.keeping = wanted && ts >= self.window.0 && ts < self.window.1;
                    self.keeping
                }
            };
            if keep {
                // Copied out, since a borrow returned from inside the loop would hold the
                // decoder for the rest of it.
                self.buf = Some(RecordBuf::try_from(rec)?);
                break;
            }
        }
        Ok(self.buf.as_ref().map(RecordBuf::as_rec_ref))
    }
}

#[cfg(test)]
mod tests {
    use super::{DayFilter, admitted};
    use dbn::decode::DecodeRecordRef;
    use dbn::{
        CbboMsg, RType, RecordBuf, RecordHeader, RecordRef, SType, Schema, SymbolMappingMsg, rtype,
    };

    const D27: u64 = 1_793_059_200_000_000_000; // 2026-10-27T00:00Z
    const DAY: u64 = 86_400_000_000_000;

    #[test]
    fn occ_symbols_inside_and_outside_the_horizon() {
        let h = Some((D27, 30));
        assert!(admitted("SPY   261027C00580000", h));
        assert!(admitted("SPY   261126P00570000", h));
        assert!(!admitted("SPY   261127P00570000", h));
        assert!(!admitted("SPY   261026C00580000", h));
        assert!(admitted("SPY", h), "not an option");
        assert!(admitted("SPY   261231C00600000", None), "no horizon");
    }

    /// Records in order; `next` gives them one at a time.
    struct Recs(Vec<RecordBuf>, usize);
    impl DecodeRecordRef for Recs {
        fn decode_record_ref(&mut self) -> dbn::Result<Option<RecordRef<'_>>> {
            self.1 += 1;
            Ok(self.0.get(self.1 - 1).map(RecordBuf::as_rec_ref))
        }
    }

    /// A mapping in the stream judges its instrument, and a later one for the same
    /// id judges it again.
    #[test]
    fn a_mapping_in_the_stream_judges_its_instrument() {
        let t = D27 + 14 * 3_600_000_000_000;
        let map = |ts: u64, raw: &str| {
            let m =
                SymbolMappingMsg::new(7, ts, SType::Parent, "SPY.OPT", SType::RawSymbol, raw, 0, 0)
                    .unwrap();
            RecordBuf::from(m)
        };
        let quote = |ts: u64| {
            RecordBuf::from(CbboMsg {
                hd: RecordHeader::new::<CbboMsg>(rtype::CBBO_1M, 30, 7, u64::MAX),
                ts_recv: ts,
                ..CbboMsg::default_for_schema(Schema::Cbbo1M)
            })
        };
        let recs = Recs(
            vec![
                map(t, "SPY   261231C00600000"),
                quote(t + 1),
                map(t + 2, "SPY   261030C00600000"),
                quote(t + 3),
            ],
            0,
        );
        let mut f = DayFilter::new(recs, RType::Cbbo1M, (D27, D27 + DAY)).horizon(
            D27,
            30,
            std::collections::HashSet::new(),
        );
        let mut kept = Vec::new();
        while let Some(r) = f.decode_record_ref().unwrap() {
            kept.push((r.header().rtype, tinytrader_dbn::timing(&r, 0).ok()));
        }
        assert_eq!(kept.len(), 2, "{kept:?}");
        assert_eq!(kept[0].0, rtype::SYMBOL_MAPPING);
        assert_eq!(kept[1].0, rtype::CBBO_1M);
    }
}

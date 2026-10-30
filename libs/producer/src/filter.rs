//! One file's records for one day: those of the asked schema inside the day's
//! window. A batch file can hold other days (a range file listed on each of its
//! days) and a folder can hold sidecars, so neither is trusted to hold only the
//! day. Companion records (an extension record after its trade) go with the
//! record before them.
use dbn::decode::DecodeRecordRef;
use dbn::{RType, RecordBuf, RecordRef};
use tinytrader_dbn::Timing;

pub(crate) struct DayFilter<D> {
    inner: D,
    rtype: u8,
    window: (u64, u64),
    /// Whether the last record that had a time was kept.
    keeping: bool,
    buf: Option<RecordBuf>,
}

impl<D> DayFilter<D> {
    pub(crate) fn new(inner: D, rtype: RType, window: (u64, u64)) -> Self {
        Self {
            inner,
            rtype: rtype as u8,
            window,
            keeping: false,
            buf: None,
        }
    }
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
                    self.keeping = rec.header().rtype == self.rtype
                        && ts >= self.window.0
                        && ts < self.window.1;
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

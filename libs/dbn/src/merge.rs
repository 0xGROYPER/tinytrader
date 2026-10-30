//! One total order over several DBN sources (decision 0009, "Ordering").
//!
//! Every delivery has the key `(ts, class, seq)`. A market record's `ts` is its
//! **index time**: `ts_recv` where the record has it, or `ts_event` plus the
//! source's declared latency offset where the record carries `F_BAD_TS_RECV`
//! (replayed or filed IEX, which has no true receive time). Records without a
//! receive time (status, system and error messages) take `ts_event`. For market
//! data `seq` is the source's rank, then the record's position in its source,
//! so records with equal times keep the order of the sources and of each file.
//!
//! Each source is already in time order, so the merge keeps one heap entry per
//! source and takes the record with the lowest key: one comparison per record,
//! never a queue insertion per record. A source that goes back in time is an
//! error, never reordered silently.
//!
//! `dbn`'s own merge decoder orders by index time only: it applies no latency
//! offset, and its ties depend on its heap, not on the sources' order. A run
//! must be the same every time, so ties here are broken by rank and position.

use dbn::decode::{DbnMetadata, DecodeRecordRef};
use dbn::flags::BAD_TS_RECV;
use dbn::{
    BboMsg, CbboMsg, Cmbp1Msg, MboMsg, Mbp1Msg, Mbp10Msg, Metadata, Record, RecordBuf, RecordRef,
    TradeMsg, UNDEF_TIMESTAMP,
};
use std::cmp::Reverse;
use std::collections::BinaryHeap;
use std::fmt;

/// What a delivery is, ordering deliveries with the same `ts`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Class {
    /// Market records, gaps and other source messages.
    Market = 1,
    /// Venue reports (fills, cancels, expiries).
    Report = 2,
    /// Timers.
    Timer = 3,
    /// Messages between actors.
    Actor = 4,
}

/// The order key of one delivery. Derived ordering compares the fields in
/// order: time, then class, then the source's rank, then the position.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Key {
    /// Index time, ns since the UNIX epoch.
    pub ts: u64,
    /// Class.
    pub class: Class,
    /// For market data, the source's rank in the run; otherwise 0.
    pub rank: u32,
    /// For market data, the record's position in its source (0-based);
    /// otherwise the order of creation.
    pub pos: u64,
}

/// The `flags` of a record, for the record types that carry them.
#[must_use]
pub fn record_flags(rec: &RecordRef<'_>) -> Option<u8> {
    macro_rules! try_flags {
        ($($t:ty),*) => {$(
            if let Some(r) = rec.get::<$t>() {
                return Some(r.flags.raw());
            }
        )*};
    }
    try_flags!(
        Cmbp1Msg, CbboMsg, BboMsg, Mbp1Msg, TradeMsg, MboMsg, Mbp10Msg
    );
    None
}

/// A record's index time: `ts_recv`, or `ts_event + offset` where the record
/// is flagged `F_BAD_TS_RECV`. `None` when the record has no usable time.
#[must_use]
pub fn index_ts(rec: &RecordRef<'_>, offset_ns: u64) -> Option<u64> {
    let bad = record_flags(rec).is_some_and(|f| f & BAD_TS_RECV != 0);
    let ts = if bad {
        rec.header().ts_event.checked_add(offset_ns)?
    } else {
        rec.raw_index_ts()
    };
    (ts != UNDEF_TIMESTAMP).then_some(ts)
}

/// Why a merge stopped.
#[derive(Debug)]
pub enum MergeError {
    /// A source couldn't be read or decoded.
    Source {
        /// The source's rank.
        rank: u32,
        /// What went wrong.
        error: String,
    },
    /// A source's record is earlier than the one before it.
    OutOfOrder {
        /// The source's rank.
        rank: u32,
        /// The record's position in its source.
        pos: u64,
        /// Its index time.
        ts: u64,
        /// The index time of the record before it.
        last: u64,
    },
    /// A record has no usable time (`UNDEF_TIMESTAMP`).
    Untimed {
        /// The source's rank.
        rank: u32,
        /// The record's position in its source.
        pos: u64,
    },
}

impl fmt::Display for MergeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Source { rank, error } => write!(f, "source {rank}: {error}"),
            Self::OutOfOrder {
                rank,
                pos,
                ts,
                last,
            } => write!(
                f,
                "source {rank}: record {pos} at {ts} is earlier than the record before it at {last}"
            ),
            Self::Untimed { rank, pos } => write!(f, "source {rank}: record {pos} has no time"),
        }
    }
}

impl std::error::Error for MergeError {}

/// A DBN source in time order: one decoder and its declared latency offset.
pub struct DbnSource<D> {
    decoder: D,
    offset_ns: u64,
    peeked: Option<(u64, RecordBuf)>,
    pos: u64,
    last: u64,
    done: bool,
}

impl<D: DecodeRecordRef> DbnSource<D> {
    /// A source over `decoder`. `offset_ns` is added to `ts_event` for records
    /// flagged `F_BAD_TS_RECV`; it is part of the run's manifest.
    pub fn new(decoder: D, offset_ns: u64) -> Self {
        Self {
            decoder,
            offset_ns,
            peeked: None,
            pos: 0,
            last: 0,
            done: false,
        }
    }

    /// The declared latency offset.
    pub fn offset_ns(&self) -> u64 {
        self.offset_ns
    }

    /// The number of records taken so far.
    pub fn position(&self) -> u64 {
        self.pos
    }

    /// The decoder, for its metadata.
    pub fn decoder(&self) -> &D {
        &self.decoder
    }

    fn peek(&mut self, rank: u32) -> Result<Option<u64>, MergeError> {
        if self.peeked.is_none() && !self.done {
            let rec = self
                .decoder
                .decode_record_ref()
                .map_err(|e| MergeError::Source {
                    rank,
                    error: e.to_string(),
                })?;
            match rec {
                None => self.done = true,
                Some(rec) => {
                    let pos = self.pos;
                    let ts =
                        index_ts(&rec, self.offset_ns).ok_or(MergeError::Untimed { rank, pos })?;
                    if ts < self.last {
                        return Err(MergeError::OutOfOrder {
                            rank,
                            pos,
                            ts,
                            last: self.last,
                        });
                    }
                    let buf = RecordBuf::try_from(rec).map_err(|e| MergeError::Source {
                        rank,
                        error: e.to_string(),
                    })?;
                    self.peeked = Some((ts, buf));
                }
            }
        }
        Ok(self.peeked.as_ref().map(|(ts, _)| *ts))
    }
}

impl<D: DecodeRecordRef + DbnMetadata> DbnSource<D> {
    /// The source's DBN metadata.
    pub fn metadata(&self) -> &Metadata {
        self.decoder.metadata()
    }
}

/// One merged delivery.
pub struct Delivery {
    /// Its order key (class `Market`).
    pub key: Key,
    /// The record, unchanged.
    pub record: RecordBuf,
}

impl Delivery {
    /// The record.
    pub fn record(&self) -> RecordRef<'_> {
        self.record.as_rec_ref()
    }
}

/// A lazy k-way merge of DBN sources. A source's rank is its index in the
/// list given to [`Merge::new`].
pub struct Merge<D> {
    sources: Vec<DbnSource<D>>,
    heap: BinaryHeap<Reverse<(u64, u32)>>,
    primed: bool,
}

impl<D: DecodeRecordRef> Merge<D> {
    /// A merge of `sources`, in rank order.
    pub fn new(sources: Vec<DbnSource<D>>) -> Self {
        Self {
            heap: BinaryHeap::with_capacity(sources.len()),
            sources,
            primed: false,
        }
    }

    /// The sources, in rank order.
    pub fn sources(&self) -> &[DbnSource<D>] {
        &self.sources
    }

    fn prime(&mut self) -> Result<(), MergeError> {
        if !self.primed {
            for rank in 0..self.sources.len() {
                self.push(rank)?;
            }
            self.primed = true;
        }
        Ok(())
    }

    fn push(&mut self, rank: usize) -> Result<(), MergeError> {
        let r = u32::try_from(rank).unwrap_or(u32::MAX);
        if let Some(ts) = self.sources[rank].peek(r)? {
            self.heap.push(Reverse((ts, r)));
        }
        Ok(())
    }

    /// The key of the next delivery, without taking it.
    ///
    /// # Errors
    /// A source that can't be read, goes back in time, or has an untimed record.
    pub fn peek_key(&mut self) -> Result<Option<Key>, MergeError> {
        self.prime()?;
        Ok(self.heap.peek().map(|Reverse((ts, rank))| Key {
            ts: *ts,
            class: Class::Market,
            rank: *rank,
            pos: self.sources[*rank as usize].pos,
        }))
    }

    /// The next delivery in key order, or `None` once every source is done.
    ///
    /// # Errors
    /// As [`Merge::peek_key`].
    pub fn next_delivery(&mut self) -> Result<Option<Delivery>, MergeError> {
        self.prime()?;
        let Some(Reverse((ts, rank))) = self.heap.pop() else {
            return Ok(None);
        };
        let src = &mut self.sources[rank as usize];
        let Some((_, record)) = src.peeked.take() else {
            return Ok(None);
        };
        let key = Key {
            ts,
            class: Class::Market,
            rank,
            pos: src.pos,
        };
        src.pos += 1;
        src.last = ts;
        self.push(rank as usize)?;
        Ok(Some(Delivery { key, record }))
    }
}

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
//! **Companion records stay with their record.** A record whose rtype `dbn`
//! doesn't know (tinytrader's extension records, such as
//! `tinytrader.trade-conditions/1`, which must follow its trade) or that has no
//! time takes the index time of the record before it in the same source and is
//! delivered right after it, never separated from it by another source's
//! record. A source whose first record is one has nothing to attach it to, and
//! fails.
//!
//! **A flagged source may step back.** `F_BAD_TS_RECV` marks records whose
//! receive time isn't real; their `ts_event` can be out of order by packet
//! reordering at the venue. Such a record's index time is clamped to the
//! source's last, so the file's order is kept, and [`DbnSource::clamped`]
//! counts how many were, for the run's record. Any other record going back in
//! time is an error.
//!
//! **Errors are sticky.** A merge that fails returns the error on every later
//! call, and a record already taken when a read-ahead fails is delivered first.
//!
//! `dbn`'s own merge decoder orders by index time only: it applies no latency
//! offset, and its ties depend on its heap, not on the sources' order. A run
//! must be the same every time, so ties here are broken by rank and position.

use dbn::decode::{DbnMetadata, DecodeRecordRef};
use dbn::flags::BAD_TS_RECV;
use dbn::{
    BboMsg, CbboMsg, Cmbp1Msg, MboMsg, Mbp1Msg, Mbp10Msg, Metadata, RType, Record, RecordBuf,
    RecordRef, TradeMsg, UNDEF_TIMESTAMP,
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
///
/// # Errors
/// A record of one of those types that is too short to hold it.
pub fn record_flags(rec: &RecordRef<'_>) -> Result<Option<u8>, String> {
    macro_rules! try_flags {
        ($($t:ty),*) => {$(
            if rec.has::<$t>() {
                return rec
                    .try_get::<$t>()
                    .map(|r| Some(r.flags.raw()))
                    .map_err(|e| e.to_string());
            }
        )*};
    }
    try_flags!(
        Cmbp1Msg, CbboMsg, BboMsg, Mbp1Msg, TradeMsg, MboMsg, Mbp10Msg
    );
    Ok(None)
}

/// How a record is timed in the merge.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Timing {
    /// At this index time.
    At(u64),
    /// At this index time, from `F_BAD_TS_RECV`'s `ts_event` plus the offset.
    Flagged(u64),
    /// With the record before it: an rtype `dbn` doesn't know (an extension
    /// record) or no time at all.
    Companion,
}

/// How a record is timed: its index time is `ts_recv`, or `ts_event +
/// offset` where the record is flagged `F_BAD_TS_RECV`.
///
/// # Errors
/// A record too short for its type, or an offset that overflows the time.
pub fn timing(rec: &RecordRef<'_>, offset_ns: u64) -> Result<Timing, String> {
    if RType::try_from(rec.header().rtype).is_err() {
        return Ok(Timing::Companion);
    }
    let flags = record_flags(rec)?;
    if flags.is_some_and(|f| f & BAD_TS_RECV != 0) {
        let ev = rec.header().ts_event;
        if ev == UNDEF_TIMESTAMP {
            return Ok(Timing::Companion);
        }
        return ev
            .checked_add(offset_ns)
            .map(Timing::Flagged)
            .ok_or_else(|| format!("ts_event {ev} plus the offset {offset_ns} overflows"));
    }
    let ts = rec.raw_index_ts();
    Ok(if ts == UNDEF_TIMESTAMP {
        Timing::Companion
    } else {
        Timing::At(ts)
    })
}

/// A record's index time, `None` for a companion record (see [`timing`]).
#[must_use]
pub fn index_ts(rec: &RecordRef<'_>, offset_ns: u64) -> Option<u64> {
    match timing(rec, offset_ns) {
        Ok(Timing::At(t) | Timing::Flagged(t)) => Some(t),
        _ => None,
    }
}

/// Why a merge stopped.
#[derive(Clone, Debug, PartialEq, Eq)]
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
    /// A source's first record is a companion: it has no time and nothing
    /// before it to take one from.
    Untimed {
        /// The source's rank.
        rank: u32,
        /// The record's position in its source.
        pos: u64,
    },
    /// A record's time can't be computed: a short record, or `ts_event` plus
    /// the source's offset overflows.
    BadRecord {
        /// The source's rank.
        rank: u32,
        /// The record's position in its source.
        pos: u64,
        /// Why.
        error: String,
    },
    /// The merge's own state is inconsistent: a bug, never data.
    Invariant(String),
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
            Self::Untimed { rank, pos } => write!(
                f,
                "source {rank}: record {pos} has no time and no record before it"
            ),
            Self::BadRecord { rank, pos, error } => {
                write!(f, "source {rank}: record {pos}: {error}")
            }
            Self::Invariant(e) => write!(f, "merge: {e}"),
        }
    }
}

impl std::error::Error for MergeError {}

/// A DBN source in time order: one decoder and its declared latency offset.
pub struct DbnSource<D> {
    decoder: D,
    offset_ns: u64,
    /// The next record: its index time, whether it is a companion, the record.
    peeked: Option<(u64, bool, RecordBuf)>,
    pos: u64,
    /// The index time of the last record taken (or peeked, for companions).
    last: Option<u64>,
    clamped: u64,
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
            last: None,
            clamped: 0,
            done: false,
        }
    }

    /// How many flagged records were clamped to the time before them.
    pub fn clamped(&self) -> u64 {
        self.clamped
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

    /// The next record's index time and whether it is a companion of the one
    /// before it, reading ahead as needed.
    fn peek(&mut self, rank: u32) -> Result<Option<(u64, bool)>, MergeError> {
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
                    let bad = |error| MergeError::BadRecord { rank, pos, error };
                    let (ts, companion) = match timing(&rec, self.offset_ns).map_err(bad)? {
                        Timing::Companion => {
                            (self.last.ok_or(MergeError::Untimed { rank, pos })?, true)
                        }
                        Timing::Flagged(ts) => match self.last {
                            Some(last) if ts < last => {
                                self.clamped += 1;
                                (last, false)
                            }
                            _ => (ts, false),
                        },
                        Timing::At(ts) => match self.last {
                            Some(last) if ts < last => {
                                return Err(MergeError::OutOfOrder {
                                    rank,
                                    pos,
                                    ts,
                                    last,
                                });
                            }
                            _ => (ts, false),
                        },
                    };
                    let buf = RecordBuf::try_from(rec).map_err(|e| MergeError::Source {
                        rank,
                        error: e.to_string(),
                    })?;
                    self.last = Some(ts);
                    self.peeked = Some((ts, companion, buf));
                }
            }
        }
        Ok(self.peeked.as_ref().map(|(ts, c, _)| (*ts, *c)))
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
    /// The source whose next record is a companion of the one just delivered.
    glued: Option<u32>,
    failed: Option<MergeError>,
}

impl<D: DecodeRecordRef> Merge<D> {
    /// A merge of `sources`, in rank order.
    pub fn new(sources: Vec<DbnSource<D>>) -> Self {
        Self {
            heap: BinaryHeap::with_capacity(sources.len()),
            sources,
            primed: false,
            glued: None,
            failed: None,
        }
    }

    /// The sources, in rank order.
    pub fn sources(&self) -> &[DbnSource<D>] {
        &self.sources
    }

    /// Remembers an error, so every later call returns it too.
    fn fail<T>(&mut self, e: MergeError) -> Result<T, MergeError> {
        self.failed = Some(e.clone());
        Err(e)
    }

    fn prime(&mut self) -> Result<(), MergeError> {
        if let Some(e) = &self.failed {
            return Err(e.clone());
        }
        if !self.primed {
            self.primed = true;
            for rank in 0..self.sources.len() {
                if let Err(e) = self.push(rank) {
                    return self.fail(e);
                }
            }
        }
        Ok(())
    }

    /// Reads ahead in source `rank`: a companion is glued to the record just
    /// taken; anything else goes on the heap.
    fn push(&mut self, rank: usize) -> Result<(), MergeError> {
        let r = u32::try_from(rank).unwrap_or(u32::MAX);
        match self.sources[rank].peek(r)? {
            Some((_, true)) => self.glued = Some(r),
            Some((ts, false)) => self.heap.push(Reverse((ts, r))),
            None => {}
        }
        Ok(())
    }

    /// The source and time of the next delivery.
    fn front(&self) -> Option<(u64, u32)> {
        match self.glued {
            Some(r) => self.sources[r as usize]
                .peeked
                .as_ref()
                .map(|(ts, _, _)| (*ts, r)),
            None => self.heap.peek().map(|Reverse(k)| *k),
        }
    }

    /// The key of the next delivery, without taking it.
    ///
    /// # Errors
    /// A source that can't be read, goes back in time, or has a bad record;
    /// the same error again on every later call.
    pub fn peek_key(&mut self) -> Result<Option<Key>, MergeError> {
        self.prime()?;
        Ok(self.front().map(|(ts, rank)| Key {
            ts,
            class: Class::Market,
            rank,
            pos: self.sources[rank as usize].pos,
        }))
    }

    /// The next delivery in key order, or `None` once every source is done.
    ///
    /// # Errors
    /// As [`Merge::peek_key`]. A record taken before a read-ahead fails is
    /// delivered first; the error comes on the next call.
    pub fn next_delivery(&mut self) -> Result<Option<Delivery>, MergeError> {
        self.prime()?;
        let (ts, rank) = match self.glued.take() {
            Some(r) => (0, r),
            None => match self.heap.pop() {
                Some(Reverse(k)) => k,
                None => return Ok(None),
            },
        };
        let src = &mut self.sources[rank as usize];
        let Some((peeked_ts, _, record)) = src.peeked.take() else {
            return self.fail(MergeError::Invariant(format!(
                "source {rank} was due but held no record"
            )));
        };
        debug_assert!(ts == 0 || ts == peeked_ts);
        let key = Key {
            ts: peeked_ts,
            class: Class::Market,
            rank,
            pos: src.pos,
        };
        src.pos += 1;
        if let Err(e) = self.push(rank as usize) {
            self.failed = Some(e);
        }
        Ok(Some(Delivery { key, record }))
    }
}

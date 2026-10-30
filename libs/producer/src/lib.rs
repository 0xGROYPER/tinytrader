//! The store producer (decision 0021, section 8): the held days of one
//! dataset, symbol and schema, as one DBN stream in time order.
//!
//! One input contract with two kinds of holdings behind it:
//!
//! - [`StoreApi`]: the store service, `GET /coverage` for what each day is held
//!   in, then `GET /files/{name}` streamed, never loaded whole.
//! - [`Folder`]: a local copy, either a finished `databento-jobs` cache job
//!   (read only once its `COMPLETE` marker exists) or a folder in the store's
//!   layout (`<Provider>/<DATASET>/<symbol>/<schema>/<day>.dbn.zst`).
//!
//! The output is the stored DBN, unchanged, with three kinds of records added
//! and said so:
//!
//! - before each day's records, a `SymbolMappingMsg` for each instrument its
//!   files' metadata maps (Databento's batch files carry no definitions), with
//!   `stype_out_symbol` the raw symbol, stamped at the day's start;
//! - a day that isn't held is a `SystemMsg` `store-gap day=… reason=…`, never a
//!   silent skip;
//! - a partial day is announced by a `SystemMsg` `store-partial day=…`.
//!
//! A day held in several files is merged by [`tinytrader_dbn::Merge`]. Each
//! file's SHA-256 is computed while it streams and checked where the holdings
//! state one; [`StoreProducer::pins`] lists what was read, for the run's
//! manifest.
mod folder;
mod hashing;
mod store;

pub use folder::Folder;
pub use store::{Retry, StoreApi};

use dbn::decode::{DbnMetadata, DecodeRecordRef, DynDecoder};
use dbn::{
    Metadata, RecordBuf, RecordRef, SType, Schema, SymbolMappingMsg, SystemMsg,
    VersionUpgradePolicy,
};
use hashing::Hashed;
use std::collections::{BTreeSet, VecDeque};
use std::io::{BufReader, Read};
use std::str::FromStr;
use tinytrader_dbn::{DbnSource, Merge};

/// What is read: one dataset, symbol and schema.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Query {
    /// The store's provider folder name: `databento`, `alpaca`.
    pub provider: String,
    /// `OPRA.PILLAR`, `ALPACA.SIP`, …
    pub dataset: String,
    /// The symbol level: `SPY.OPT`, `SPY`.
    pub symbol: String,
    /// `cbbo-1m`, `cmbp-1`, …
    pub schema: String,
}

/// One file of a held day.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileRef {
    /// Its name in the holdings: a store logical name, or a path in a folder.
    pub name: String,
    /// The SHA-256 the holdings state for it, if any.
    pub sha256: Option<String>,
}

/// What the holdings have for one day.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Day {
    /// Held in these files; `partial` when only some contracts or hours are.
    Held {
        /// The files, merged in this order on ties.
        files: Vec<FileRef>,
        /// Only part of the day is held.
        partial: bool,
    },
    /// Not held.
    Missing {
        /// Why, for the gap message.
        reason: String,
    },
}

/// Somewhere days are held.
pub trait Holdings: Send {
    /// Where, for the pins: `store http://…`, `job OPRA-…`, `folder /…`.
    fn describe(&self) -> String;
    /// What is held for each of `days` (`YYYY-MM-DD`), in the same order.
    ///
    /// # Errors
    /// The holdings can't be asked, or don't hold what `query` names.
    fn plan(&self, query: &Query, days: &[String]) -> Result<Vec<Day>, String>;
    /// Opens one file for streaming.
    ///
    /// # Errors
    /// It can't be opened.
    fn open(&self, file: &FileRef) -> Result<Box<dyn Read + Send>, String>;
}

/// One file read, for the manifest.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FilePin {
    /// Its name in the holdings.
    pub name: String,
    /// The SHA-256 of its bytes as read.
    pub sha256: String,
    /// Its size.
    pub bytes: u64,
    /// The holdings stated this hash and it matched.
    pub verified: bool,
}

/// One day of the stream, for the manifest.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DayPin {
    /// `YYYY-MM-DD`.
    pub day: String,
    /// The files read (empty for a missing day).
    pub files: Vec<FilePin>,
    /// Only part of the day was held.
    pub partial: bool,
    /// Why it wasn't held, for a gap.
    pub missing: Option<String>,
}

type Decoder = DynDecoder<'static, BufReader<Hashed>>;

enum State {
    Next,
    Records(Merge<Decoder>),
    Done,
}

/// The producer: a [`DecodeRecordRef`] over the held days, in day order.
pub struct StoreProducer {
    holdings: Box<dyn Holdings>,
    query: Query,
    plan: Vec<(String, Day)>,
    at: usize,
    metadata: Metadata,
    state: State,
    queued: VecDeque<RecordBuf>,
    cur: Option<RecordBuf>,
    reading: Vec<(FileRef, Hashed)>,
    pins: Vec<DayPin>,
}

/// Midnight UTC of `YYYY-MM-DD`, in ns.
fn day_ns(day: &str) -> Option<u64> {
    let mut it = day.splitn(3, '-').map(i64::from_str);
    let (y, m, d) = (it.next()?.ok()?, it.next()?.ok()?, it.next()?.ok()?);
    if !(1..=12).contains(&m) || !(1..=31).contains(&d) || day.len() != 10 {
        return None;
    }
    // Days from civil (Howard Hinnant).
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (m + if m > 2 { -3 } else { 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    u64::try_from(days).ok()?.checked_mul(86_400_000_000_000)
}

/// A `SystemMsg`, its text cut to what the record holds.
fn system(ts: u64, text: &str) -> RecordBuf {
    let mut t: String = text.chars().take(302).collect();
    loop {
        if let Ok(m) = SystemMsg::new(ts, None, &t) {
            return RecordBuf::from(m);
        }
        t.pop();
    }
}

fn fail(e: impl ToString) -> dbn::Error {
    dbn::Error::decode(e)
}

impl StoreProducer {
    /// Plans `days` (`YYYY-MM-DD`, in order) of `query` from `holdings`.
    ///
    /// # Errors
    /// The holdings can't be asked, the days aren't in order, or `query`
    /// names an unknown schema.
    pub fn new(holdings: Box<dyn Holdings>, query: Query, days: &[String]) -> Result<Self, String> {
        if days.windows(2).any(|w| w[0] >= w[1]) || days.iter().any(|d| day_ns(d).is_none()) {
            return Err("days must be YYYY-MM-DD, in order, each once".into());
        }
        let schema = Schema::from_str(&query.schema).map_err(|e| e.to_string())?;
        let plan = holdings.plan(&query, days)?;
        if plan.len() != days.len() {
            return Err(format!(
                "{}: answered for {} of {} days",
                holdings.describe(),
                plan.len(),
                days.len()
            ));
        }
        let start = days.first().and_then(|d| day_ns(d)).unwrap_or(0);
        let end = days
            .last()
            .and_then(|d| day_ns(d))
            .map(|t| t + 86_400_000_000_000);
        let mut metadata = Metadata::builder()
            .dataset(query.dataset.clone())
            .schema(Some(schema))
            .start(start)
            .stype_in(None)
            .stype_out(SType::InstrumentId)
            .symbols(vec![query.symbol.clone()])
            .build();
        metadata.end = end.and_then(std::num::NonZeroU64::new);
        Ok(Self {
            holdings,
            query,
            plan: days.iter().cloned().zip(plan).collect(),
            at: 0,
            metadata,
            state: State::Next,
            queued: VecDeque::new(),
            cur: None,
            reading: Vec::new(),
            pins: Vec::new(),
        })
    }

    /// What each day is held in, as planned.
    pub fn plan(&self) -> &[(String, Day)] {
        &self.plan
    }

    /// The days finished so far, with the files read and their hashes.
    pub fn pins(&self) -> &[DayPin] {
        &self.pins
    }

    /// Where the days come from.
    pub fn describe(&self) -> String {
        self.holdings.describe()
    }

    /// Starts the next day: its announcements are queued, its files opened.
    fn start_day(&mut self) -> Result<(), String> {
        let Some((day, plan)) = self.plan.get(self.at).cloned() else {
            self.state = State::Done;
            return Ok(());
        };
        self.at += 1;
        let ts = day_ns(&day).unwrap_or(0);
        match plan {
            Day::Missing { reason } => {
                self.queued
                    .push_back(system(ts, &format!("store-gap day={day} reason={reason}")));
                self.pins.push(DayPin {
                    day,
                    files: Vec::new(),
                    partial: false,
                    missing: Some(reason),
                });
                self.state = State::Next;
            }
            Day::Held { files, partial } => {
                if partial {
                    self.queued
                        .push_back(system(ts, &format!("store-partial day={day}")));
                }
                let mut sources = Vec::new();
                let mut mapped = BTreeSet::new();
                for f in &files {
                    let r = self.holdings.open(f)?;
                    let h = Hashed::new(r);
                    let dec = DynDecoder::inferred_with_buffer(
                        BufReader::new(h.clone()),
                        VersionUpgradePolicy::UpgradeToV3,
                    )
                    .map_err(|e| format!("{}: {e}", f.name))?;
                    let parent = dec.metadata().stype_in == Some(SType::Parent);
                    for m in &dec.metadata().mappings {
                        for iv in &m.intervals {
                            let Ok(iid) = iv.symbol.parse::<u32>() else {
                                continue;
                            };
                            if !mapped.insert((iid, m.raw_symbol.clone())) {
                                continue;
                            }
                            let to_ns = |d: time::Date| {
                                u64::try_from(i64::from(d.to_julian_day()) - 2_440_588)
                                    .map_or(0, |n| n * 86_400_000_000_000)
                            };
                            let input = if parent {
                                &self.query.symbol
                            } else {
                                &m.raw_symbol
                            };
                            let msg = SymbolMappingMsg::new(
                                iid,
                                ts,
                                if parent {
                                    SType::Parent
                                } else {
                                    SType::RawSymbol
                                },
                                input,
                                SType::RawSymbol,
                                &m.raw_symbol,
                                to_ns(iv.start_date),
                                to_ns(iv.end_date),
                            )
                            .map_err(|e| format!("{}: {e}", f.name))?;
                            self.queued.push_back(RecordBuf::from(msg));
                        }
                    }
                    sources.push(DbnSource::new(dec, 0));
                    self.reading.push((f.clone(), h));
                }
                self.pins.push(DayPin {
                    day,
                    files: Vec::new(),
                    partial,
                    missing: None,
                });
                self.state = State::Records(Merge::new(sources));
            }
        }
        Ok(())
    }

    /// Finishes the day being read: drains and checks each file's hash.
    fn end_day(&mut self) -> Result<(), String> {
        let reading = std::mem::take(&mut self.reading);
        let pin = self.pins.last_mut().ok_or("no day being read")?;
        for (f, h) in reading {
            let (sha256, bytes) = h.finish().map_err(|e| format!("{}: {e}", f.name))?;
            let verified = match &f.sha256 {
                Some(want) if !want.eq_ignore_ascii_case(&sha256) => {
                    return Err(format!(
                        "{}: SHA-256 {sha256}, but the holdings state {want}",
                        f.name
                    ));
                }
                Some(_) => true,
                None => false,
            };
            pin.files.push(FilePin {
                name: f.name,
                sha256,
                bytes,
                verified,
            });
        }
        self.state = State::Next;
        Ok(())
    }

    fn advance(&mut self) -> Result<Option<RecordBuf>, String> {
        loop {
            if let Some(r) = self.queued.pop_front() {
                return Ok(Some(r));
            }
            match &mut self.state {
                State::Done => return Ok(None),
                State::Next => self.start_day()?,
                State::Records(m) => match m.next_delivery().map_err(|e| e.to_string())? {
                    Some(d) => return Ok(Some(d.record)),
                    None => self.end_day()?,
                },
            }
        }
    }
}

impl DecodeRecordRef for StoreProducer {
    fn decode_record_ref(&mut self) -> dbn::Result<Option<RecordRef<'_>>> {
        self.cur = self.advance().map_err(fail)?;
        Ok(self.cur.as_ref().map(RecordBuf::as_rec_ref))
    }
}

impl DbnMetadata for StoreProducer {
    fn metadata(&self) -> &Metadata {
        &self.metadata
    }

    fn metadata_mut(&mut self) -> &mut Metadata {
        &mut self.metadata
    }
}

#[cfg(test)]
mod tests {
    use super::day_ns;

    #[test]
    fn days_are_utc_midnights() {
        assert_eq!(day_ns("1970-01-01"), Some(0));
        assert_eq!(day_ns("2026-10-27"), Some(1_793_059_200_000_000_000));
        assert_eq!(day_ns("2024-02-29"), Some(1_709_164_800_000_000_000));
        assert_eq!(day_ns("2026-1x-01"), None);
    }
}

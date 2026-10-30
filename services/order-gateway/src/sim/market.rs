//! Market-mode quote input: an ordinary DBN stream (a file, or a relay or replay TCP
//! stream), decoded with Databento's `dbn` crate (older versions upgraded to v3), or
//! records pushed in process by a session ([`Feed`]), each with the time the session's
//! merge gave it. Definitions, symbol mappings and the top of book of `cmbp-1`,
//! `tcbbo`, `cbbo-*`, `bbo-*` and `mbp-1` are applied; other record types are skipped.
use dbn::decode::{DbnDecoder, DecodeRecordRef};
use dbn::{
    BboMsg, CbboMsg, Cmbp1Msg, InstrumentDefMsg, Mbp1Msg, RecordBuf, RecordRef, SymbolMappingMsg,
    UNDEF_PRICE, VersionUpgradePolicy,
};
use sha2::{Digest, Sha256};
use std::io::{BufReader, Read};
use std::net::TcpStream;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, Sender, TryRecvError, channel};

/// One applied input record.
#[derive(Clone, Debug)]
pub(crate) enum Rec {
    Def {
        iid: u32,
        symbol: String,
    },
    Quote {
        iid: u32,
        bid: Option<i64>,
        ask: Option<i64>,
        bid_sz: u64,
        ask_sz: u64,
        /// From a `tcbbo` record: the quote before a trade, used as a book only for an
        /// instrument no other quote record has given one.
        trade: bool,
    },
}

enum Input {
    Decoder(Box<DbnDecoder<Box<dyn Read + Send>>>),
    Feed(Receiver<(u64, RecordBuf)>),
}

pub(crate) struct QuoteSource {
    input: Input,
    peeked: Option<(u64, Rec)>,
    pub(crate) pin: String,
}

/// Records pushed into a simulator by the process that clocks it (a session).
///
/// Each record carries its time in the session's order (decision 0009's index time,
/// with the source's latency offset applied). The session pushes every record up to a
/// time before it advances the simulator to that time, so an empty feed means nothing
/// more has happened yet: the simulator never waits on it.
#[derive(Clone)]
pub struct Feed(Sender<(u64, RecordBuf)>, Arc<AtomicU64>);

impl Feed {
    /// Pushes one record at `ts`.
    ///
    /// # Errors
    /// The simulator has gone, the record can't be copied, or `ts` is earlier than a
    /// record pushed before it.
    pub fn push(&self, ts: u64, rec: RecordRef<'_>) -> Result<(), String> {
        // Records come in the session's order (rule 1); one going back is a session bug.
        let last = self.1.fetch_max(ts, Ordering::SeqCst);
        if ts < last {
            return Err(format!("a record at {ts} after one at {last}"));
        }
        let buf = RecordBuf::try_from(rec).map_err(|e| e.to_string())?;
        self.0
            .send((ts, buf))
            .map_err(|_| "the simulator has stopped".to_owned())
    }
}

/// The book-relevant content of a record, if it has any.
fn decode(rec: &RecordRef<'_>) -> Option<(u64, Rec)> {
    let side = |p: i64| Some(p).filter(|p| *p != UNDEF_PRICE);
    let quote = |iid, bid, ask, bid_sz: u32, ask_sz: u32| Rec::Quote {
        iid,
        bid: side(bid),
        ask: side(ask),
        bid_sz: u64::from(bid_sz),
        ask_sz: u64::from(ask_sz),
        trade: false,
    };
    if let Some(d) = rec.get::<InstrumentDefMsg>() {
        Some((
            d.ts_recv,
            Rec::Def {
                iid: d.hd.instrument_id,
                symbol: d.raw_symbol().unwrap_or_default().replace(' ', ""),
            },
        ))
    } else if let Some(m) = rec.get::<SymbolMappingMsg>() {
        // Batch files carry mappings, not definitions; the output symbol is the raw one.
        Some((
            m.hd.ts_event,
            Rec::Def {
                iid: m.hd.instrument_id,
                symbol: m.stype_out_symbol().unwrap_or_default().replace(' ', ""),
            },
        ))
    } else if let Some(q) = rec.get::<Cmbp1Msg>() {
        // OPRA consolidated quotes, and tcbbo's quote before each trade, by receipt.
        let l = &q.levels[0];
        let mut r = quote(q.hd.instrument_id, l.bid_px, l.ask_px, l.bid_sz, l.ask_sz);
        if let Rec::Quote { trade, .. } = &mut r {
            *trade = q.hd.rtype == dbn::rtype::TCBBO;
        }
        Some((q.ts_recv, r))
    } else if let Some(q) = rec.get::<CbboMsg>() {
        let l = &q.levels[0];
        Some((
            q.ts_recv,
            quote(q.hd.instrument_id, l.bid_px, l.ask_px, l.bid_sz, l.ask_sz),
        ))
    } else if let Some(q) = rec.get::<BboMsg>() {
        let l = &q.levels[0];
        Some((
            q.ts_recv,
            quote(q.hd.instrument_id, l.bid_px, l.ask_px, l.bid_sz, l.ask_sz),
        ))
    } else if let Some(q) = rec.get::<Mbp1Msg>() {
        // IEX MBP-1 by event.
        let l = &q.levels[0];
        Some((
            q.hd.ts_event,
            quote(q.hd.instrument_id, l.bid_px, l.ask_px, l.bid_sz, l.ask_sz),
        ))
    } else {
        None
    }
}

impl QuoteSource {
    /// `tcp:HOST:PORT` connects to a DBN stream; `relay:HOST:PORT` joins a
    /// market data relay (`tinytrader.relay/1`, live from now); anything else
    /// is a DBN file path.
    pub(crate) fn open(spec: &str) -> Result<Self, String> {
        let (r, pin): (Box<dyn Read + Send>, String) = if let Some(addr) =
            spec.strip_prefix("relay:")
        {
            let mut s =
                TcpStream::connect(addr).map_err(|e| format!("--sim-quotes {spec}: {e}"))?;
            std::io::Write::write_all(
                &mut s,
                b"{\"protocol\":\"tinytrader.relay/1\",\"client\":\"sim-broker\"}\n",
            )
            .map_err(|e| format!("--sim-quotes {spec}: {e}"))?;
            (
                Box::new(BufReader::new(s)),
                crate::protocol::Obj::new().s("source", "relay").done(),
            )
        } else if let Some(addr) = spec.strip_prefix("tcp:") {
            let s = TcpStream::connect(addr).map_err(|e| format!("--sim-quotes {spec}: {e}"))?;
            (
                Box::new(BufReader::new(s)),
                crate::protocol::Obj::new().s("source", "dbn-stream").done(),
            )
        } else {
            let bytes = std::fs::read(spec).map_err(|e| format!("--sim-quotes {spec}: {e}"))?;
            let sha = crate::intent::hex(&Sha256::digest(&bytes));
            let pin = crate::protocol::Obj::new()
                .s("source", "dbn-file")
                .s("sha256", &sha)
                .done();
            (Box::new(std::io::Cursor::new(bytes)), pin)
        };
        let decoder = DbnDecoder::with_upgrade_policy(r, VersionUpgradePolicy::UpgradeToV3)
            .map_err(|e| format!("not a DBN stream: {e}"))?;
        Ok(Self {
            input: Input::Decoder(Box::new(decoder)),
            peeked: None,
            pin,
        })
    }

    /// A source fed in process; `pin` says what feeds it (the session's own pins).
    pub(crate) fn feed(pin: String) -> (Self, Feed) {
        let (tx, rx) = channel();
        let src = Self {
            input: Input::Feed(rx),
            peeked: None,
            pin,
        };
        (src, Feed(tx, Arc::new(AtomicU64::new(0))))
    }

    /// Timestamp of the next applicable record, reading ahead as needed. A feed with
    /// nothing pushed has no next record yet.
    pub(crate) fn peek_ts(&mut self) -> Result<Option<u64>, String> {
        while self.peeked.is_none() {
            match &mut self.input {
                Input::Decoder(d) => {
                    let Some(rec) = d
                        .decode_record_ref()
                        .map_err(|e| format!("quote source: {e}"))?
                    else {
                        return Ok(None);
                    };
                    self.peeked = decode(&rec);
                }
                Input::Feed(rx) => match rx.try_recv() {
                    Ok((ts, buf)) => {
                        self.peeked = decode(&buf.as_rec_ref()).map(|(_, r)| (ts, r));
                    }
                    Err(TryRecvError::Empty | TryRecvError::Disconnected) => return Ok(None),
                },
            }
        }
        Ok(self.peeked.as_ref().map(|(t, _)| *t))
    }

    pub(crate) fn next(&mut self) -> Option<(u64, Rec)> {
        self.peeked.take()
    }
}

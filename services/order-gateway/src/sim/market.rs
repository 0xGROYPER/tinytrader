//! Market-mode quote input: an ordinary DBN stream (a file, or a relay or replay TCP
//! stream), decoded with Databento's `dbn` crate (older versions upgraded to v3).
//! Definitions, `cmbp-1` and `mbp-1` are applied; other record types are skipped.
use dbn::decode::{DbnDecoder, DecodeRecordRef};
use dbn::{Cmbp1Msg, InstrumentDefMsg, Mbp1Msg, UNDEF_PRICE, VersionUpgradePolicy};
use sha2::{Digest, Sha256};
use std::io::{BufReader, Read};
use std::net::TcpStream;

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
    },
}

pub(crate) struct QuoteSource {
    decoder: DbnDecoder<Box<dyn Read + Send>>,
    peeked: Option<(u64, Rec)>,
    pub(crate) pin: String,
}

impl QuoteSource {
    /// `tcp:HOST:PORT` connects to a stream; anything else is a DBN file path.
    pub(crate) fn open(spec: &str) -> Result<Self, String> {
        let (r, pin): (Box<dyn Read + Send>, String) = if let Some(addr) = spec.strip_prefix("tcp:")
        {
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
            decoder,
            peeked: None,
            pin,
        })
    }

    /// Timestamp of the next applicable record, reading ahead as needed.
    pub(crate) fn peek_ts(&mut self) -> Result<Option<u64>, String> {
        let side = |p: i64| Some(p).filter(|p| *p != UNDEF_PRICE);
        while self.peeked.is_none() {
            let Some(rec) = self
                .decoder
                .decode_record_ref()
                .map_err(|e| format!("quote source: {e}"))?
            else {
                return Ok(None);
            };
            self.peeked = if let Some(d) = rec.get::<InstrumentDefMsg>() {
                Some((
                    d.ts_recv,
                    Rec::Def {
                        iid: d.hd.instrument_id,
                        symbol: d.raw_symbol().unwrap_or_default().replace(' ', ""),
                    },
                ))
            } else if let Some(q) = rec.get::<Cmbp1Msg>() {
                // OPRA consolidated quotes are timed by receipt.
                let l = &q.levels[0];
                Some((
                    q.ts_recv,
                    Rec::Quote {
                        iid: q.hd.instrument_id,
                        bid: side(l.bid_px),
                        ask: side(l.ask_px),
                        bid_sz: u64::from(l.bid_sz),
                        ask_sz: u64::from(l.ask_sz),
                    },
                ))
            } else if let Some(q) = rec.get::<Mbp1Msg>() {
                // IEX MBP-1 by event.
                let l = &q.levels[0];
                Some((
                    q.hd.ts_event,
                    Rec::Quote {
                        iid: q.hd.instrument_id,
                        bid: side(l.bid_px),
                        ask: side(l.ask_px),
                        bid_sz: u64::from(l.bid_sz),
                        ask_sz: u64::from(l.ask_sz),
                    },
                ))
            } else {
                None
            };
        }
        Ok(self.peeked.as_ref().map(|(t, _)| *t))
    }

    pub(crate) fn next(&mut self) -> Option<(u64, Rec)> {
        self.peeked.take()
    }
}

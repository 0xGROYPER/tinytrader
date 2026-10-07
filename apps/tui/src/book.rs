//! A live book from a relay: the TUI joins it as a client
//! (`tinytrader.relay/1`, named `tinytrader-tui`), keeps each instrument's
//! top of book from its quotes, the option contracts from their definitions,
//! and a tape of trades.

use std::collections::{HashMap, VecDeque};
use std::io::Write as _;
use std::net::{Shutdown, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use dbn::decode::DecodeRecordRef as _;
use dbn::record::{Cmbp1Msg, InstrumentDefMsg, Mbp1Msg};
use dbn::{RecordRefEnum, UNDEF_PRICE};

/// An option contract, from its definition.
#[derive(Clone, Debug, PartialEq)]
pub struct Contract {
    pub strike: f64,
    /// Expiration, ns since the epoch.
    pub expiry: u64,
    pub call: bool,
}

/// One side of a top of book, and when it last changed.
#[derive(Clone, Copy, Debug)]
pub struct Side {
    pub px: f64,
    pub sz: u32,
    pub changed: Instant,
    /// Whether the last change raised the price (or size, at the same price).
    pub up: bool,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Quote {
    pub bid: Option<Side>,
    pub ask: Option<Side>,
}

impl Quote {
    pub fn mid(&self) -> Option<f64> {
        Some((self.bid?.px + self.ask?.px) / 2.0)
    }
}

#[derive(Clone, Debug)]
pub struct Trade {
    pub at_ns: u64,
    pub name: String,
    pub px: f64,
    pub size: u32,
}

/// What one relay has told us.
#[derive(Default)]
pub struct Book {
    pub contracts: HashMap<u32, Contract>,
    pub names: HashMap<u32, String>,
    pub quotes: HashMap<u32, Quote>,
    pub tape: VecDeque<Trade>,
    pub status: String,
    pub records: u64,
}

const TAPE: usize = 200;

fn px(p: i64) -> Option<f64> {
    (p != UNDEF_PRICE && p != 0).then(|| p as f64 / 1e9)
}

fn side(old: Option<Side>, p: i64, sz: u32, now: Instant) -> Option<Side> {
    let price = px(p)?;
    Some(match old {
        Some(o) if (o.px - price).abs() < 1e-12 && o.sz == sz => o,
        Some(o) => Side {
            px: price,
            sz,
            changed: now,
            up: price > o.px || (price == o.px && sz > o.sz),
        },
        None => Side {
            px: price,
            sz,
            changed: now,
            up: true,
        },
    })
}

impl Book {
    /// A short name for an instrument: `775C` for an option, else its symbol.
    pub fn label(&self, id: u32) -> String {
        match self.contracts.get(&id) {
            Some(c) => format!(
                "{}{}",
                strike_text(c.strike),
                if c.call { 'C' } else { 'P' }
            ),
            None => self
                .names
                .get(&id)
                .cloned()
                .unwrap_or_else(|| id.to_string()),
        }
    }

    fn definition(&mut self, d: &InstrumentDefMsg) {
        let id = d.hd.instrument_id;
        if let Ok(sym) = d.raw_symbol() {
            self.names.insert(id, sym.trim().to_owned());
        }
        let class = d.instrument_class as u8;
        if (class == b'C' || class == b'P')
            && let Some(strike) = px(d.strike_price)
        {
            self.contracts.insert(
                id,
                Contract {
                    strike,
                    expiry: d.expiration,
                    call: class == b'C',
                },
            );
        }
    }

    fn top(&mut self, id: u32, bid: (i64, u32), ask: (i64, u32), trade: Option<(u64, i64, u32)>) {
        let now = Instant::now();
        let q = self.quotes.entry(id).or_default();
        q.bid = side(q.bid, bid.0, bid.1, now);
        q.ask = side(q.ask, ask.0, ask.1, now);
        if let Some((at_ns, p, size)) = trade
            && let Some(price) = px(p)
        {
            let name = self.label(id);
            self.tape.push_front(Trade {
                at_ns,
                name,
                px: price,
                size,
            });
            self.tape.truncate(TAPE);
        }
    }

    fn cmbp1(&mut self, m: &Cmbp1Msg) {
        let l = &m.levels[0];
        let trade = (m.action as u8 == b'T').then_some((m.hd.ts_event, m.price, m.size));
        self.top(
            m.hd.instrument_id,
            (l.bid_px, l.bid_sz),
            (l.ask_px, l.ask_sz),
            trade,
        );
    }

    fn mbp1(&mut self, m: &Mbp1Msg) {
        let l = &m.levels[0];
        let trade = (m.action as u8 == b'T').then_some((m.hd.ts_event, m.price, m.size));
        self.top(
            m.hd.instrument_id,
            (l.bid_px, l.bid_sz),
            (l.ask_px, l.ask_sz),
            trade,
        );
    }
}

/// `775`, or `772.5` when the strike has a fraction.
pub fn strike_text(s: f64) -> String {
    if (s - s.round()).abs() < 1e-9 {
        format!("{s:.0}")
    } else {
        format!("{s}")
    }
}

fn lock(m: &Mutex<Book>) -> std::sync::MutexGuard<'_, Book> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// A relay connection feeding a book until stopped; it reconnects after a
/// drop.
pub struct Feed {
    pub book: Arc<Mutex<Book>>,
    stop: Arc<AtomicBool>,
    stream: Arc<Mutex<Option<TcpStream>>>,
}

impl Feed {
    pub fn start(addr: String) -> Self {
        let book = Arc::new(Mutex::new(Book::default()));
        let stop = Arc::new(AtomicBool::new(false));
        let stream: Arc<Mutex<Option<TcpStream>>> = Arc::default();
        let (b, s, st) = (book.clone(), stop.clone(), stream.clone());
        std::thread::spawn(move || {
            while !s.load(Ordering::Relaxed) {
                lock(&b).status = format!("connecting to {addr}");
                if let Err(e) = read(&addr, &b, &s, &st) {
                    lock(&b).status = e;
                }
                if !s.load(Ordering::Relaxed) {
                    std::thread::sleep(Duration::from_secs(2));
                }
            }
        });
        Self { book, stop, stream }
    }
}

impl Drop for Feed {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(s) = self
            .stream
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take()
        {
            let _ = s.shutdown(Shutdown::Both);
        }
    }
}

fn read(
    addr: &str,
    book: &Mutex<Book>,
    stop: &AtomicBool,
    keep: &Mutex<Option<TcpStream>>,
) -> Result<(), String> {
    let mut s =
        TcpStream::connect(addr).map_err(|e| format!("cannot reach the relay at {addr}: {e}"))?;
    *keep.lock().unwrap_or_else(PoisonError::into_inner) =
        Some(s.try_clone().map_err(|e| e.to_string())?);
    s.write_all(b"{\"protocol\":\"tinytrader.relay/1\",\"client\":\"tinytrader-tui\"}\n")
        .map_err(|e| e.to_string())?;
    let mut decoder = dbn::decode::dbn::Decoder::new(std::io::BufReader::with_capacity(1 << 20, s))
        .map_err(|e| format!("not a relay stream: {e}"))?;
    lock(book).status = "joined".into();
    while !stop.load(Ordering::Relaxed) {
        let Some(rec) = decoder
            .decode_record_ref()
            .map_err(|e| format!("relay stream ended: {e}"))?
        else {
            return Err("the relay closed the connection".into());
        };
        // Records of kinds this view doesn't know (such as trade conditions) are skipped.
        let Ok(rec) = rec.as_enum() else { continue };
        let mut b = lock(book);
        b.records += 1;
        match rec {
            RecordRefEnum::InstrumentDef(d) => b.definition(d),
            RecordRefEnum::Cmbp1(m) => b.cmbp1(m),
            RecordRefEnum::Mbp1(m) => b.mbp1(m),
            RecordRefEnum::System(m) => {
                if let Ok(t) = m.msg() {
                    b.status = t.to_owned();
                }
            }
            RecordRefEnum::Error(m) => {
                if let Ok(t) = m.err() {
                    b.status = format!("relay error: {t}");
                }
            }
            _ => {}
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use dbn::record::{ConsolidatedBidAskPair, RecordHeader};
    use dbn::rtype;

    fn def(id: u32, strike: i64, class: u8) -> InstrumentDefMsg {
        InstrumentDefMsg {
            hd: RecordHeader::new::<InstrumentDefMsg>(rtype::INSTRUMENT_DEF, 0, id, 0),
            strike_price: strike,
            instrument_class: class as std::ffi::c_char,
            expiration: 1_791_403_200_000_000_000,
            ..Default::default()
        }
    }

    fn quote(id: u32, bid: i64, ask: i64, action: u8, price: i64) -> Cmbp1Msg {
        let mut m = Cmbp1Msg::default_for_schema(dbn::Schema::Cmbp1);
        m.hd = RecordHeader::new::<Cmbp1Msg>(rtype::CMBP_1, 0, id, 1);
        m.levels[0] = ConsolidatedBidAskPair {
            bid_px: bid,
            ask_px: ask,
            bid_sz: 10,
            ask_sz: 12,
            ..Default::default()
        };
        m.action = action as std::ffi::c_char;
        m.price = price;
        m.size = 3;
        m
    }

    #[test]
    fn keeps_contracts_quotes_and_trades() {
        let mut b = Book::default();
        b.definition(&def(7, 775_000_000_000, b'C'));
        b.definition(&def(8, 772_500_000_000, b'P'));
        assert_eq!(
            b.contracts[&7],
            Contract {
                strike: 775.0,
                expiry: 1_791_403_200_000_000_000,
                call: true
            }
        );
        assert_eq!(b.label(8), "772.5P");
        b.cmbp1(&quote(7, 2_300_000_000, 2_400_000_000, b'A', 0));
        let q = b.quotes[&7];
        assert_eq!((q.bid.unwrap().px, q.ask.unwrap().px), (2.3, 2.4));
        assert!((q.mid().unwrap() - 2.35).abs() < 1e-9);
        b.cmbp1(&quote(7, 2_200_000_000, 2_400_000_000, b'T', 2_400_000_000));
        let q = b.quotes[&7];
        assert!(!q.bid.unwrap().up, "the bid fell");
        assert_eq!(
            (b.tape[0].name.as_str(), b.tape[0].px, b.tape[0].size),
            ("775C", 2.4, 3)
        );
        // An undefined price clears that side.
        b.cmbp1(&quote(7, UNDEF_PRICE, 2_400_000_000, b'A', 0));
        assert!(b.quotes[&7].bid.is_none());
    }
}

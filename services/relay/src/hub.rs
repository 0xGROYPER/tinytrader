//! Fan-out from one provider connection to any number of local clients.
//!
//! The provider task publishes chunks of DBN records; each live client reads
//! them from a bounded broadcast queue. A client that falls behind is warned
//! at half the queue and disconnected when it overflows, so it never slows the
//! provider connection or other clients.
//!
//! The hub also keeps the latest reference records (instrument definitions
//! and statistics) and sends them to each client when it joins, followed by
//! a `ReplayCompleted` system record, so every client knows every instrument
//! without its own provider session. Market data itself is never cached.

use std::collections::BTreeMap;
use std::mem::{offset_of, size_of};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use databento::dbn::{
    Cmbp1Msg, ErrorCode, InstrumentDefMsg, Mbp1Msg, Metadata, RecordHeader, StatMsg, SystemCode,
    UNDEF_PRICE, UNDEF_TIMESTAMP, rtype,
};
use tinytrader_observe::{Events, Series};
use tokio::io::{AsyncBufReadExt as _, AsyncReadExt as _, AsyncWriteExt as _, BufReader};
use tokio::net::TcpStream;
use tokio::net::tcp::OwnedWriteHalf;
use tokio::sync::broadcast;

use crate::protocol::{self, Join};

/// Records as the provider sent them, in order.
pub type Chunk = Arc<[u8]>;

/// How long a client has to send its join line.
const JOIN_TIMEOUT: Duration = Duration::from_secs(5);
/// Longest join line accepted.
const JOIN_MAX: u64 = 4096;

/// What happened to a write to a client.
enum Sent {
    Ok,
    /// The client went away.
    Closed,
    /// The client stopped reading: the write didn't finish in time.
    Stalled,
}

/// The latest reference records, by instrument.
#[derive(Default)]
struct Reference {
    /// Definition bytes and expiration, by instrument id.
    definitions: BTreeMap<u32, (u64, Vec<u8>)>,
    /// Statistic bytes, by instrument id and statistic type.
    statistics: BTreeMap<(u32, u16), Vec<u8>>,
}

fn u16_at(b: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(b.get(at..at + 2)?.try_into().ok()?))
}
fn u32_at(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(at..at + 4)?.try_into().ok()?))
}
fn u64_at(b: &[u8], at: usize) -> Option<u64> {
    Some(u64::from_le_bytes(b.get(at..at + 8)?.try_into().ok()?))
}

impl Reference {
    /// Keep the reference records in a chunk of DBN records.
    fn absorb(&mut self, chunk: &[u8]) {
        let mut at = 0;
        while at < chunk.len() {
            let len = usize::from(chunk[at]) * 4;
            let Some(rec) = chunk.get(at..at + len).filter(|r| r.len() >= 16) else {
                return;
            };
            at += len;
            let Some(id) = u32_at(rec, offset_of!(RecordHeader, instrument_id)) else {
                continue;
            };
            match rec[1] {
                rtype::INSTRUMENT_DEF if len >= size_of::<InstrumentDefMsg>() => {
                    let expires = u64_at(rec, offset_of!(InstrumentDefMsg, expiration))
                        .unwrap_or(UNDEF_TIMESTAMP);
                    self.definitions.insert(id, (expires, rec.to_vec()));
                }
                rtype::STATISTICS if len >= size_of::<StatMsg>() => {
                    let Some(kind) = u16_at(rec, offset_of!(StatMsg, stat_type)) else {
                        continue;
                    };
                    // update_action 2 deletes a statistic.
                    if rec.get(offset_of!(StatMsg, update_action)) == Some(&2) {
                        self.statistics.remove(&(id, kind));
                    } else {
                        self.statistics.insert((id, kind), rec.to_vec());
                    }
                }
                _ => {}
            }
        }
    }

    /// Every current definition, then every statistic, dropping instruments
    /// that have expired.
    fn snapshot(&mut self, now: u64) -> Vec<u8> {
        self.definitions
            .retain(|_, (expires, _)| *expires == UNDEF_TIMESTAMP || *expires > now);
        let defs = &self.definitions;
        self.statistics.retain(|(id, _), _| defs.contains_key(id));
        let mut out = Vec::new();
        for (_, rec) in self.definitions.values() {
            out.extend_from_slice(rec);
        }
        for rec in self.statistics.values() {
            out.extend_from_slice(rec);
        }
        out
    }

    fn len(&self) -> usize {
        self.definitions.len() + self.statistics.len()
    }

    /// The instrument's symbol, from its definition.
    fn symbol(&self, id: u32) -> Option<String> {
        let (_, def) = self.definitions.get(&id)?;
        let at = offset_of!(InstrumentDefMsg, raw_symbol);
        let raw = def.get(at..at + 71)?;
        let end = raw.iter().position(|b| *b == 0).unwrap_or(raw.len());
        std::str::from_utf8(&raw[..end])
            .ok()
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Upstream {
    /// Not connected yet since start.
    Starting,
    Up,
    /// Lost at this time (ns since the epoch).
    Down(u64),
}

pub struct Hub {
    tx: broadcast::Sender<Chunk>,
    /// The same stream with trade conditions interleaved, for sources that
    /// have them; otherwise identical to `tx`.
    tx_cond: broadcast::Sender<Chunk>,
    capacity: usize,
    heartbeat: Duration,
    /// A write taking longer than this means the client has stopped reading.
    write_timeout: Duration,
    /// Header for live clients; `start` is set per client.
    metadata: Metadata,
    upstream: Mutex<Upstream>,
    /// Held while publishing, so a joining client's snapshot and its live
    /// queue meet with nothing missed or repeated.
    reference: Mutex<Reference>,
    pub events: Events,
    /// Trade prices of the most active instruments, by symbol, for charts.
    pub series: Series,
    pub clients: AtomicU64,
    pub records: AtomicU64,
    pub bytes: AtomicU64,
    pub reconnects: AtomicU64,
    pub slow_disconnects: AtomicU64,
    pub refused: AtomicU64,
    /// Provider rows that couldn't be represented or weren't for this relay.
    pub refused_rows: AtomicU64,
}

impl Hub {
    /// `capacity` chunks of up to 64 KiB each are queued per client.
    pub fn new(metadata: Metadata, capacity: usize, heartbeat: Duration) -> Self {
        let (tx, _) = broadcast::channel(capacity.max(2));
        let (tx_cond, _) = broadcast::channel(capacity.max(2));
        Self {
            tx,
            tx_cond,
            capacity: capacity.max(2),
            heartbeat,
            write_timeout: Duration::from_secs(10),
            metadata,
            upstream: Mutex::new(Upstream::Starting),
            reference: Mutex::new(Reference::default()),
            series: Series::new(900, 1000, 6),
            events: Events::default(),
            clients: AtomicU64::new(0),
            records: AtomicU64::new(0),
            bytes: AtomicU64::new(0),
            reconnects: AtomicU64::new(0),
            slow_disconnects: AtomicU64::new(0),
            refused: AtomicU64::new(0),
            refused_rows: AtomicU64::new(0),
        }
    }

    pub fn heartbeat(&self) -> Duration {
        self.heartbeat
    }

    /// The relay's stream header, the same for every client, with `start`
    /// set to where this client's stream begins.
    pub fn metadata(&self, start: u64) -> Metadata {
        let mut m = self.metadata.clone();
        m.start = start;
        m
    }

    #[cfg(test)]
    fn with_write_timeout(mut self, t: Duration) -> Self {
        self.write_timeout = t;
        self
    }

    async fn send(&self, out: &mut OwnedWriteHalf, bytes: &[u8]) -> Sent {
        match tokio::time::timeout(self.write_timeout, out.write_all(bytes)).await {
            Ok(Ok(())) => Sent::Ok,
            Ok(Err(_)) => Sent::Closed,
            Err(_) => Sent::Stalled,
        }
    }

    /// Disconnect a client that fell behind, telling it why if it can still
    /// take a record.
    async fn drop_slow(&self, out: &mut OwnedWriteHalf, why: &str) {
        self.slow_disconnects.fetch_add(1, Ordering::Relaxed);
        self.events
            .push(format!("disconnected a client: {why}"), why.to_owned());
        let e = protocol::error(ErrorCode::SkippedRecordsAfterSlowReading, why);
        let _ = tokio::time::timeout(Duration::from_secs(1), out.write_all(&e)).await;
        let _ = tokio::time::timeout(Duration::from_secs(1), out.shutdown()).await;
    }

    /// Send records to every live client, keeping any reference records.
    pub fn publish(&self, chunk: Vec<u8>, records: u64) {
        let chunk: Chunk = chunk.into();
        self.publish_pair(chunk.clone(), chunk, records);
    }

    /// Send records to live clients: `plain` to most, `with_conditions` to
    /// those that asked for trade conditions.
    pub fn publish_pair(&self, plain: Chunk, with_conditions: Chunk, records: u64) {
        self.records.fetch_add(records, Ordering::Relaxed);
        self.bytes.fetch_add(plain.len() as u64, Ordering::Relaxed);
        let mut reference = lock(&self.reference);
        reference.absorb(&plain);
        self.chart(&reference, &plain);
        // No receivers is fine: nobody is listening right now.
        let _ = self.tx.send(plain);
        let _ = self.tx_cond.send(with_conditions);
    }

    /// Record the price of every trade in a chunk under its symbol.
    fn chart(&self, reference: &Reference, chunk: &[u8]) {
        let mut at = 0;
        while at < chunk.len() {
            let len = usize::from(chunk[at]) * 4;
            let Some(rec) = chunk.get(at..at + len).filter(|r| r.len() >= 16) else {
                return;
            };
            at += len;
            let (action, price) = match rec[1] {
                rtype::MBP_1 => (offset_of!(Mbp1Msg, action), offset_of!(Mbp1Msg, price)),
                rtype::CMBP_1 => (offset_of!(Cmbp1Msg, action), offset_of!(Cmbp1Msg, price)),
                _ => continue,
            };
            if rec.get(action) != Some(&b'T') {
                continue;
            }
            let Some(px) = u64_at(rec, price)
                .map(|p| p as i64)
                .filter(|p| *p != UNDEF_PRICE)
            else {
                continue;
            };
            let id = u32_at(rec, offset_of!(RecordHeader, instrument_id)).unwrap_or(0);
            if let Some(symbol) = reference.symbol(id) {
                self.series.push(&symbol, px as f64 / 1e9);
            }
        }
    }

    fn broadcast(&self, bytes: Vec<u8>) {
        let chunk: Chunk = bytes.into();
        let _ = self.tx.send(chunk.clone());
        let _ = self.tx_cond.send(chunk);
    }

    /// Reference records held for joining clients.
    pub fn reference_len(&self) -> usize {
        lock(&self.reference).len()
    }

    pub fn upstream(&self) -> Upstream {
        *lock(&self.upstream)
    }

    /// The provider connection is up; tell clients how long it was down.
    pub fn upstream_up(&self) {
        let mut u = lock(&self.upstream);
        if let Upstream::Down(since) = *u {
            self.reconnects.fetch_add(1, Ordering::Relaxed);
            let text = format!("upstream-up gap_from={since} gap_to={}", protocol::now_ns());
            self.events.push(text.clone(), text.clone());
            self.broadcast(protocol::system_text(&text));
        }
        *u = Upstream::Up;
    }

    /// The provider connection was lost; tell clients once.
    pub fn upstream_down(&self) {
        let mut u = lock(&self.upstream);
        if matches!(*u, Upstream::Down(_)) {
            return;
        }
        let since = protocol::now_ns();
        *u = Upstream::Down(since);
        let text = format!("upstream-down since={since}");
        self.events.push(text.clone(), text.clone());
        self.broadcast(protocol::system_text(&text));
    }

    /// Refuse a client with an `ErrorMsg` and close.
    pub async fn refuse(&self, mut out: OwnedWriteHalf, code: ErrorCode, why: &str) {
        self.refused.fetch_add(1, Ordering::Relaxed);
        self.events
            .push(format!("refused: {why}"), format!("refused: {why}"));
        let _ = out.write_all(&protocol::error(code, why)).await;
        let _ = out.shutdown().await;
    }

    /// Send a client the reference records, then (when catching up) the
    /// history it asked for, then `ReplayCompleted`, then live records until
    /// it disconnects or falls behind. `start` goes in its stream header.
    ///
    /// Live records are buffered while history is sent, and market records
    /// from before `cut` (which history covers) are dropped from them.
    pub async fn serve(&self, mut out: OwnedWriteHalf, join: Serve) {
        let (mut rx, snapshot) = {
            let mut reference = lock(&self.reference);
            let rx = if join.conditions {
                self.tx_cond.subscribe()
            } else {
                self.tx.subscribe()
            };
            (rx, reference.snapshot(protocol::now_ns()))
        };
        let Ok(mut first) = protocol::header(&self.metadata(join.start)).await else {
            return;
        };
        first.extend(snapshot);
        if !matches!(self.send(&mut out, &first).await, Sent::Ok) {
            return;
        }
        let mut buffered: Vec<Chunk> = Vec::new();
        if let Some(mut history) = join.history {
            loop {
                tokio::select! {
                    page = history.recv() => match page {
                        Some(Ok(bytes)) => {
                            if !matches!(self.send(&mut out, &bytes).await, Sent::Ok) {
                                return;
                            }
                        }
                        Some(Err(e)) => {
                            let _ = out.write_all(&protocol::error(ErrorCode::InternalError, &e)).await;
                            return;
                        }
                        None => break,
                    },
                    got = rx.recv() => match got {
                        Ok(chunk) => buffered.push(chunk),
                        Err(broadcast::error::RecvError::Lagged(_)) => {
                            self.drop_slow(&mut out, "client fell behind while catching up").await;
                            return;
                        }
                        Err(broadcast::error::RecvError::Closed) => return,
                    },
                }
            }
        }
        let mut cut = join.cut;
        let mut tail = protocol::system_code(SystemCode::ReplayCompleted);
        if let Upstream::Down(since) = self.upstream() {
            tail.extend(protocol::system_text(&format!(
                "upstream-down since={since}"
            )));
        }
        for chunk in &buffered {
            tail.extend(after_cut(chunk, &mut cut));
        }
        if !matches!(self.send(&mut out, &tail).await, Sent::Ok) {
            return;
        }
        let mut last_write = Instant::now();
        let mut warned = false;
        let mut tick = tokio::time::interval(Duration::from_secs(1));
        loop {
            let bytes: Vec<u8> = tokio::select! {
                got = rx.recv() => match got {
                    Ok(chunk) => {
                        let mut bytes = after_cut(&chunk, &mut cut);
                        let queued = rx.len();
                        if !warned && queued > self.capacity / 2 {
                            warned = true;
                            bytes.extend(protocol::system_code(SystemCode::SlowReaderWarning));
                        } else if queued < self.capacity / 4 {
                            warned = false;
                        }
                        bytes
                    }
                    Err(broadcast::error::RecvError::Lagged(_)) => {
                        self.drop_slow(&mut out, "client fell behind its queue").await;
                        return;
                    }
                    Err(broadcast::error::RecvError::Closed) => return,
                },
                _ = tick.tick() => {
                    if last_write.elapsed() < self.heartbeat {
                        continue;
                    }
                    protocol::system_code(SystemCode::Heartbeat)
                }
            };
            if bytes.is_empty() {
                continue;
            }
            match self.send(&mut out, &bytes).await {
                Sent::Ok => last_write = Instant::now(),
                Sent::Closed => return,
                Sent::Stalled => {
                    self.drop_slow(&mut out, "client stopped reading").await;
                    return;
                }
            }
        }
    }
}

/// How a client is served.
pub struct Serve {
    /// For the stream header.
    pub start: u64,
    /// Interleave trade conditions.
    pub conditions: bool,
    /// History to send before live data, as chunks of records.
    pub history: Option<tokio::sync::mpsc::Receiver<Result<Vec<u8>, String>>>,
    /// Live market records before this time are dropped (history covers
    /// them). Zero keeps everything.
    pub cut: u64,
}

/// The records of a live chunk that history doesn't already cover: market
/// records (MBP-1 and trade conditions) timed before `cut` are dropped until
/// one at or after it is seen, after which `cut` is cleared.
fn after_cut(chunk: &[u8], cut: &mut u64) -> Vec<u8> {
    if *cut == 0 {
        return chunk.to_vec();
    }
    let mut out = Vec::with_capacity(chunk.len());
    let mut at = 0;
    while at < chunk.len() {
        let len = usize::from(chunk[at]) * 4;
        let Some(rec) = chunk.get(at..at + len).filter(|r| r.len() >= 16) else {
            break;
        };
        at += len;
        let market = rec[1] == rtype::MBP_1 || rec[1] == tinytrader_dbn::trade_conditions::RTYPE;
        let ts = u64_at(rec, 8).unwrap_or(0);
        if market && *cut != 0 {
            if ts < *cut {
                continue;
            }
            *cut = 0;
        }
        out.extend_from_slice(rec);
    }
    out
}

/// Read a client's join line. On failure the client has been refused.
pub async fn read_join(stream: TcpStream, hub: &Hub) -> Option<(Join, OwnedWriteHalf)> {
    let (read, write) = stream.into_split();
    let mut line = String::new();
    let mut reader = BufReader::new(read.take(JOIN_MAX));
    let got = tokio::time::timeout(JOIN_TIMEOUT, reader.read_line(&mut line)).await;
    let why = match got {
        Err(_) => "no join request within 5 seconds".to_owned(),
        Ok(Err(e)) => format!("cannot read join request: {e}"),
        Ok(Ok(_)) if !line.ends_with('\n') => "join request must be one line of JSON".to_owned(),
        Ok(Ok(_)) => match protocol::parse_join(&line) {
            Ok(join) => return Some((join, write)),
            Err(e) => e,
        },
    };
    hub.refuse(write, ErrorCode::InvalidSubscription, &why)
        .await;
    None
}

/// Counts a connected client for as long as it lives.
pub struct ClientGuard<'a>(&'a AtomicU64);

impl<'a> ClientGuard<'a> {
    pub fn new(counter: &'a AtomicU64) -> Self {
        counter.fetch_add(1, Ordering::Relaxed);
        Self(counter)
    }
}

impl Drop for ClientGuard<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::Relaxed);
    }
}

pub fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use super::*;
    use databento::dbn::decode::{AsyncDbnDecoder, DbnMetadata as _};
    use databento::dbn::{ErrorMsg, RecordRefEnum, SystemMsg};
    use tokio::net::TcpListener;

    fn metadata() -> Metadata {
        Metadata::builder()
            .dataset("TEST.DATA".to_owned())
            .schema(None)
            .start(0)
            .stype_in(None)
            .stype_out(databento::dbn::SType::InstrumentId)
            .build()
    }

    /// A hub on a local port that serves every client live.
    async fn serving(hub: Arc<Hub>) -> String {
        let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap().to_string();
        tokio::spawn(async move {
            loop {
                let (s, _) = l.accept().await.unwrap();
                let hub = hub.clone();
                tokio::spawn(async move {
                    if let Some((_, out)) = read_join(s, &hub).await {
                        hub.serve(
                            out,
                            Serve {
                                start: protocol::now_ns(),
                                conditions: false,
                                history: None,
                                cut: 0,
                            },
                        )
                        .await;
                    }
                });
            }
        });
        addr
    }

    async fn join(addr: &str, line: &str) -> AsyncDbnDecoder<TcpStream> {
        let mut s = TcpStream::connect(addr).await.unwrap();
        s.write_all(line.as_bytes()).await.unwrap();
        AsyncDbnDecoder::new(s).await.unwrap()
    }

    #[tokio::test]
    async fn forwards_records_and_reports_the_provider() {
        let hub = Arc::new(Hub::new(metadata(), 64, Duration::from_secs(1)));
        let addr = serving(hub.clone()).await;
        let mut dec = join(&addr, "{\"protocol\":\"tinytrader.relay/1\"}\n").await;
        assert_eq!(dec.metadata().dataset, "TEST.DATA");
        assert_eq!(dec.metadata().version, 3);
        tokio::time::sleep(Duration::from_millis(50)).await;

        let rec = protocol::system_text("from the provider");
        hub.publish(rec, 1);
        hub.upstream_down();
        hub.upstream_up();
        let mut texts = Vec::new();
        while texts.len() < 5 {
            let r = dec.decode_record_ref().await.unwrap().unwrap();
            if let RecordRefEnum::System(m) = r.as_enum().unwrap() {
                texts.push(m.msg().unwrap().to_owned());
            }
        }
        // Nothing cached yet, so ReplayCompleted comes straight away.
        texts.remove(0);
        assert_eq!(texts[0], "from the provider");
        assert!(texts[1].starts_with("upstream-down since="));
        assert!(texts[2].starts_with("upstream-up gap_from="));
        // Then a heartbeat after a second of silence.
        assert_eq!(texts[3], "");
        assert_eq!(hub.reconnects.load(Ordering::Relaxed), 1);
    }

    fn definition(id: u32, expiration: u64, strike: i64) -> Vec<u8> {
        let rec = InstrumentDefMsg {
            hd: RecordHeader::new::<InstrumentDefMsg>(rtype::INSTRUMENT_DEF, 1, id, 0),
            expiration,
            strike_price: strike,
            ..Default::default()
        };
        databento::dbn::RecordRef::from(&rec).as_ref().to_vec()
    }

    fn statistic(id: u32, kind: u16, quantity: i64) -> Vec<u8> {
        let rec = StatMsg {
            hd: RecordHeader::new::<StatMsg>(rtype::STATISTICS, 1, id, 0),
            stat_type: kind,
            quantity,
            update_action: 1,
            ..Default::default()
        };
        databento::dbn::RecordRef::from(&rec).as_ref().to_vec()
    }

    #[tokio::test]
    async fn sends_reference_records_to_joining_clients() {
        let hub = Arc::new(Hub::new(metadata(), 64, Duration::from_secs(10)));
        let addr = serving(hub.clone()).await;
        let mut chunk = definition(7, u64::MAX, 100);
        chunk.extend(definition(8, 1, 200)); // expired long ago
        chunk.extend(definition(7, u64::MAX, 150)); // replaces the first
        chunk.extend(statistic(7, 9, 1000));
        chunk.extend(statistic(8, 9, 5)); // for the expired instrument
        hub.publish(chunk, 5);

        let mut dec = join(&addr, "{\"protocol\":\"tinytrader.relay/1\"}\n").await;
        let mut seen = Vec::new();
        loop {
            let r = dec.decode_record_ref().await.unwrap().unwrap();
            match r.as_enum().unwrap() {
                RecordRefEnum::InstrumentDef(d) => {
                    seen.push(format!("def {} {}", d.hd.instrument_id, d.strike_price))
                }
                RecordRefEnum::Stat(m) => {
                    seen.push(format!("stat {} {}", m.hd.instrument_id, m.quantity))
                }
                RecordRefEnum::System(m) if m.code().ok() == Some(SystemCode::ReplayCompleted) => {
                    break;
                }
                _ => {}
            }
        }
        assert_eq!(seen, ["def 7 150", "stat 7 1000"]);
        assert_eq!(hub.reference_len(), 2);
    }

    #[test]
    fn charts_trades_by_symbol() {
        let hub = Hub::new(metadata(), 8, Duration::from_secs(10));
        let mut def = InstrumentDefMsg {
            hd: RecordHeader::new::<InstrumentDefMsg>(rtype::INSTRUMENT_DEF, 1, 7, 0),
            expiration: u64::MAX,
            ..Default::default()
        };
        def.raw_symbol = databento::dbn::record::str_to_c_chars("SPY 261016C00775000").unwrap();
        let trade = |action: u8, price: i64| {
            let m = Mbp1Msg {
                hd: RecordHeader::new::<Mbp1Msg>(rtype::MBP_1, 1, 7, 0),
                action: action as std::ffi::c_char,
                price,
                ..Default::default()
            };
            databento::dbn::RecordRef::from(&m).as_ref().to_vec()
        };
        let mut chunk = databento::dbn::RecordRef::from(&def).as_ref().to_vec();
        chunk.extend(trade(b'T', 1_250_000_000));
        chunk.extend(trade(b'A', 9_000_000_000)); // a quote, not charted
        hub.publish(chunk, 3);
        let recent = hub.series.snapshot();
        let points = recent["SPY 261016C00775000"].as_array().unwrap();
        assert_eq!(points.len(), 1);
        assert_eq!(points[0][1].as_f64(), Some(1.25));
    }

    #[tokio::test]
    async fn refuses_bad_joins() {
        let hub = Arc::new(Hub::new(metadata(), 64, Duration::from_secs(10)));
        let addr = serving(hub.clone()).await;
        let mut s = TcpStream::connect(&addr).await.unwrap();
        s.write_all(b"{\"protocol\":\"other/9\"}\n").await.unwrap();
        let mut buf = Vec::new();
        s.read_to_end(&mut buf).await.unwrap();
        assert_eq!(buf.len(), std::mem::size_of::<ErrorMsg>());
        assert_eq!(hub.refused.load(Ordering::Relaxed), 1);
    }

    #[tokio::test]
    async fn disconnects_a_client_that_falls_behind() {
        let hub = Arc::new(
            Hub::new(metadata(), 8, Duration::from_secs(10))
                .with_write_timeout(Duration::from_millis(200)),
        );
        let addr = serving(hub.clone()).await;
        // Connect but never read.
        let mut s = TcpStream::connect(&addr).await.unwrap();
        s.write_all(b"{\"protocol\":\"tinytrader.relay/1\"}\n")
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;
        let big = vec![0u8; 64 * 1024];
        for _ in 0..200 {
            hub.publish(big.clone(), 1);
            tokio::time::sleep(Duration::from_millis(10)).await;
            if hub.slow_disconnects.load(Ordering::Relaxed) > 0 {
                break;
            }
        }
        assert_eq!(hub.slow_disconnects.load(Ordering::Relaxed), 1);
        let _ = SystemMsg::heartbeat(0);
    }
}

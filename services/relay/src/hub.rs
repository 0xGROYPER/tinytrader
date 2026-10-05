//! Fan-out from one provider connection to any number of local clients.
//!
//! The provider task publishes chunks of DBN records; each live client reads
//! them from a bounded broadcast queue. A client that falls behind is warned
//! at half the queue and disconnected when it overflows, so it never slows the
//! provider connection or other clients.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use databento::dbn::{ErrorCode, Metadata, SystemCode};
use glacier_observe::Events;
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
    capacity: usize,
    heartbeat: Duration,
    /// A write taking longer than this means the client has stopped reading.
    write_timeout: Duration,
    /// Header for live clients; `start` is set per client.
    metadata: Metadata,
    upstream: Mutex<Upstream>,
    pub events: Events,
    pub clients: AtomicU64,
    pub records: AtomicU64,
    pub bytes: AtomicU64,
    pub reconnects: AtomicU64,
    pub slow_disconnects: AtomicU64,
    pub refused: AtomicU64,
}

impl Hub {
    /// `capacity` chunks of up to 64 KiB each are queued per client.
    pub fn new(metadata: Metadata, capacity: usize, heartbeat: Duration) -> Self {
        let (tx, _) = broadcast::channel(capacity.max(2));
        Self {
            tx,
            capacity: capacity.max(2),
            heartbeat,
            write_timeout: Duration::from_secs(10),
            metadata,
            upstream: Mutex::new(Upstream::Starting),
            events: Events::default(),
            clients: AtomicU64::new(0),
            records: AtomicU64::new(0),
            bytes: AtomicU64::new(0),
            reconnects: AtomicU64::new(0),
            slow_disconnects: AtomicU64::new(0),
            refused: AtomicU64::new(0),
        }
    }

    pub fn heartbeat(&self) -> Duration {
        self.heartbeat
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

    /// Send records to every live client.
    pub fn publish(&self, chunk: Vec<u8>, records: u64) {
        self.records.fetch_add(records, Ordering::Relaxed);
        self.bytes.fetch_add(chunk.len() as u64, Ordering::Relaxed);
        // No receivers is fine: nobody is listening right now.
        let _ = self.tx.send(chunk.into());
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
            let _ = self.tx.send(protocol::system_text(&text).into());
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
        let _ = self.tx.send(protocol::system_text(&text).into());
    }

    /// Refuse a client with an `ErrorMsg` and close.
    pub async fn refuse(&self, mut out: OwnedWriteHalf, code: ErrorCode, why: &str) {
        self.refused.fetch_add(1, Ordering::Relaxed);
        self.events
            .push(format!("refused: {why}"), format!("refused: {why}"));
        let _ = out.write_all(&protocol::error(code, why)).await;
        let _ = out.shutdown().await;
    }

    /// Stream live records to a client until it disconnects or falls behind.
    pub async fn serve_live(&self, mut out: OwnedWriteHalf) {
        let mut rx = self.tx.subscribe();
        let mut metadata = self.metadata.clone();
        metadata.start = protocol::now_ns();
        let Ok(mut first) = protocol::header(&metadata).await else {
            return;
        };
        if let Upstream::Down(since) = self.upstream() {
            first.extend(protocol::system_text(&format!(
                "upstream-down since={since}"
            )));
        }
        if !matches!(self.send(&mut out, &first).await, Sent::Ok) {
            return;
        }
        let mut last_write = Instant::now();
        let mut warned = false;
        let mut tick = tokio::time::interval(Duration::from_secs(1));
        loop {
            let bytes: Vec<u8> = tokio::select! {
                got = rx.recv() => match got {
                    Ok(chunk) => {
                        let mut bytes = chunk.to_vec();
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
                        hub.serve_live(out).await;
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
        let mut dec = join(&addr, "{\"protocol\":\"glacier.relay/1\"}\n").await;
        assert_eq!(dec.metadata().dataset, "TEST.DATA");
        assert_eq!(dec.metadata().version, 3);
        tokio::time::sleep(Duration::from_millis(50)).await;

        let rec = protocol::system_text("from the provider");
        hub.publish(rec, 1);
        hub.upstream_down();
        hub.upstream_up();
        let mut texts = Vec::new();
        while texts.len() < 4 {
            let r = dec.decode_record_ref().await.unwrap().unwrap();
            if let RecordRefEnum::System(m) = r.as_enum().unwrap() {
                texts.push(m.msg().unwrap().to_owned());
            }
        }
        assert_eq!(texts[0], "from the provider");
        assert!(texts[1].starts_with("upstream-down since="));
        assert!(texts[2].starts_with("upstream-up gap_from="));
        // Then a heartbeat after a second of silence.
        assert_eq!(texts[3], "");
        assert_eq!(hub.reconnects.load(Ordering::Relaxed), 1);
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
        s.write_all(b"{\"protocol\":\"glacier.relay/1\"}\n")
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

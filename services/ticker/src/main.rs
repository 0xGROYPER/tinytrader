//! Toy service for the quickstart: subscribes to Coinbase's public ticker
//! channel, appends every message to a JSON Lines file per product and UTC
//! day, and exposes `/health`, `/metrics` and `/recent`.
//!
//! It exists to exercise the platform (build, run, observe, alert) before the
//! real market-data services are rewritten here. It is not a recorder: files
//! are plain appends with no commit, recovery or gap accounting beyond what
//! the metrics report.
//!
//! Configuration (environment):
//! - `TICKER_PRODUCTS`: comma-separated product ids (default `BTC-USD,ETH-USD`)
//! - `TICKER_DATA_DIR`: output directory (default `./data`)
//! - `TICKER_LISTEN` (or `TINYTRADER_LISTEN`): address for the HTTP endpoints (default `127.0.0.1:9105`)
//! - `TICKER_FEED`: websocket URL (default Coinbase Exchange public feed)

use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::Router;
use futures_util::{SinkExt, StreamExt};
use tinytrader_observe::{Events, Observer, Series, State, unix_seconds};
use tokio_tungstenite::tungstenite::Message;

const DEFAULT_FEED: &str = "wss://ws-feed.exchange.coinbase.com";
#[derive(Default)]
struct Product {
    messages: u64,
    last_price: f64,
    last_at: Option<Instant>,
}

#[derive(Default)]
struct Shared {
    products: Mutex<HashMap<String, Product>>,
    connected: AtomicBool,
    reconnects: AtomicU64,
    bytes_written: AtomicU64,
    gaps: AtomicU64,
    last_gap: Mutex<Option<(f64, f64)>>,
    /// Recent messages, served at `/events` for the console's live tail.
    events: Events,
    /// Sampled prices per product, served at `/recent` for charts.
    series: Series,
}

#[tokio::main]
async fn main() {
    // Select the TLS crypto backend explicitly; feature unification alone is ambiguous.
    let _ = rustls::crypto::ring::default_provider().install_default();
    let products: Vec<String> = env("TICKER_PRODUCTS", "BTC-USD,ETH-USD")
        .split(',')
        .map(|p| p.trim().to_owned())
        .filter(|p| !p.is_empty())
        .collect();
    let data_dir = PathBuf::from(env("TICKER_DATA_DIR", "./data"));
    let listen = tinytrader_observe::listen_address("TICKER_LISTEN", "127.0.0.1:9105");
    let feed = env("TICKER_FEED", DEFAULT_FEED);

    let observer = Observer::new("ticker", env!("CARGO_PKG_VERSION"));
    let shared = Arc::new(Shared::default());
    {
        let mut map = lock(&shared.products);
        for p in &products {
            map.insert(p.clone(), Product::default());
        }
    }
    register_metrics(&observer, &shared);
    observer.describe(serde_json::json!({
        "role": format!("Streams Coinbase tickers for {} to JSON Lines files.", products.join(", ")),
        "upstream": [{ "id": "coinbase", "label": "coinbase", "sub": "public ticker feed", "kind": "flow", "via": "websocket" }],
        "streams": { "events": "/events", "series": "/recent" },
        "stores": [{ "id": "data", "label": "ticker-data", "sub": "JSON Lines files", "metric": "tinytrader_ticker_bytes_written_total" }],
        "panels": [{
            "heading": "Live data",
            "note": "Sampled once per second",
            "decimals": 2,
            "footer": [
                { "metric": "tinytrader_ticker_messages_total", "match": "product", "format": "count", "suffix": " messages" },
                { "metric": "tinytrader_ticker_last_message_age_seconds", "match": "product", "format": "age", "prefix": "last ", "suffix": " ago" }
            ]
        }]
    }));

    let app = Router::new()
        .with_state(shared.clone())
        .merge(observer.router())
        .merge(shared.events.router())
        .merge(shared.series.router());
    let listener = match tokio::net::TcpListener::bind(&listen).await {
        Ok(l) => l,
        Err(e) => {
            eprintln!("ticker: cannot listen on {listen}: {e}");
            std::process::exit(2);
        }
    };
    eprintln!("ticker: endpoints on http://{listen} (health, metrics, recent, events)");
    tokio::spawn(async move {
        if let Err(e) = axum::serve(listener, app).await {
            eprintln!("ticker: http server stopped: {e}");
        }
    });

    let stream = stream_forever(feed, products, data_dir, observer.clone(), shared);
    tokio::select! {
        () = stream => {}
        () = shutdown_signal() => {
            observer.set_state(State::Stopping, "shutting down");
            eprintln!("ticker: shutting down");
        }
    }
}

/// Connect, stream and reconnect with backoff until the process is stopped.
async fn stream_forever(
    feed: String,
    products: Vec<String>,
    dir: PathBuf,
    observer: Observer,
    shared: Arc<Shared>,
) {
    let mut sink = Sink::new(dir);
    let mut backoff = Duration::from_secs(1);
    let mut disconnected_at: Option<f64> = None;
    loop {
        observer.set_state(
            if disconnected_at.is_some() {
                State::Recovering
            } else {
                State::Starting
            },
            "connecting to feed",
        );
        match stream_once(
            &feed,
            &products,
            &mut sink,
            &observer,
            &shared,
            &mut disconnected_at,
        )
        .await
        {
            Ok(()) => observer.record_error("feed closed the connection"),
            Err(e) => {
                eprintln!("ticker: {e}");
                observer.record_error(&e);
            }
        }
        // A connection that streamed resets the backoff; repeated failures grow it.
        if shared.connected.swap(false, Ordering::Relaxed) {
            backoff = Duration::from_secs(1);
        }
        if disconnected_at.is_none() {
            disconnected_at = Some(unix_seconds());
        }
        observer.set_state(State::Degraded, "disconnected; reconnecting");
        tokio::time::sleep(backoff).await;
        backoff = (backoff * 2).min(Duration::from_secs(30));
        shared.reconnects.fetch_add(1, Ordering::Relaxed);
    }
}

async fn stream_once(
    feed: &str,
    products: &[String],
    sink: &mut Sink,
    observer: &Observer,
    shared: &Shared,
    disconnected_at: &mut Option<f64>,
) -> Result<(), String> {
    let (mut ws, _) = tokio_tungstenite::connect_async(feed)
        .await
        .map_err(|e| format!("connect failed: {e}"))?;
    let subscribe = serde_json::json!({
        "type": "subscribe",
        "product_ids": products,
        "channels": ["ticker"],
    });
    ws.send(Message::Text(subscribe.to_string().into()))
        .await
        .map_err(|e| format!("subscribe failed: {e}"))?;

    // A silent feed is treated as a dead connection.
    let idle = Duration::from_secs(30);
    loop {
        let next = tokio::time::timeout(idle, ws.next())
            .await
            .map_err(|_| "no messages for 30s".to_owned())?;
        let Some(msg) = next else { return Ok(()) };
        let msg = msg.map_err(|e| format!("read failed: {e}"))?;
        let text = match msg {
            Message::Text(t) => t,
            Message::Ping(p) => {
                let _ = ws.send(Message::Pong(p)).await;
                continue;
            }
            Message::Close(_) => return Ok(()),
            _ => continue,
        };
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) else {
            continue;
        };
        match v["type"].as_str() {
            Some("ticker") => {}
            Some("error") => {
                return Err(format!(
                    "feed error: {}",
                    v["message"].as_str().unwrap_or("unknown")
                ));
            }
            _ => continue,
        }
        let Some(product) = v["product_id"].as_str() else {
            continue;
        };
        let price = v["price"]
            .as_str()
            .and_then(|p| p.parse::<f64>().ok())
            .unwrap_or(f64::NAN);

        let now_s = unix_seconds();
        match sink.append(product, now_s, &text) {
            Ok(n) => {
                shared.bytes_written.fetch_add(n, Ordering::Relaxed);
            }
            Err(e) => {
                observer.record_error(&format!("write failed: {e}"));
                observer.set_state(State::Degraded, "cannot write to data directory");
                continue;
            }
        }
        if !shared.connected.swap(true, Ordering::Relaxed)
            && let Some(start) = disconnected_at.take()
        {
            shared.gaps.fetch_add(1, Ordering::Relaxed);
            *lock(&shared.last_gap) = Some((start, now_s));
        }
        observer.set_state(State::Ready, "streaming");

        let summary = format!(
            "{product} {} {} {}",
            v["side"].as_str().unwrap_or("-"),
            v["last_size"].as_str().unwrap_or("-"),
            v["price"].as_str().unwrap_or("-"),
        );
        shared.events.push(summary, text.to_string());

        let mut map = lock(&shared.products);
        let p = map.entry(product.to_owned()).or_default();
        p.messages += 1;
        p.last_at = Some(Instant::now());
        if price.is_finite() {
            p.last_price = price;
            shared.series.push(product, price);
        }
    }
}

fn register_metrics(observer: &Observer, shared: &Arc<Shared>) {
    let s = shared.clone();
    observer.set_metrics(move |w| {
        w.gauge(
            "tinytrader_ticker_connected",
            "1 while the feed connection is receiving messages.",
            &[],
            if s.connected.load(Ordering::Relaxed) {
                1.0
            } else {
                0.0
            },
        );
        w.counter(
            "tinytrader_ticker_reconnects_total",
            "Reconnect attempts since start.",
            &[],
            s.reconnects.load(Ordering::Relaxed) as f64,
        );
        w.counter(
            "tinytrader_ticker_gaps_total",
            "Intervals with no connection, counted when streaming resumes.",
            &[],
            s.gaps.load(Ordering::Relaxed) as f64,
        );
        if let Some((start, end)) = *lock(&s.last_gap) {
            w.gauge(
                "tinytrader_ticker_last_gap_start_seconds",
                "Start of the most recent gap.",
                &[],
                start,
            );
            w.gauge(
                "tinytrader_ticker_last_gap_end_seconds",
                "End of the most recent gap.",
                &[],
                end,
            );
        }
        w.counter(
            "tinytrader_ticker_bytes_written_total",
            "Bytes appended to data files.",
            &[],
            s.bytes_written.load(Ordering::Relaxed) as f64,
        );
        let map = lock(&s.products);
        let mut names: Vec<&String> = map.keys().collect();
        names.sort();
        for name in names {
            let p = &map[name];
            let labels = [("product", name.as_str())];
            w.counter(
                "tinytrader_ticker_messages_total",
                "Ticker messages received.",
                &labels,
                p.messages as f64,
            );
            w.gauge(
                "tinytrader_ticker_last_price",
                "Most recent trade price.",
                &labels,
                p.last_price,
            );
            if let Some(at) = p.last_at {
                w.gauge(
                    "tinytrader_ticker_last_message_age_seconds",
                    "Seconds since the last message for this product.",
                    &labels,
                    at.elapsed().as_secs_f64(),
                );
            }
        }
    });
}

/// Appends lines to `<dir>/<product>/<YYYY-MM-DD>.jsonl`, one open file per product.
struct Sink {
    dir: PathBuf,
    open: HashMap<String, (String, BufWriter<File>)>,
}

impl Sink {
    fn new(dir: PathBuf) -> Self {
        Self {
            dir,
            open: HashMap::new(),
        }
    }

    fn append(&mut self, product: &str, now_s: f64, line: &str) -> std::io::Result<u64> {
        let day = utc_date(now_s);
        let reopen = self.open.get(product).is_none_or(|(d, _)| *d != day);
        if reopen {
            let dir = self.dir.join(product);
            fs::create_dir_all(&dir)?;
            let file = OpenOptions::new()
                .create(true)
                .append(true)
                .open(dir.join(format!("{day}.jsonl")))?;
            if let Some((_, mut old)) = self
                .open
                .insert(product.to_owned(), (day, BufWriter::new(file)))
            {
                old.flush()?;
            }
        }
        let (_, w) = self.open.get_mut(product).expect("file opened above");
        w.write_all(line.as_bytes())?;
        w.write_all(b"\n")?;
        w.flush()?;
        Ok(line.len() as u64 + 1)
    }
}

/// `YYYY-MM-DD` in UTC for a Unix timestamp (civil-from-days).
fn utc_date(unix_s: f64) -> String {
    let days = (unix_s / 86_400.0).floor() as i64;
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}")
}

async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    #[cfg(unix)]
    let term = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut s) => {
                s.recv().await;
            }
            Err(_) => std::future::pending::<()>().await,
        }
    };
    #[cfg(not(unix))]
    let term = std::future::pending::<()>();
    tokio::select! {
        () = ctrl_c => {}
        () = term => {}
    }
}

fn env(key: &str, default: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.to_owned())
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use super::utc_date;

    #[test]
    fn utc_dates() {
        assert_eq!(utc_date(0.0), "1970-01-01");
        assert_eq!(utc_date(951_782_400.0), "2000-02-29");
        assert_eq!(utc_date(1_791_158_400.5), "2026-10-05");
    }
}

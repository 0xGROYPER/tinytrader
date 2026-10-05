//! Example service: streams Alpaca's real-time IEX trades and quotes, keeps a
//! live tail of them, and exposes `/health`, `/metrics` and `/events`.
//!
//! Alpaca's free market data plan includes the IEX feed. The protocol is
//! handled by the `apca` crate; this service only subscribes and reports.
//! It does not record data.
//!
//! Configuration (environment):
//! - `APCA_API_KEY_ID` and `APCA_API_SECRET_KEY`, or the same names with a
//!   `_FILE` suffix pointing at files containing them
//! - `ALPACA_SYMBOLS`: comma-separated (required)
//! - `ALPACA_CHANNELS`: any of `trades`, `quotes` (default `trades,quotes`)
//! - `ALPACA_LISTEN`: address for the HTTP endpoints (default `127.0.0.1:9102`)

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use apca::data::v2::stream::{Data, IEX, MarketData, RealtimeData, drive};
use apca::{ApiInfo, Client};
use axum::Router;
use futures_util::{FutureExt as _, StreamExt as _};
use glacier_observe::{Events, Observer, Series, State};

/// Alpaca's API base URL; market data streams are the same for paper and live accounts.
const BASE_URL: &str = "https://paper-api.alpaca.markets";

struct Config {
    key_id: String,
    secret: String,
    symbols: Vec<String>,
    trades: bool,
    quotes: bool,
}

#[derive(Default)]
struct Shared {
    connected: AtomicBool,
    reconnects: AtomicU64,
    records: Mutex<BTreeMap<&'static str, u64>>,
    last_record: Mutex<Option<Instant>>,
    events: Events,
    /// Trade prices per symbol, served at `/recent` for charts.
    series: Series,
}

#[tokio::main]
async fn main() {
    let config = match load_config() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("alpaca-live: {e}");
            std::process::exit(2);
        }
    };
    let listen = env("ALPACA_LISTEN", "127.0.0.1:9102");
    let observer = Observer::new("alpaca-live", env!("CARGO_PKG_VERSION"));
    let shared = Arc::new(Shared::default());
    register_metrics(&observer, &shared);
    observer.describe(serde_json::json!({
        "role": format!("Alpaca real-time IEX trades and quotes for {}.", config.symbols.join(", ")),
        "upstream": [{ "id": "alpaca", "label": "alpaca", "sub": "IEX real-time", "kind": "flow", "via": "websocket" }],
        "streams": { "events": "/events", "series": "/recent" },
        "panels": [{ "heading": "Live trades", "note": "IEX trade prices, sampled once per second", "decimals": 2 }]
    }));

    let app = Router::new()
        .merge(observer.router())
        .merge(shared.events.router())
        .merge(shared.series.router());
    let listener = match tokio::net::TcpListener::bind(&listen).await {
        Ok(l) => l,
        Err(e) => {
            eprintln!("alpaca-live: cannot listen on {listen}: {e}");
            std::process::exit(2);
        }
    };
    eprintln!(
        "alpaca-live: IEX {} on http://{listen}",
        config.symbols.join(",")
    );
    tokio::spawn(async move {
        if let Err(e) = axum::serve(listener, app).await {
            eprintln!("alpaca-live: http server stopped: {e}");
        }
    });

    tokio::select! {
        () = stream_forever(&config, &observer, &shared) => {}
        _ = tokio::signal::ctrl_c() => {}
        () = terminate() => {}
    }
    observer.set_state(State::Stopping, "shutting down");
}

/// Connect, stream and reconnect with backoff until the process is stopped.
async fn stream_forever(config: &Config, observer: &Observer, shared: &Shared) {
    let mut backoff = Duration::from_secs(1);
    loop {
        observer.set_state(State::Starting, "connecting to Alpaca");
        if let Err(e) = stream_once(config, observer, shared).await {
            eprintln!("alpaca-live: {e}");
            observer.record_error(&e);
        }
        if shared.connected.swap(false, Ordering::Relaxed) {
            backoff = Duration::from_secs(1);
        }
        observer.set_state(State::Degraded, "disconnected; reconnecting");
        tokio::time::sleep(backoff).await;
        backoff = (backoff * 2).min(Duration::from_secs(60));
        shared.reconnects.fetch_add(1, Ordering::Relaxed);
    }
}

async fn stream_once(config: &Config, observer: &Observer, shared: &Shared) -> Result<(), String> {
    let api_info = ApiInfo::from_parts(BASE_URL, &config.key_id, &config.secret)
        .map_err(|e| format!("invalid credentials: {e}"))?;
    let client = Client::new(api_info);
    let (mut stream, mut subscription) = client
        .subscribe::<RealtimeData<IEX>>()
        .await
        .map_err(|e| format!("connect failed: {e}"))?;

    let mut data = MarketData::default();
    if config.trades {
        data.set_trades(config.symbols.clone());
    }
    if config.quotes {
        data.set_quotes(config.symbols.clone());
    }
    let subscribe = subscription.subscribe(&data).boxed();
    drive(subscribe, &mut stream)
        .await
        .map_err(|_| "stream ended while subscribing".to_owned())?
        .map_err(|e| format!("subscribe failed: {e}"))?
        .map_err(|e| format!("subscribe rejected: {e}"))?;
    shared.connected.store(true, Ordering::Relaxed);
    observer.set_state(State::Ready, "subscribed; waiting for data");

    while let Some(message) = stream.next().await {
        let item = match message {
            Ok(Ok(item)) => item,
            Ok(Err(e)) => {
                observer.record_error(&format!("could not decode a message: {e}"));
                continue;
            }
            Err(e) => return Err(format!("websocket error: {e}")),
        };
        *lock(&shared.last_record) = Some(Instant::now());
        let kind = match &item {
            Data::Trade(t) => {
                shared.events.push(
                    format!("{} {} @ {}", t.symbol, t.trade_size, t.trade_price),
                    format!("{t:?}"),
                );
                if let Ok(price) = t.trade_price.to_string().parse::<f64>() {
                    shared.series.push(&t.symbol, price);
                }
                "trade"
            }
            Data::Quote(q) => {
                shared.events.push(
                    format!(
                        "{} {} × {} / {} × {}",
                        q.symbol, q.bid_size, q.bid_price, q.ask_price, q.ask_size
                    ),
                    format!("{q:?}"),
                );
                "quote"
            }
            Data::Bar(_) => "bar",
            _ => "other",
        };
        observer.set_state(State::Ready, "streaming");
        *lock(&shared.records).entry(kind).or_default() += 1;
    }
    Err("stream closed".into())
}

fn register_metrics(observer: &Observer, shared: &Arc<Shared>) {
    let s = shared.clone();
    observer.set_metrics(move |w| {
        let base = [("venue", "alpaca"), ("feed", "iex")];
        w.gauge(
            "glacier_alpaca_connected",
            "1 while the stream is subscribed.",
            &base,
            if s.connected.load(Ordering::Relaxed) {
                1.0
            } else {
                0.0
            },
        );
        w.counter(
            "glacier_alpaca_reconnects_total",
            "Reconnect attempts since start.",
            &base,
            s.reconnects.load(Ordering::Relaxed) as f64,
        );
        for (kind, n) in lock(&s.records).iter() {
            w.counter(
                "glacier_alpaca_records_total",
                "Records received, by kind.",
                &[base[0], base[1], ("kind", kind)],
                *n as f64,
            );
        }
        if let Some(at) = *lock(&s.last_record) {
            w.gauge(
                "glacier_alpaca_last_record_age_seconds",
                "Seconds since the last record.",
                &base,
                at.elapsed().as_secs_f64(),
            );
        }
    });
}

fn load_config() -> Result<Config, String> {
    let symbols: Vec<String> = std::env::var("ALPACA_SYMBOLS")
        .map_err(|_| "set ALPACA_SYMBOLS".to_owned())?
        .split(',')
        .map(|s| s.trim().to_uppercase())
        .filter(|s| !s.is_empty())
        .collect();
    if symbols.is_empty() {
        return Err("ALPACA_SYMBOLS is empty".into());
    }
    let channels = env("ALPACA_CHANNELS", "trades,quotes");
    let trades = channels.split(',').any(|c| c.trim() == "trades");
    let quotes = channels.split(',').any(|c| c.trim() == "quotes");
    if !trades && !quotes {
        return Err("ALPACA_CHANNELS must include trades or quotes".into());
    }
    Ok(Config {
        key_id: secret("APCA_API_KEY_ID")?,
        secret: secret("APCA_API_SECRET_KEY")?,
        symbols,
        trades,
        quotes,
    })
}

/// A credential from `NAME`, or from the file named by `NAME_FILE`.
fn secret(name: &str) -> Result<String, String> {
    if let Ok(path) = std::env::var(format!("{name}_FILE")) {
        return std::fs::read_to_string(&path)
            .map(|s| s.trim().to_owned())
            .map_err(|e| format!("cannot read {name}_FILE {path}: {e}"));
    }
    std::env::var(name).map_err(|_| format!("set {name} or {name}_FILE"))
}

async fn terminate() {
    #[cfg(unix)]
    if let Ok(mut s) = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
        s.recv().await;
        return;
    }
    std::future::pending::<()>().await;
}

fn env(key: &str, default: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.to_owned())
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

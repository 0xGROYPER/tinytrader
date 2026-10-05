//! Example service: subscribes to a Databento live dataset, keeps a live tail
//! of the records it receives, and exposes `/health`, `/metrics` and `/events`.
//!
//! Like the quickstart's ticker, it exists to exercise the platform with a
//! real feed. It does not record data.
//!
//! Configuration (environment):
//! - `DATABENTO_API_KEY`, or `DATABENTO_API_KEY_FILE` pointing at a file
//!   containing the key (for Docker secrets and similar)
//! - `DATABENTO_DATASET`: for example `OPRA.PILLAR` (required)
//! - `DATABENTO_SYMBOLS`: comma-separated (required)
//! - `DATABENTO_SCHEMA`: default `trades`
//! - `DATABENTO_STYPE_IN`: default `parent`
//! - `DATABENTO_LISTEN`: address for the HTTP endpoints (default `127.0.0.1:9101`)
//!
//! Live data can be billed by usage. Check what a subscription costs on your
//! plan before running this against a new dataset or schema.

use std::collections::BTreeMap;
use std::str::FromStr;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::Router;
use databento::LiveClient;
use databento::dbn::{
    ErrorMsg, PitSymbolMap, SType, Schema, SymbolMappingMsg, SystemMsg, TradeMsg, UNDEF_PRICE,
};
use databento::live::Subscription;
use glacier_observe::{Events, Observer, Series, State};

struct Config {
    key: String,
    dataset: String,
    symbols: Vec<String>,
    schema: Schema,
    stype_in: SType,
}

#[derive(Default)]
struct Shared {
    connected: AtomicBool,
    reconnects: AtomicU64,
    records: Mutex<BTreeMap<&'static str, u64>>,
    last_record: Mutex<Option<Instant>>,
    events: Events,
    /// Trade prices for the most traded instruments, served at `/recent`.
    series: Series,
}

#[tokio::main]
async fn main() {
    let config = match load_config() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("databento-live: {e}");
            std::process::exit(2);
        }
    };
    let listen = env("DATABENTO_LISTEN", "127.0.0.1:9101");
    let observer = Observer::new("databento-live", env!("CARGO_PKG_VERSION"));
    let shared = Arc::new(Shared {
        // Options feeds have thousands of instruments; chart the six most traded.
        series: Series::new(900, 1000, 6),
        ..Shared::default()
    });
    register_metrics(&observer, &shared, &config);
    observer.describe(serde_json::json!({
        "role": format!(
            "Databento live subscription: {} {} for {}.",
            config.dataset,
            config.schema,
            config.symbols.join(", ")
        ),
        "upstream": [{ "id": "databento", "label": "databento", "sub": config.dataset, "kind": "flow", "via": "live session" }],
        "streams": { "events": "/events", "series": "/recent" },
        "panels": [{ "heading": "Live trades", "note": "Most traded instruments, sampled once per second", "decimals": 2 }]
    }));

    let app = Router::new()
        .merge(observer.router())
        .merge(shared.events.router())
        .merge(shared.series.router());
    let listener = match tokio::net::TcpListener::bind(&listen).await {
        Ok(l) => l,
        Err(e) => {
            eprintln!("databento-live: cannot listen on {listen}: {e}");
            std::process::exit(2);
        }
    };
    eprintln!(
        "databento-live: {} {} {} on http://{listen}",
        config.dataset,
        config.schema,
        config.symbols.join(",")
    );
    tokio::spawn(async move {
        if let Err(e) = axum::serve(listener, app).await {
            eprintln!("databento-live: http server stopped: {e}");
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
        observer.set_state(State::Starting, "connecting to Databento");
        if let Err(e) = stream_once(config, observer, shared).await {
            eprintln!("databento-live: {e}");
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
    let mut client = LiveClient::builder()
        .key(&config.key)
        .map_err(|e| format!("invalid API key: {e}"))?
        .dataset(&config.dataset)
        .build()
        .await
        .map_err(|e| format!("connect failed: {e}"))?;
    client
        .subscribe(
            Subscription::builder()
                .symbols(config.symbols.clone())
                .schema(config.schema)
                .stype_in(config.stype_in)
                .build(),
        )
        .await
        .map_err(|e| format!("subscribe failed: {e}"))?;
    client
        .start()
        .await
        .map_err(|e| format!("start failed: {e}"))?;
    shared.connected.store(true, Ordering::Relaxed);
    observer.set_state(State::Ready, "subscribed; waiting for records");

    let mut symbols = PitSymbolMap::new();
    while let Some(rec) = client
        .next_record()
        .await
        .map_err(|e| format!("read failed: {e}"))?
    {
        *lock(&shared.last_record) = Some(Instant::now());
        let _ = symbols.on_record(rec);
        let kind = if let Some(trade) = rec.get::<TradeMsg>() {
            let symbol = symbols
                .get(trade.hd.instrument_id)
                .map_or_else(|| trade.hd.instrument_id.to_string(), Clone::clone);
            shared
                .events
                .push(trade_summary(&symbol, trade), format!("{trade:?}"));
            if trade.price != UNDEF_PRICE {
                shared.series.push(symbol.trim(), trade.price as f64 / 1e9);
            }
            observer.set_state(State::Ready, "streaming");
            "trade"
        } else if let Some(m) = rec.get::<SymbolMappingMsg>() {
            shared.events.push(
                format!("mapping {}", m.stype_out_symbol().unwrap_or("?")),
                format!("{m:?}"),
            );
            "symbol_mapping"
        } else if let Some(m) = rec.get::<SystemMsg>() {
            if m.is_heartbeat() {
                "heartbeat"
            } else {
                shared.events.push(
                    format!("system: {}", m.msg().unwrap_or("?")),
                    format!("{m:?}"),
                );
                "system"
            }
        } else if let Some(m) = rec.get::<ErrorMsg>() {
            let text = m.err().unwrap_or("unknown error").to_owned();
            observer.record_error(&format!("gateway: {text}"));
            shared
                .events
                .push(format!("error: {text}"), format!("{m:?}"));
            "error"
        } else {
            "other"
        };
        *lock(&shared.records).entry(kind).or_default() += 1;
    }
    Err("feed closed the connection".into())
}

fn trade_summary(symbol: &str, t: &TradeMsg) -> String {
    let side = match t.side as u8 {
        b'B' => "buy",
        b'A' => "sell",
        _ => "-",
    };
    let price = if t.price == UNDEF_PRICE {
        "-".to_owned()
    } else {
        format!("{:.2}", t.price as f64 / 1e9)
    };
    format!("{symbol} {side} {} @ {price}", t.size)
}

fn register_metrics(observer: &Observer, shared: &Arc<Shared>, config: &Config) {
    let s = shared.clone();
    let dataset = config.dataset.clone();
    let schema = config.schema.to_string();
    observer.set_metrics(move |w| {
        let base = [("dataset", dataset.as_str()), ("schema", schema.as_str())];
        w.gauge(
            "glacier_databento_connected",
            "1 while the live session is open.",
            &base,
            if s.connected.load(Ordering::Relaxed) {
                1.0
            } else {
                0.0
            },
        );
        w.counter(
            "glacier_databento_reconnects_total",
            "Reconnect attempts since start.",
            &base,
            s.reconnects.load(Ordering::Relaxed) as f64,
        );
        for (kind, n) in lock(&s.records).iter() {
            w.counter(
                "glacier_databento_records_total",
                "Records received, by kind.",
                &[base[0], base[1], ("kind", kind)],
                *n as f64,
            );
        }
        if let Some(at) = *lock(&s.last_record) {
            w.gauge(
                "glacier_databento_last_record_age_seconds",
                "Seconds since the last record of any kind, heartbeats included.",
                &base,
                at.elapsed().as_secs_f64(),
            );
        }
    });
}

fn load_config() -> Result<Config, String> {
    let key = match std::env::var("DATABENTO_API_KEY_FILE") {
        Ok(path) => std::fs::read_to_string(&path)
            .map_err(|e| format!("cannot read DATABENTO_API_KEY_FILE {path}: {e}"))?
            .trim()
            .to_owned(),
        Err(_) => std::env::var("DATABENTO_API_KEY")
            .map_err(|_| "set DATABENTO_API_KEY or DATABENTO_API_KEY_FILE".to_owned())?,
    };
    let dataset =
        std::env::var("DATABENTO_DATASET").map_err(|_| "set DATABENTO_DATASET".to_owned())?;
    let symbols: Vec<String> = std::env::var("DATABENTO_SYMBOLS")
        .map_err(|_| "set DATABENTO_SYMBOLS".to_owned())?
        .split(',')
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
        .collect();
    if symbols.is_empty() {
        return Err("DATABENTO_SYMBOLS is empty".into());
    }
    let schema = Schema::from_str(&env("DATABENTO_SCHEMA", "trades"))
        .map_err(|e| format!("invalid DATABENTO_SCHEMA: {e}"))?;
    let stype_in = SType::from_str(&env("DATABENTO_STYPE_IN", "parent"))
        .map_err(|e| format!("invalid DATABENTO_STYPE_IN: {e}"))?;
    Ok(Config {
        key,
        dataset,
        symbols,
        schema,
        stype_in,
    })
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summarizes_trades() {
        let mut t = TradeMsg {
            price: 7_740_000_000,
            size: 3,
            side: b'B' as _,
            ..Default::default()
        };
        assert_eq!(
            trade_summary("SPY 260918C00565000", &t),
            "SPY 260918C00565000 buy 3 @ 7.74"
        );
        t.price = UNDEF_PRICE;
        assert_eq!(trade_summary("X", &t), "X buy 3 @ -");
    }
}

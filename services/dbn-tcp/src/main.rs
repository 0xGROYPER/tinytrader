//! Example service: connects to a TCP address that serves a live DBN stream
//! (a metadata header followed by records), keeps a live tail and price
//! series, and exposes `/health`, `/metrics`, `/describe`, `/events` and
//! `/recent`. Decoding is done by the official `dbn` crate.
//!
//! Symbols come from instrument definition and symbol mapping records.
//! Trades are `trades` records, or `mbp-1` records with action `T`; other
//! `mbp-1` records are shown as quotes.
//!
//! Configuration (environment):
//! - `DBN_TCP_ADDR`: `host:port` serving the stream (required)
//! - `DBN_TCP_SOURCE`: name of the source in the console graph (default: the address)
//! - `DBN_TCP_LISTEN`: address for the HTTP endpoints (default `127.0.0.1:9103`)

use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::Router;
use dbn::decode::AsyncDbnDecoder;
use dbn::{
    ErrorMsg, InstrumentDefMsg, Mbp1Msg, SymbolMappingMsg, SystemMsg, TradeMsg, UNDEF_PRICE,
};
use glacier_observe::{Events, Observer, Series, State};

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
    let Ok(addr) = std::env::var("DBN_TCP_ADDR") else {
        eprintln!("dbn-tcp: set DBN_TCP_ADDR");
        std::process::exit(2);
    };
    let source = env("DBN_TCP_SOURCE", &addr);
    let listen = env("DBN_TCP_LISTEN", "127.0.0.1:9103");
    let observer = Observer::new("dbn-tcp", env!("CARGO_PKG_VERSION"));
    let shared = Arc::new(Shared::default());
    register_metrics(&observer, &shared);
    observer.describe(serde_json::json!({
        "role": format!("Live DBN stream from {source} ({addr})."),
        "upstream": [{ "id": slug(&source), "label": source, "sub": addr, "kind": "flow", "via": "DBN over TCP" }],
        "streams": { "events": "/events", "series": "/recent" },
        "panels": [{ "heading": "Live trades", "note": "Trade prices, sampled once per second", "decimals": 2 }]
    }));

    let app = Router::new()
        .merge(observer.router())
        .merge(shared.events.router())
        .merge(shared.series.router());
    let listener = match tokio::net::TcpListener::bind(&listen).await {
        Ok(l) => l,
        Err(e) => {
            eprintln!("dbn-tcp: cannot listen on {listen}: {e}");
            std::process::exit(2);
        }
    };
    eprintln!("dbn-tcp: reading {addr} ({source}); endpoints on http://{listen}");
    tokio::spawn(async move {
        if let Err(e) = axum::serve(listener, app).await {
            eprintln!("dbn-tcp: http server stopped: {e}");
        }
    });

    tokio::select! {
        () = stream_forever(&addr, &observer, &shared) => {}
        _ = tokio::signal::ctrl_c() => {}
        () = terminate() => {}
    }
    observer.set_state(State::Stopping, "shutting down");
}

/// Connect, read and reconnect with backoff until the process is stopped.
async fn stream_forever(addr: &str, observer: &Observer, shared: &Shared) {
    let mut backoff = Duration::from_secs(1);
    loop {
        observer.set_state(State::Starting, "connecting");
        if let Err(e) = stream_once(addr, observer, shared).await {
            eprintln!("dbn-tcp: {e}");
            observer.record_error(&e);
        }
        if shared.connected.swap(false, Ordering::Relaxed) {
            backoff = Duration::from_secs(1);
        }
        observer.set_state(State::Degraded, "disconnected; reconnecting");
        tokio::time::sleep(backoff).await;
        backoff = (backoff * 2).min(Duration::from_secs(30));
        shared.reconnects.fetch_add(1, Ordering::Relaxed);
    }
}

async fn stream_once(addr: &str, observer: &Observer, shared: &Shared) -> Result<(), String> {
    let stream = tokio::net::TcpStream::connect(addr)
        .await
        .map_err(|e| format!("connect to {addr} failed: {e}"))?;
    let mut decoder = AsyncDbnDecoder::new(stream)
        .await
        .map_err(|e| format!("not a DBN stream: {e}"))?;
    shared.connected.store(true, Ordering::Relaxed);
    observer.set_state(State::Ready, "connected; waiting for records");

    let mut symbols: HashMap<u32, String> = HashMap::new();
    while let Some(rec) = decoder
        .decode_record_ref()
        .await
        .map_err(|e| format!("read failed: {e}"))?
    {
        *lock(&shared.last_record) = Some(Instant::now());
        let id = rec.header().instrument_id;
        let symbol = |symbols: &HashMap<u32, String>| {
            symbols.get(&id).cloned().unwrap_or_else(|| id.to_string())
        };
        let kind = if let Some(d) = rec.get::<InstrumentDefMsg>() {
            if let Ok(s) = d.raw_symbol() {
                symbols.insert(id, s.to_owned());
            }
            "definition"
        } else if let Some(m) = rec.get::<SymbolMappingMsg>() {
            if let Ok(s) = m.stype_out_symbol() {
                symbols.insert(id, s.to_owned());
            }
            "symbol_mapping"
        } else if let Some(t) = rec.get::<TradeMsg>() {
            trade(shared, &symbol(&symbols), t.price, t.size, format!("{t:?}"));
            "trade"
        } else if let Some(m) = rec.get::<Mbp1Msg>() {
            let sym = symbol(&symbols);
            if m.action as u8 == b'T' {
                trade(shared, &sym, m.price, m.size, format!("{m:?}"));
                "trade"
            } else {
                let lvl = &m.levels[0];
                shared.events.push(
                    format!(
                        "{sym} {} × {} / {} × {}",
                        lvl.bid_sz,
                        px(lvl.bid_px),
                        px(lvl.ask_px),
                        lvl.ask_sz
                    ),
                    format!("{m:?}"),
                );
                "quote"
            }
        } else if let Some(m) = rec.get::<SystemMsg>() {
            if m.is_heartbeat() || m.msg().is_ok_and(|s| s.ends_with("heartbeat")) {
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
            observer.record_error(&format!("source: {text}"));
            shared
                .events
                .push(format!("error: {text}"), format!("{m:?}"));
            "error"
        } else {
            "other"
        };
        if kind == "trade" || kind == "quote" {
            observer.set_state(State::Ready, "streaming");
        }
        *lock(&shared.records).entry(kind).or_default() += 1;
    }
    Err("source closed the connection".into())
}

fn trade(shared: &Shared, symbol: &str, price: i64, size: u32, raw: String) {
    shared
        .events
        .push(format!("{symbol} {size} @ {}", px(price)), raw);
    if price != UNDEF_PRICE {
        shared.series.push(symbol, price as f64 / 1e9);
    }
}

fn px(p: i64) -> String {
    if p == UNDEF_PRICE {
        "-".into()
    } else {
        format!("{:.2}", p as f64 / 1e9)
    }
}

fn register_metrics(observer: &Observer, shared: &Arc<Shared>) {
    let s = shared.clone();
    observer.set_metrics(move |w| {
        w.gauge(
            "glacier_dbn_tcp_connected",
            "1 while connected to the source.",
            &[],
            if s.connected.load(Ordering::Relaxed) {
                1.0
            } else {
                0.0
            },
        );
        w.counter(
            "glacier_dbn_tcp_reconnects_total",
            "Reconnect attempts since start.",
            &[],
            s.reconnects.load(Ordering::Relaxed) as f64,
        );
        for (kind, n) in lock(&s.records).iter() {
            w.counter(
                "glacier_dbn_tcp_records_total",
                "Records received, by kind.",
                &[("kind", kind)],
                *n as f64,
            );
        }
        if let Some(at) = *lock(&s.last_record) {
            w.gauge(
                "glacier_dbn_tcp_last_record_age_seconds",
                "Seconds since the last record of any kind.",
                &[],
                at.elapsed().as_secs_f64(),
            );
        }
    });
}

/// A graph id for a source name: lower case, other characters as `-`.
fn slug(s: &str) -> String {
    s.to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
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
    fn formats_prices_and_slugs() {
        assert_eq!(px(450_120_000_000), "450.12");
        assert_eq!(px(UNDEF_PRICE), "-");
        assert_eq!(slug("Hyphae relay (IEX)"), "hyphae-relay--iex-");
    }
}

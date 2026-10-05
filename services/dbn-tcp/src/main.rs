//! Example service: connects to a TCP address that serves a live DBN stream
//! (a metadata header followed by records), keeps a live tail and price
//! series, and exposes `/health`, `/metrics`, `/describe`, `/events` and
//! `/recent`. Decoding is done by the official `dbn` crate.
//!
//! Symbols come from instrument definition and symbol mapping records and are
//! keyed by name, since instrument ids can change between sessions. Trades
//! are `trades` records, or `mbp-1` records with action `T`; other `mbp-1`
//! records are shown as quotes.
//!
//! In-band markers of the form `hyphae:v=1 <kind> key=value ...` (sent by
//! Hyphae feed relays) are acted on: `snapshot begin`/`end` bracket starting
//! state, which is not shown as live data; `gap` records a reported loss;
//! `reconnect` notes the source's own upstream reconnecting; `heartbeat`
//! means quiet, not dead; a `slow-consumer` error triggers an immediate
//! reconnect. Every reconnect rebuilds state from the fresh header,
//! definitions and snapshot.
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
    gaps: AtomicU64,
    last_gap: Mutex<Option<(f64, f64)>>,
    upstream_reconnects: AtomicU64,
    slow_consumer_drops: AtomicU64,
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

    // State is rebuilt from scratch on every connection: the source sends its
    // header, definitions and (for relays that support it) a snapshot again.
    // Symbols are keyed by name, never by instrument id, because ids can change
    // between sessions.
    let mut symbols: HashMap<u32, String> = HashMap::new();
    let mut in_snapshot = false;
    while let Some(rec) = decoder
        .decode_record_ref()
        .await
        .map_err(|e| format!("read failed: {e}"))?
    {
        *lock(&shared.last_record) = Some(Instant::now());
        let symbol = symbols.get(&rec.header().instrument_id).cloned();
        let kind = if let Some(d) = rec.get::<InstrumentDefMsg>() {
            if let Ok(s) = d.raw_symbol() {
                symbols.insert(d.hd.instrument_id, s.to_owned());
            }
            "definition"
        } else if let Some(m) = rec.get::<SymbolMappingMsg>() {
            if let Ok(s) = m.stype_out_symbol() {
                symbols.insert(m.hd.instrument_id, s.to_owned());
            }
            "symbol_mapping"
        } else if in_snapshot && (rec.get::<TradeMsg>().is_some() || rec.get::<Mbp1Msg>().is_some())
        {
            // Starting state with its original timestamps, not time passing.
            "snapshot"
        } else if let Some(t) = rec.get::<TradeMsg>() {
            trade(shared, symbol.as_deref(), t.price, t.size, format!("{t:?}"));
            "trade"
        } else if let Some(m) = rec.get::<Mbp1Msg>() {
            if m.action as u8 == b'T' {
                trade(shared, symbol.as_deref(), m.price, m.size, format!("{m:?}"));
                "trade"
            } else {
                let lvl = &m.levels[0];
                shared.events.push(
                    format!(
                        "{} {} × {} / {} × {}",
                        symbol.as_deref().unwrap_or("?"),
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
            let text = m.msg().unwrap_or("").to_owned();
            match Marker::parse(&text) {
                Some(mk) => handle_marker(&mk, &text, observer, shared, &mut in_snapshot),
                None if m.is_heartbeat() => "heartbeat",
                None => {
                    shared
                        .events
                        .push(format!("system: {text}"), format!("{m:?}"));
                    "system"
                }
            }
        } else if let Some(m) = rec.get::<ErrorMsg>() {
            let text = m.err().unwrap_or("unknown error").to_owned();
            observer.record_error(&format!("source: {text}"));
            shared
                .events
                .push(format!("error: {text}"), format!("{m:?}"));
            if Marker::parse(&text).is_some_and(|mk| mk.kind == "slow-consumer") {
                // The source drops us right after this; reconnect at once.
                shared.slow_consumer_drops.fetch_add(1, Ordering::Relaxed);
                return Err("dropped by the source as a slow consumer".into());
            }
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

/// An in-band marker: `hyphae:v=1 <kind> key=value ...`, as sent by Hyphae
/// feed relays.
struct Marker<'a> {
    kind: &'a str,
    fields: HashMap<&'a str, &'a str>,
}

impl<'a> Marker<'a> {
    fn parse(text: &'a str) -> Option<Self> {
        let mut words = text.trim_end_matches('\0').split_whitespace();
        if words.next()? != "hyphae:v=1" {
            return None;
        }
        let kind = words.next()?;
        let fields = words.filter_map(|w| w.split_once('=')).collect();
        Some(Self { kind, fields })
    }

    fn ns(&self, key: &str) -> Option<f64> {
        self.fields.get(key)?.parse::<f64>().ok().map(|n| n / 1e9)
    }
}

/// Act on a marker; returns the record kind to count it as.
fn handle_marker(
    mk: &Marker,
    text: &str,
    observer: &Observer,
    shared: &Shared,
    in_snapshot: &mut bool,
) -> &'static str {
    match (mk.kind, mk.fields.keys().next().copied()) {
        ("snapshot", _) if text.contains("snapshot begin") => {
            *in_snapshot = true;
            shared.events.push(
                format!(
                    "snapshot: {} instruments as starting state",
                    mk.fields.get("count").unwrap_or(&"?")
                ),
                text.to_owned(),
            );
            "marker"
        }
        ("snapshot", _) => {
            *in_snapshot = false;
            "marker"
        }
        ("heartbeat", _) => {
            // Quiet, not dead. A relay whose own upstream is down says so here.
            if mk.fields.get("upstream") == Some(&"down") {
                observer.set_state(State::Degraded, "source reports its upstream is down");
            }
            "heartbeat"
        }
        ("gap", _) => {
            shared.gaps.fetch_add(1, Ordering::Relaxed);
            if let (Some(from), Some(to)) = (mk.ns("from_ns"), mk.ns("to_ns")) {
                *lock(&shared.last_gap) = Some((from, to));
            }
            shared.events.push(
                format!(
                    "gap: data lost upstream ({})",
                    mk.fields.get("reason").unwrap_or(&"no reason given")
                ),
                text.to_owned(),
            );
            "gap"
        }
        ("reconnect", _) => {
            shared.upstream_reconnects.fetch_add(1, Ordering::Relaxed);
            shared
                .events
                .push("source reconnected to its upstream".into(), text.to_owned());
            "marker"
        }
        _ => {
            shared
                .events
                .push(format!("system: {text}"), text.to_owned());
            "marker"
        }
    }
}

fn trade(shared: &Shared, symbol: Option<&str>, price: i64, size: u32, raw: String) {
    shared.events.push(
        format!("{} {size} @ {}", symbol.unwrap_or("?"), px(price)),
        raw,
    );
    if let Some(symbol) = symbol
        && price != UNDEF_PRICE
    {
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
        w.counter(
            "glacier_dbn_tcp_gaps_total",
            "Intervals the source reported as lost upstream.",
            &[],
            s.gaps.load(Ordering::Relaxed) as f64,
        );
        if let Some((from, to)) = *lock(&s.last_gap) {
            w.gauge(
                "glacier_dbn_tcp_last_gap_start_seconds",
                "Start of the most recent reported gap.",
                &[],
                from,
            );
            w.gauge(
                "glacier_dbn_tcp_last_gap_end_seconds",
                "End of the most recent reported gap.",
                &[],
                to,
            );
        }
        w.counter(
            "glacier_dbn_tcp_upstream_reconnects_total",
            "Times the source reported reconnecting to its own upstream.",
            &[],
            s.upstream_reconnects.load(Ordering::Relaxed) as f64,
        );
        w.counter(
            "glacier_dbn_tcp_slow_consumer_drops_total",
            "Times the source dropped this reader for falling behind.",
            &[],
            s.slow_consumer_drops.load(Ordering::Relaxed) as f64,
        );
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

    #[test]
    fn parses_markers() {
        let m = Marker::parse(
            "hyphae:v=1 gap feed=x from_ns=1000000000 to_ns=3000000000 reason=upstream-closed",
        )
        .unwrap();
        assert_eq!(m.kind, "gap");
        assert_eq!(m.ns("from_ns"), Some(1.0));
        assert_eq!(m.fields["reason"], "upstream-closed");
        assert_eq!(
            Marker::parse("hyphae:v=1 snapshot begin count=3")
                .unwrap()
                .fields["count"],
            "3"
        );
        assert!(Marker::parse("heartbeat").is_none());
    }
}

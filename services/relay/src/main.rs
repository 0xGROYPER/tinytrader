//! Market data relay (`glacier.relay/1`): holds one provider connection and
//! serves its records to any number of local clients. It only forwards; it
//! keeps no recording. See decision 0006 for the protocol.
//!
//! A client connects over TCP and sends one line of JSON,
//! `{"protocol":"glacier.relay/1"}`, optionally with
//! `"start":"<RFC 3339 time>"` and `"conditions":true`. The relay answers with
//! a DBN v3 stream: reference records (definitions, and statistics where the
//! source has them), any catch-up the source supports, a `ReplayCompleted`
//! system record, then live data.
//!
//! Sources:
//! - `databento`: any live dataset (OPRA by default). Catch-up is the day's
//!   reference data only; Databento can't replay `cmbp-1`.
//! - `alpaca-iex`: Alpaca's IEX quotes and trades as `ALPACA.IEX` MBP-1.
//!   Catch-up replays history from `start`; `conditions` interleaves a
//!   `glacier.trade-conditions/1` record after each trade.
//!
//! Credentials: `DATABENTO_API_KEY`, or `APCA_API_KEY_ID` and
//! `APCA_API_SECRET_KEY`; each also as `<NAME>_FILE` naming a file.

mod alpaca;
mod databento;
mod hub;
mod protocol;

use std::str::FromStr as _;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use clap::{Parser, ValueEnum};
use databento::Databento;
use glacier_observe::{Observer, State, listen_address};
use hub::{ClientGuard, Hub, Upstream};
use serde_json::json;

use ::databento::dbn::{Metadata, SType, Schema};

#[derive(Clone, Copy, Debug, ValueEnum)]
enum Source {
    Databento,
    AlpacaIex,
}

/// Holds one provider connection and serves its records to local clients.
#[derive(Parser, Debug)]
#[command(version, about)]
struct Args {
    /// Where the data comes from.
    #[arg(long, env = "RELAY_SOURCE", value_enum, default_value = "databento")]
    source: Source,

    /// Dataset to relay.
    #[arg(long, env = "RELAY_DATASET", default_value = "OPRA.PILLAR")]
    dataset: String,

    /// Symbols, comma-separated, e.g. `SPY.OPT`.
    #[arg(long, env = "RELAY_SYMBOLS", value_delimiter = ',', required = true)]
    symbols: Vec<String>,

    /// Schema to relay live.
    #[arg(long, env = "RELAY_SCHEMA", default_value = "cmbp-1")]
    schema: String,

    /// Reference schemas sent live too, and replayed for clients catching
    /// up, comma-separated.
    #[arg(
        long,
        env = "RELAY_REFERENCE",
        value_delimiter = ',',
        default_value = "definition,statistics"
    )]
    reference: Vec<String>,

    /// Symbology of `--symbols`: `parent`, `raw_symbol`, ...
    #[arg(long, env = "RELAY_STYPE_IN", default_value = "parent")]
    stype_in: String,

    /// Address clients connect to.
    #[arg(long, env = "RELAY_LISTEN", default_value = "127.0.0.1:7600")]
    listen: String,

    /// Chunks of up to 64 KiB queued per client before it is disconnected.
    #[arg(long, env = "RELAY_CLIENT_QUEUE", default_value_t = 1024)]
    client_queue: usize,

    /// Seconds without data before a heartbeat is sent.
    #[arg(long, env = "RELAY_HEARTBEAT", default_value_t = 10)]
    heartbeat: u64,

    /// Alpaca only: use Alpaca's test stream (symbol `FAKEPACA`), which runs
    /// at any time of day.
    #[arg(long, env = "RELAY_ALPACA_TEST")]
    alpaca_test: bool,
}

/// The provider a relay holds.
enum Provider {
    Databento(Arc<Databento>),
    Alpaca(Arc<alpaca::Alpaca>),
}

#[tokio::main]
async fn main() {
    // One TLS crypto provider for the websocket and HTTP clients.
    let _ = rustls::crypto::ring::default_provider().install_default();
    let args = Args::parse();
    if let Err(e) = run(args).await {
        eprintln!("relay: {e}");
        std::process::exit(2);
    }
}

async fn run(args: Args) -> Result<(), String> {
    let schema =
        Schema::from_str(&args.schema).map_err(|e| format!("--schema {}: {e}", args.schema))?;
    let stype_in = SType::from_str(&args.stype_in)
        .map_err(|e| format!("--stype-in {}: {e}", args.stype_in))?;
    let reference = args
        .reference
        .iter()
        .map(|s| Schema::from_str(s).map_err(|e| format!("--reference {s}: {e}")))
        .collect::<Result<Vec<_>, _>>()?;
    let (dataset, provider) = match args.source {
        Source::Databento => (
            args.dataset.clone(),
            Provider::Databento(Arc::new(Databento {
                key: secret("DATABENTO_API_KEY")?,
                dataset: args.dataset.clone(),
                symbols: args.symbols.clone(),
                schema,
                reference,
                stype_in,
            })),
        ),
        Source::AlpacaIex => (
            relay::iex::DATASET.to_owned(),
            Provider::Alpaca(Arc::new(alpaca::Alpaca {
                key: secret("APCA_API_KEY_ID")?,
                secret: secret("APCA_API_SECRET_KEY")?,
                symbols: args.symbols.clone(),
                test: args.alpaca_test,
                http: reqwest::Client::builder()
                    .timeout(Duration::from_secs(30))
                    .build()
                    .map_err(|e| e.to_string())?,
            })),
        ),
    };
    let (stype_in, schema_name, provider_id, via) = match provider {
        Provider::Databento(_) => (stype_in, args.schema.clone(), "databento", "live API"),
        Provider::Alpaca(_) => (SType::RawSymbol, "mbp-1".to_owned(), "alpaca", "websocket"),
    };

    let metadata = Metadata::builder()
        .dataset(dataset.clone())
        .schema(None)
        .start(0)
        .stype_in(Some(stype_in))
        .stype_out(SType::InstrumentId)
        .symbols(args.symbols.clone())
        .build();
    let hub = Arc::new(Hub::new(
        metadata,
        args.client_queue,
        Duration::from_secs(args.heartbeat.max(1)),
    ));

    let observer = Observer::new("relay", env!("CARGO_PKG_VERSION"));
    observer.set_state(State::Starting, &format!("connecting to {provider_id}"));
    observer.describe(json!({
        "role": format!("Relays {dataset} {schema_name} for {} to local clients ({}).", args.symbols.join(", "), protocol::PROTOCOL),
        "upstream": [{ "id": provider_id, "label": provider_id, "sub": dataset, "kind": "flow", "via": via }],
        "streams": { "events": "/events" },
        "relay": { "protocol": protocol::PROTOCOL, "listen": args.listen, "symbols": args.symbols },
    }));
    register_metrics(&observer, &hub);

    let observe_at = listen_address("RELAY_OBSERVE_LISTEN", "127.0.0.1:9110");
    let app = observer.router().merge(hub.events.router());
    let observe = tokio::net::TcpListener::bind(&observe_at)
        .await
        .map_err(|e| format!("cannot listen on {observe_at}: {e}"))?;
    tokio::spawn(async move {
        if let Err(e) = axum::serve(observe, app).await {
            eprintln!("relay: http server stopped: {e}");
        }
    });

    let clients = tokio::net::TcpListener::bind(&args.listen)
        .await
        .map_err(|e| format!("cannot listen on {}: {e}", args.listen))?;
    eprintln!(
        "relay: {dataset} {schema_name} for {} on {} (endpoints on http://{observe_at})",
        args.symbols.join(","),
        args.listen
    );
    let alpaca = match &provider {
        Provider::Databento(d) => {
            tokio::spawn(d.clone().run(hub.clone(), observer.clone()));
            None
        }
        Provider::Alpaca(a) => {
            tokio::spawn(a.clone().run(hub.clone(), observer.clone()));
            Some(a.clone())
        }
    };

    loop {
        tokio::select! {
            accepted = clients.accept() => {
                let Ok((stream, peer)) = accepted else { continue };
                let _ = stream.set_nodelay(true);
                let (hub, alpaca) = (hub.clone(), alpaca.clone());
                tokio::spawn(async move {
                    let Some((join, out)) = hub::read_join(stream, &hub).await else { return };
                    if join.conditions && alpaca.is_none() {
                        hub.refuse(out, ::databento::dbn::ErrorCode::InvalidSubscription,
                            "this source has no trade conditions").await;
                        return;
                    }
                    let now = protocol::now_ns();
                    let requested = join.start.and_then(|t| u64::try_from(t.unix_timestamp_nanos()).ok());
                    // Alpaca serves history; catch up from the requested start
                    // to now, then continue live from the same point.
                    let (history, cut) = match (&alpaca, requested) {
                        (Some(a), Some(start)) if start < now => (Some(a.catch_up(start, now, join.conditions)), now),
                        _ => (None, 0),
                    };
                    hub.events.push(format!("client {peer} joined"), format!("client {peer} joined"));
                    let _guard = ClientGuard::new(&hub.clients);
                    hub.serve(out, hub::Serve { start: requested.unwrap_or(now), conditions: join.conditions, history, cut }).await;
                });
            }
            _ = shutdown() => {
                observer.set_state(State::Stopping, "shutting down");
                return Ok(());
            }
        }
    }
}

fn register_metrics(observer: &Observer, hub: &Arc<Hub>) {
    let hub = hub.clone();
    observer.set_metrics(move |w| {
        let c = |a: &std::sync::atomic::AtomicU64| a.load(Ordering::Relaxed) as f64;
        w.gauge(
            "glacier_relay_clients",
            "Connected clients.",
            &[],
            c(&hub.clients),
        );
        w.gauge(
            "glacier_relay_reference_records",
            "Reference records (definitions, statistics) held for joining clients.",
            &[],
            hub.reference_len() as f64,
        );
        w.gauge(
            "glacier_relay_upstream_up",
            "1 while the provider connection is up.",
            &[],
            f64::from(u8::from(hub.upstream() == Upstream::Up)),
        );
        w.counter(
            "glacier_relay_records_total",
            "Records received from the provider.",
            &[],
            c(&hub.records),
        );
        w.counter(
            "glacier_relay_bytes_total",
            "Bytes received from the provider.",
            &[],
            c(&hub.bytes),
        );
        w.counter(
            "glacier_relay_upstream_reconnects_total",
            "Provider connections restored after a loss.",
            &[],
            c(&hub.reconnects),
        );
        w.counter(
            "glacier_relay_slow_disconnects_total",
            "Clients disconnected for falling behind.",
            &[],
            c(&hub.slow_disconnects),
        );
        w.counter(
            "glacier_relay_refused_total",
            "Join requests refused.",
            &[],
            c(&hub.refused),
        );
    });
}

async fn shutdown() {
    let ctrl_c = tokio::signal::ctrl_c();
    #[cfg(unix)]
    {
        let mut term =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()).ok();
        tokio::select! {
            _ = ctrl_c => {}
            () = async { match term.as_mut() { Some(t) => { t.recv().await; } None => std::future::pending().await } } => {}
        }
    }
    #[cfg(not(unix))]
    let _ = ctrl_c.await;
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

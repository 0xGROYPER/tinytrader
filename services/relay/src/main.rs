//! Market data relay (`glacier.relay/1`): holds one provider connection and
//! serves its records to any number of local clients. It only forwards; it
//! keeps no recording. See decision 0006 for the protocol.
//!
//! A client connects over TCP and sends one line of JSON,
//! `{"protocol":"glacier.relay/1"}` for live data from now, or with
//! `"start":"<RFC 3339 time>"` to replay from then and continue live. The
//! relay answers with a DBN v3 stream.
//!
//! Sources: `databento` (any live dataset; OPRA by default).
//!
//! Credentials: `DATABENTO_API_KEY`, or `DATABENTO_API_KEY_FILE` naming a file
//! containing it.

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

    /// Schema to relay; instrument definitions are always included.
    #[arg(long, env = "RELAY_SCHEMA", default_value = "cmbp-1")]
    schema: String,

    /// Symbology of `--symbols`: `parent`, `raw_symbol`, ...
    #[arg(long, env = "RELAY_STYPE_IN", default_value = "parent")]
    stype_in: String,

    /// Address clients connect to.
    #[arg(long, env = "RELAY_LISTEN", default_value = "127.0.0.1:7600")]
    listen: String,

    /// Most clients catching up at once (each holds its own provider session).
    #[arg(long, env = "RELAY_MAX_REPLAYS", default_value_t = 4)]
    max_replays: usize,

    /// Chunks of up to 64 KiB queued per client before it is disconnected.
    #[arg(long, env = "RELAY_CLIENT_QUEUE", default_value_t = 1024)]
    client_queue: usize,

    /// Seconds without data before a heartbeat is sent.
    #[arg(long, env = "RELAY_HEARTBEAT", default_value_t = 10)]
    heartbeat: u64,
}

#[tokio::main]
async fn main() {
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
    let Source::Databento = args.source;
    let key = secret("DATABENTO_API_KEY")?;

    let metadata = Metadata::builder()
        .dataset(args.dataset.clone())
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
    let source = Arc::new(Databento {
        key,
        dataset: args.dataset.clone(),
        symbols: args.symbols.clone(),
        schema,
        stype_in,
        replays: tokio::sync::Semaphore::new(args.max_replays),
    });

    let observer = Observer::new("relay", env!("CARGO_PKG_VERSION"));
    observer.set_state(State::Starting, "connecting to Databento");
    observer.describe(json!({
        "role": format!("Relays {} {} for {} to local clients ({}).", args.dataset, args.schema, args.symbols.join(", "), protocol::PROTOCOL),
        "upstream": [{ "id": "databento", "label": "databento", "sub": args.dataset, "kind": "flow", "via": "live API" }],
        "streams": { "events": "/events" },
        "relay": { "protocol": protocol::PROTOCOL, "listen": args.listen, "symbols": args.symbols },
    }));
    register_metrics(&observer, &hub, &source, args.max_replays);

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
        "relay: {} {} for {} on {} (endpoints on http://{observe_at})",
        args.dataset,
        args.schema,
        args.symbols.join(","),
        args.listen
    );
    tokio::spawn(source.clone().run(hub.clone(), observer.clone()));

    loop {
        tokio::select! {
            accepted = clients.accept() => {
                let Ok((stream, peer)) = accepted else { continue };
                let _ = stream.set_nodelay(true);
                let (hub, source) = (hub.clone(), source.clone());
                tokio::spawn(async move {
                    let Some((join, out)) = hub::read_join(stream, &hub).await else { return };
                    if join.conditions {
                        hub.refuse(out, ::databento::dbn::ErrorCode::InvalidSubscription,
                            "this source has no trade conditions").await;
                        return;
                    }
                    match join.start {
                        Some(start) => source.replay(start, out, &hub).await,
                        None => {
                            hub.events.push(format!("client {peer} joined live"), format!("client {peer} joined live"));
                            let _guard = ClientGuard::new(&hub.clients);
                            hub.serve_live(out).await;
                        }
                    }
                });
            }
            _ = shutdown() => {
                observer.set_state(State::Stopping, "shutting down");
                return Ok(());
            }
        }
    }
}

fn register_metrics(observer: &Observer, hub: &Arc<Hub>, source: &Arc<Databento>, max: usize) {
    let (hub, source) = (hub.clone(), source.clone());
    observer.set_metrics(move |w| {
        let c = |a: &std::sync::atomic::AtomicU64| a.load(Ordering::Relaxed) as f64;
        w.gauge(
            "glacier_relay_clients",
            "Connected clients.",
            &[],
            c(&hub.clients),
        );
        w.gauge(
            "glacier_relay_replay_sessions",
            "Clients catching up on their own provider session.",
            &[],
            (max - source.replays.available_permits()) as f64,
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

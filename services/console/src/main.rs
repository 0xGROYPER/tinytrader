//! tinytrader console backend.
//!
//! Serves the console page and a small read-only API:
//!
//! - `GET /api/topology`: the deployment as drawn: hosts, services, stores,
//!   edges and panels.
//! - `GET /api/state`: the latest snapshot of every service: container state
//!   from the Docker Engine API, plus each service's `/health`, `/metrics`
//!   and optional `/recent` endpoints. With `?since=G&epoch=E` (the last
//!   answer's), only the service fields changed since (see `delta`).
//!   Each account in a service's `accounts` carries `age_ms`: the console's
//!   clock now minus the account's `updated_at`, or null.
//!
//! A background task refreshes the snapshot every two seconds, so any number
//! of viewers cost one round of polling. The console never writes to Docker
//! or to any service; it should reach Docker through a read-only proxy.
//!
//! **Discovery.** With Docker configured, every container in the project
//! appears in the graph. A container labelled `tinytrader.observe=<port>` is
//! asked for its `/describe` document, which supplies its role, upstream
//! sources, stores, event and series endpoints and suggested panels. Other
//! labels: `tinytrader.role` (a description for containers without
//! `/describe`), `tinytrader.label` and `tinytrader.host`. Without Docker, or for
//! services Docker doesn't run, a topology entry such as
//! `{ "id": "ticker", "observe": "http://127.0.0.1:9105" }` is described and
//! polled the same way.
//!
//! **Overrides.** The topology file is optional. When present, it is layered
//! over what was discovered: `name`, `focus` and `hosts` replace; `services`,
//! `external` and `stores` entries merge by `id`; `edges` are added; `panels`
//! for a service replace that service's suggested panels; `hide` lists ids to
//! leave out. Without Docker, the file alone describes the deployment.
//!
//! Configuration (environment):
//! - `CONSOLE_LISTEN` (or `TINYTRADER_LISTEN`): address to serve on (default `127.0.0.1:8080`)
//! - `CONSOLE_TOPOLOGY`: path to the optional topology file (default `topology.json`)
//! - `CONSOLE_DOCKER`: Docker Engine API base URL, e.g. `http://docker-proxy:2375` (optional)
//! - `CONSOLE_COMPOSE_PROJECT`: only show containers from this Compose project (optional)
//! - `CONSOLE_COLLECTIONS`: document collections shown in the sidebar, e.g.
//!   `Docs=/srv/docs;Research=/srv/research` (optional; see `collections.rs`)
//! - `CONSOLE_NOTIFY`: a URL to POST a notice to when a service goes down or
//!   comes back (ntfy-style: the body is the message, with `Title`, `Priority`
//!   and `Tags` headers), or `CONSOLE_NOTIFY_FILE` naming a file holding it.
//!   Optional. Down: a container exited, dead, restarting or gone, or a
//!   service reporting `failed` (`notify.rs`).
//! - `CONSOLE_STORE`: the store, e.g. `http://store:9120`, so
//!   the Storage widget can approve a new drive (decision 0007). Only that
//!   one call is passed through. `CONSOLE_STORE_TOKEN` (or `_FILE`): the
//!   store's write token, when the store requires one, sent with it.
//! - `CONSOLE_JOBS`: the Databento jobs service, e.g. `http://databento-jobs:9121`
//!   (optional). The one exception to read-only: `/api/jobs/...` is passed to
//!   it, so the Data page can price, submit and download jobs. The jobs
//!   service itself insists on an exact confirmation before any charge.
//! - `CONSOLE_QUEUE`: the job queue, e.g. `http://queue:9123` (optional;
//!   decision 0012). `/api/queue/...` is passed to it, so the Queue widget
//!   can add, edit and cancel jobs; the queue runs only its templates.
//! - `CONSOLE_GATEWAYS`: order gateways by account, e.g.
//!   `paper=gateway-paper:7701;live=gateway-live:7710`, and
//!   `CONSOLE_GATEWAY_TOKEN` (or `_FILE`), the operator's admin token for them
//!   (optional). `/api/accounts/{account}/flatten` and `.../resume` act through
//!   the account's gateway; an account without one can't be flattened.
//!   The console says `hello` as `CONSOLE_GATEWAY_CLIENT` (default `ops`).
//!   With `CONSOLE_TRADE_TOKEN` (or `_FILE`), the console's trade client
//!   (`CONSOLE_TRADE_CLIENT`, default `console`; order ids start with
//!   `CONSOLE_TRADE_PREFIX`, default `tt-`) sends the Spread builder's orders: `GET /api/orders` (each
//!   account's gateway state, its `unresolved` count and the console's open orders), `POST
//!   /api/orders/{account}` (`{order, confirm: account}`) and `POST
//!   /api/orders/{account}/{client_order_id}/cancel`.
//!   Every write route needs `CONSOLE_ORDER_LOGINS` (the Tailscale logins
//!   allowed to write, comma-separated) and `CONSOLE_HOSTS` (the host names
//!   the console is reached by, comma-separated, ports optional); an empty
//!   list refuses all writes (`WriteGate`). A live account's order, cancel,
//!   flatten or resume also needs the order token in the page's
//!   `X-Order-Token`: the file `CONSOLE_ORDER_TOKEN_FILE` names (one line,
//!   e.g. 64 hex characters), or else `CONSOLE_ORDER_TOKEN`. So does a paper
//!   account's flatten or resume unless `CONSOLE_TOKEN_FOR_PAPER_FLATTEN` is
//!   `false` (default `true`). Without a token, those are refused and
//!   everything else still works. Paper orders need only the login. A live
//!   account's flatten or resume also needs `confirm` naming the account. An
//!   account is paper only when both the accounts service's `environment` for
//!   it (`alpaca-account`) and its gateway's welcome say `paper`.
//!   `GET /api/write-gate` says whether writes are on for the viewer.
//!   The write routes are the order routes (those, and an account's flatten
//!   and resume), the jobs service's and the queue's POST (and the queue's
//!   PATCH) routes, drive approval, POST `/api/reminders` and `/api/profiles`,
//!   PUT `/api/prefs/{key}`, and the journal's POST, PUT and DELETE. Their GET
//!   routes stay open.
//! - `CONSOLE_PAGE`: serve the page from this file, and its modules and
//!   libraries from beside it, read on each request, instead of the copies
//!   built in (optional; lets the page change without a rebuild).
//! - `CONSOLE_PROFILES`: where profile-mode recordings are kept
//!   (`/api/profiles`); by default `profiles/` beside `CONSOLE_REMINDERS`.
//! - `CONSOLE_PREFS`: a JSON file of the page's settings that follow the user
//!   between devices (`/api/prefs`); by default `prefs.json` beside
//!   `CONSOLE_REMINDERS`. `CONSOLE_PREFS_DEFAULTS`: a read-only JSON file of
//!   the same keys, used where the settings have none, such as curated views
//!   kept in a deployment's repository and written by
//!   `scripts/prefs-promote` (optional).
//! - `CONSOLE_REMINDERS`: a JSON file for the roadmap's reminders, which the
//!   console edits (`/api/reminders`); it may be a file in a git checkout,
//!   and is read again when it changes on disk. `CONSOLE_REMINDERS_SEED`: a
//!   Markdown roadmap to start it from when there is no file (optional).
//! - `CONSOLE_JOURNAL`: the trading journal's JSON file (`/api/journal`), by
//!   default `journal.json` beside `CONSOLE_REMINDERS`. Writes are refused
//!   while the file can't be read (it is left as it is).

mod collections;
mod delta;
mod gateways;
mod journal;
mod notify;
mod prefs;
mod profiles;
mod reminders;

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use axum::Router;
use axum::extract::State as AxumState;
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use serde_json::json;
use tinytrader_fleet::{EventsError, Poller, PollerConfig};
use tinytrader_observe::{Observer, State};

/// Every file under `web/`, built in.
mod web {
    include!(concat!(env!("OUT_DIR"), "/web.rs"));
}

struct App {
    poller: Poller,
    /// The jobs service's base URL, and a client to reach it.
    jobs: Option<(String, reqwest::Client)>,
    /// The job queue's base URL, and a client to reach it.
    queue: Option<(String, reqwest::Client)>,
    /// The store's base URL, for approving drives.
    store: Option<String>,
    /// The store's write token, if it requires one.
    store_token: Option<String>,
    /// The roadmap's reminders, if configured.
    /// Settings that follow the user between devices (`/api/prefs`).
    prefs: Option<prefs::Prefs>,
    reminders: Option<reminders::Reminders>,
    /// The trading journal (`/api/journal`), if its file can be read.
    journal: Option<journal::Journal>,
    /// Profile-mode recordings uploaded by pages, if a directory is configured.
    profiles: Option<profiles::Profiles>,
    /// Order gateways by account, for Flatten and Resume.
    gateways: gateways::Gateways,
    /// Who may write, and the token a live account's order needs.
    gate: WriteGate,
    polls: AtomicU64,
    /// Changes-only snapshots for pages that poll.
    delta: delta::Delta,
    collections: Vec<collections::Collection>,
}

#[tokio::main]
async fn main() {
    let listen = tinytrader_observe::listen_address("CONSOLE_LISTEN", "127.0.0.1:8080");
    let store_token = match tinytrader_observe::optional_secret("CONSOLE_STORE_TOKEN") {
        Ok(t) => t,
        Err(e) => {
            eprintln!("console: {e}");
            std::process::exit(2);
        }
    };
    let write_gate = WriteGate::from_env();
    if write_gate.logins.is_empty() {
        eprintln!("console: writes are refused until CONSOLE_ORDER_LOGINS is set");
    }
    if write_gate.hosts.is_empty() {
        eprintln!("console: writes are refused until CONSOLE_HOSTS is set");
    }
    if write_gate.token.is_none() {
        eprintln!(
            "console: live orders are disabled: the console has no order token configured (CONSOLE_ORDER_TOKEN or _FILE)"
        );
    }
    let config = PollerConfig {
        topology: env("CONSOLE_TOPOLOGY", "topology.json"),
        docker: std::env::var("CONSOLE_DOCKER")
            .ok()
            .filter(|s| !s.is_empty()),
        project: std::env::var("CONSOLE_COMPOSE_PROJECT")
            .ok()
            .filter(|s| !s.is_empty()),
    };
    // The Docker proxy, if any, is this console's upstream.
    let proxy = config.docker.as_deref().and_then(|u| {
        u.split("//")
            .nth(1)?
            .split([':', '/'])
            .next()
            .map(str::to_owned)
    });
    let app = Arc::new(App {
        poller: Poller::new(config),
        polls: AtomicU64::new(0),
        delta: delta::Delta::new(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_millis() as u64),
        ),
        collections: collections::parse(&env("CONSOLE_COLLECTIONS", "")),
        store: std::env::var("CONSOLE_STORE")
            .ok()
            .filter(|s| !s.is_empty())
            .map(|s| s.trim_end_matches('/').to_owned()),
        store_token,
        jobs: std::env::var("CONSOLE_JOBS")
            .ok()
            .filter(|s| !s.is_empty())
            .map(|u| (u.trim_end_matches('/').to_owned(), reqwest::Client::new())),
        queue: std::env::var("CONSOLE_QUEUE")
            .ok()
            .filter(|s| !s.is_empty())
            .map(|u| (u.trim_end_matches('/').to_owned(), reqwest::Client::new())),
        gateways: gateways::Gateways::from_env(),
        gate: write_gate.clone(),
        // Beside the reminders unless named.
        profiles: std::env::var("CONSOLE_PROFILES")
            .ok()
            .filter(|s| !s.is_empty())
            .map(std::path::PathBuf::from)
            .or_else(|| {
                std::env::var("CONSOLE_REMINDERS")
                    .ok()
                    .filter(|s| !s.is_empty())
                    .and_then(|r| {
                        std::path::Path::new(&r)
                            .parent()
                            .map(|d| d.join("profiles"))
                    })
            })
            .map(profiles::Profiles::new),
        // Beside the reminders unless named.
        prefs: std::env::var("CONSOLE_PREFS")
            .ok()
            .filter(|s| !s.is_empty())
            .map(std::path::PathBuf::from)
            .or_else(|| {
                std::env::var("CONSOLE_REMINDERS")
                    .ok()
                    .filter(|s| !s.is_empty())
                    .and_then(|r| {
                        std::path::Path::new(&r)
                            .parent()
                            .map(|d| d.join("prefs.json"))
                    })
            })
            .map(|p| {
                let defaults = std::env::var("CONSOLE_PREFS_DEFAULTS")
                    .ok()
                    .filter(|s| !s.is_empty());
                prefs::Prefs::open(p, defaults.as_deref().map(std::path::Path::new))
            }),
        journal: std::env::var("CONSOLE_JOURNAL")
            .ok()
            .filter(|s| !s.is_empty())
            .map(std::path::PathBuf::from)
            .or_else(|| {
                journal::default_path(
                    std::env::var("CONSOLE_REMINDERS")
                        .ok()
                        .filter(|s| !s.is_empty())
                        .map(std::path::PathBuf::from)
                        .as_deref(),
                )
            })
            .and_then(|p| {
                journal::Journal::open(p)
                    .map_err(|e| eprintln!("console: journal off: {e}"))
                    .ok()
            }),
        reminders: std::env::var("CONSOLE_REMINDERS")
            .ok()
            .filter(|s| !s.is_empty())
            .and_then(|p| {
                let seed = std::env::var("CONSOLE_REMINDERS_SEED")
                    .ok()
                    .filter(|s| !s.is_empty());
                // An unreadable file is left for the operator to fix, and
                // reminders are off until then.
                reminders::Reminders::open(
                    std::path::PathBuf::from(p),
                    seed.as_deref().map(std::path::Path::new),
                )
                .map_err(|e| eprintln!("console: reminders off: {e}"))
                .ok()
            }),
    });

    let observer = Observer::new("console", env!("CARGO_PKG_VERSION"));
    observer.describe(json!({
        "role": "The operator console: polls every service and Docker every two seconds.",
        "schedule": [{ "name": "Polling every service and Docker", "kind": "interval", "every_seconds": 2, "when": "every 2 s" }],
        "upstream": proxy.map_or_else(Vec::new, |p| vec![json!({ "service": p, "kind": "read", "via": "container state" })]),
    }));
    {
        let a = app.clone();
        observer.set_metrics(move |w| {
            w.counter(
                "tinytrader_console_polls_total",
                "Snapshot refreshes since start.",
                &[],
                a.polls.load(Ordering::Relaxed) as f64,
            );
        });
    }

    tokio::spawn(poll_forever(app.clone(), observer.clone()));

    let router = routes(write_gate).with_state(app).merge(observer.router());
    let listener = match tokio::net::TcpListener::bind(&listen).await {
        Ok(l) => l,
        Err(e) => {
            eprintln!("console: cannot listen on {listen}: {e}");
            std::process::exit(2);
        }
    };
    eprintln!("console: serving http://{listen}");
    if let Err(e) = axum::serve(listener, router)
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await
    {
        eprintln!("console: server stopped: {e}");
    }
}

async fn poll_forever(app: Arc<App>, observer: Observer) {
    let mut tick = tokio::time::interval(Duration::from_secs(2));
    // Push notifications when a service goes down or comes back (CONSOLE_NOTIFY).
    let notify = match tinytrader_observe::secret("CONSOLE_NOTIFY") {
        Ok(u) if !u.is_empty() => Some(u),
        Ok(_) => None,
        // Unset is fine (no notices); a named file that can't be read is said.
        Err(e) => {
            if std::env::var_os("CONSOLE_NOTIFY_FILE").is_some() {
                eprintln!("console: {e}; no notices");
            }
            None
        }
    };
    let (mut watch, http) = (notify::Watch::default(), reqwest::Client::new());
    loop {
        tick.tick().await;
        let problem = app.poller.poll().await;
        app.polls.fetch_add(1, Ordering::Relaxed);
        if let Some(url) = &notify {
            for n in watch.observe(&app.poller.topology(), &app.poller.snapshot()) {
                if let Err(e) = notify::send(&http, url, &n).await {
                    observer.record_error(&e);
                }
            }
        }
        match problem {
            None => observer.set_state(State::Ready, "polling"),
            Some(p) => {
                observer.record_error(&p);
                observer.set_state(State::Degraded, &p);
            }
        }
    }
}

/// A file of the page, from the directory `CONSOLE_PAGE` names when set
/// (read on each request, so the page changes without a rebuild), or else
/// as built in. One source either way: neither falls back to the other.
fn web_file(path: &str) -> Option<std::borrow::Cow<'static, [u8]>> {
    match std::env::var("CONSOLE_PAGE") {
        Ok(page) => {
            let ok = path.split('/').all(|s| {
                !s.is_empty()
                    && !s.starts_with('.')
                    && s.chars()
                        .all(|c| c.is_ascii_alphanumeric() || "._-".contains(c))
            });
            let dir = std::path::Path::new(&page).parent()?;
            let file = if path == "index.html" {
                std::path::PathBuf::from(&page)
            } else {
                dir.join(path)
            };
            ok.then(|| std::fs::read(file).ok())
                .flatten()
                .map(std::borrow::Cow::Owned)
        }
        Err(_) => web::FILES
            .iter()
            .find(|(n, _)| *n == path)
            .map(|(_, b)| std::borrow::Cow::Borrowed(*b)),
    }
}

async fn file(uri: axum::http::Uri) -> Response {
    let path = uri.path().trim_start_matches('/');
    let Some(body) = web_file(path) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let kind = match path.rsplit('.').next() {
        Some("js") => "text/javascript; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("svg") => "image/svg+xml",
        Some("json") => "application/json",
        Some("woff2") => "font/woff2",
        _ => "application/octet-stream",
    };
    // Libraries carry their versions in their names; the page's own modules change.
    let cache = if path.starts_with("vendor/") {
        "public, max-age=86400"
    } else {
        "no-cache"
    };
    (
        [(header::CONTENT_TYPE, kind), (header::CACHE_CONTROL, cache)],
        NO_FRAMING,
        body,
    )
        .into_response()
}

/// The page's Content-Security-Policy: scripts only from the console itself
/// (the page has no inline scripts or inline event handlers), and no other
/// site may frame it.
const CSP: &str = "script-src 'self'; frame-ancestors 'none'";

/// Headers on the page and its files: the CSP above, and `X-Frame-Options`
/// for browsers that don't read `frame-ancestors`.
const NO_FRAMING: [(header::HeaderName, &str); 2] = [
    (header::CONTENT_SECURITY_POLICY, CSP),
    (header::X_FRAME_OPTIONS, "DENY"),
];

async fn page() -> Response {
    match web_file("index.html") {
        Some(body) => (
            [
                (header::CONTENT_TYPE, "text/html; charset=utf-8"),
                (header::CACHE_CONTROL, "no-cache"),
            ],
            NO_FRAMING,
            body,
        )
            .into_response(),
        None => (
            StatusCode::INTERNAL_SERVER_ERROR,
            "the console page is missing (CONSOLE_PAGE)",
        )
            .into_response(),
    }
}

async fn topology(AxumState(app): AxumState<Arc<App>>) -> Response {
    no_store(axum::Json(app.poller.topology_json()))
}

async fn state(
    AxumState(app): AxumState<Arc<App>>,
    axum::extract::Query(q): axum::extract::Query<HashMap<String, String>>,
) -> Response {
    let since = q.get("since").and_then(|v| v.parse().ok());
    let epoch = q.get("epoch").and_then(|v| v.parse().ok());
    let mut state = app.delta.answer(app.poller.snapshot_json(), since, epoch);
    add_account_ages(&mut state, tinytrader_observe::unix_seconds());
    no_store(axum::Json(state))
}

/// Adds `age_ms` to each account a service serves (`services.<id>.accounts.accounts[]`):
/// this server's clock now minus the account's `updated_at` (its last successful
/// poll), in whole milliseconds, or null without one. Computed here, so the
/// age does not depend on the clock of the device reading it.
fn add_account_ages(state: &mut serde_json::Value, now: f64) {
    let Some(services) = state.get_mut("services").and_then(|s| s.as_object_mut()) else {
        return;
    };
    for svc in services.values_mut() {
        let Some(accounts) = svc
            .pointer_mut("/accounts/accounts")
            .and_then(|a| a.as_array_mut())
        else {
            continue;
        };
        for account in accounts {
            let age = account
                .get("updated_at")
                .and_then(|t| t.as_f64())
                .map(|at| ((now - at) * 1000.0).max(0.0).round() as i64);
            if let Some(obj) = account.as_object_mut() {
                obj.insert("age_ms".into(), json!(age));
            }
        }
    }
}

/// A fleet lookup's failure as the API's error document: `404` for a stream
/// the service doesn't declare, `502` for one that doesn't answer.
fn fleet_error(e: &EventsError) -> Response {
    let code = match e {
        EventsError::Unreachable(_) => StatusCode::BAD_GATEWAY,
        EventsError::NotDeclared(_) | EventsError::NotServed(..) => StatusCode::NOT_FOUND,
    };
    (code, axum::Json(json!({ "error": e.to_string() }))).into_response()
}

/// Live tail of a service's events, forwarded from the service.
async fn events(
    AxumState(app): AxumState<Arc<App>>,
    axum::extract::Path(service): axum::extract::Path<String>,
    axum::extract::Query(q): axum::extract::Query<HashMap<String, String>>,
) -> Response {
    let after = q.get("after").and_then(|v| v.parse().ok()).unwrap_or(0);
    let limit = q.get("limit").and_then(|v| v.parse().ok()).unwrap_or(100);
    match app.poller.events_json(&service, after, limit).await {
        Ok(v) => no_store(axum::Json(v)),
        Err(e) => fleet_error(&e),
    }
}

/// One symbol's one-minute bars from a relay (`symbol=SPY&days=2`).
async fn bars(
    AxumState(app): AxumState<Arc<App>>,
    axum::extract::Path(service): axum::extract::Path<String>,
    axum::extract::RawQuery(query): axum::extract::RawQuery,
) -> Response {
    match app
        .poller
        .bars_json(&service, query.as_deref().unwrap_or(""))
        .await
    {
        Ok((code, v)) => no_store((
            StatusCode::from_u16(code).unwrap_or(StatusCode::BAD_GATEWAY),
            axum::Json(v),
        )),
        Err(e) => fleet_error(&e),
    }
}

/// A service's on-demand data (stock snapshots, option chains, bars).
async fn stream(
    AxumState(app): AxumState<Arc<App>>,
    axum::extract::Path((service, stream)): axum::extract::Path<(String, String)>,
    axum::extract::RawQuery(query): axum::extract::RawQuery,
) -> Response {
    match app
        .poller
        .stream_json(&service, &stream, query.as_deref().unwrap_or(""))
        .await
    {
        Ok((code, v)) => no_store((
            StatusCode::from_u16(code).unwrap_or(StatusCode::BAD_GATEWAY),
            axum::Json(v),
        )),
        Err(e) => fleet_error(&e),
    }
}

async fn list_collections(AxumState(app): AxumState<Arc<App>>) -> Response {
    let list: Vec<serde_json::Value> = app
        .collections
        .iter()
        .map(collections::Collection::index)
        .collect();
    no_store(axum::Json(list))
}

async fn collection_doc(
    AxumState(app): AxumState<Arc<App>>,
    axum::extract::Path((id, index)): axum::extract::Path<(String, usize)>,
) -> Response {
    app.collections
        .iter()
        .find(|c| c.id == id)
        .and_then(|c| c.render(index))
        .map_or_else(
            || {
                (
                    StatusCode::NOT_FOUND,
                    axum::Json(json!({ "error": "no such document" })),
                )
                    .into_response()
            },
            |d| no_store(axum::Json(d)),
        )
}

async fn prefs_get(AxumState(app): AxumState<Arc<App>>) -> Response {
    match &app.prefs {
        Some(p) => no_store(axum::Json(p.get())),
        None => (StatusCode::NOT_FOUND, "no settings file configured").into_response(),
    }
}

/// Save one setting. `If-Match` names the version the page started from;
/// a stale save gets 409 with the current value, to merge and save again.
async fn prefs_put(
    AxumState(app): AxumState<Arc<App>>,
    axum::extract::Path(key): axum::extract::Path<String>,
    headers: axum::http::HeaderMap,
    axum::Json(value): axum::Json<serde_json::Value>,
) -> Response {
    let Some(p) = &app.prefs else {
        return (StatusCode::NOT_FOUND, "no settings file configured").into_response();
    };
    let based_on = headers
        .get(axum::http::header::IF_MATCH)
        .and_then(|v| v.to_str().ok())
        .map(|v| v.trim_matches('"').to_owned());
    // A page from before versions (an old tab) can't overwrite: reload it.
    let Some(based_on) = based_on else {
        return (
            StatusCode::PRECONDITION_REQUIRED,
            axum::Json(json!({ "error": "save with If-Match (reload the page)" })),
        )
            .into_response();
    };
    match p.put(&key, value, Some(&based_on)) {
        Ok(()) => no_store(axum::Json(json!({ "ok": true }))),
        Err(prefs::Refused::Stale(current)) => (
            StatusCode::CONFLICT,
            axum::Json(json!({ "error": "changed since", "current": current })),
        )
            .into_response(),
        Err(prefs::Refused::Bad(e)) => {
            (StatusCode::BAD_REQUEST, axum::Json(json!({ "error": e }))).into_response()
        }
    }
}

/// The entries whose day is in `from..=to`, newest first.
async fn journal_get(
    AxumState(app): AxumState<Arc<App>>,
    axum::extract::Query(q): axum::extract::Query<HashMap<String, String>>,
) -> Response {
    let Some(j) = &app.journal else {
        return (StatusCode::NOT_FOUND, "no journal configured").into_response();
    };
    let entries = j.list(
        q.get("from").map(String::as_str),
        q.get("to").map(String::as_str),
    );
    no_store(axum::Json(json!({ "entries": entries })))
}

/// Journal writes answer with the entry, or the error.
fn journal_answer(r: Result<journal::Entry, journal::Refused>) -> Response {
    match r {
        Ok(e) => no_store(axum::Json(e)),
        Err(journal::Refused::NotFound) => (
            StatusCode::NOT_FOUND,
            axum::Json(json!({ "error": "no such entry" })),
        )
            .into_response(),
        Err(journal::Refused::Bad(e)) => {
            (StatusCode::BAD_REQUEST, axum::Json(json!({ "error": e }))).into_response()
        }
    }
}

async fn journal_post(
    AxumState(app): AxumState<Arc<App>>,
    axum::Json(body): axum::Json<serde_json::Value>,
) -> Response {
    match &app.journal {
        Some(j) => journal_answer(j.create(&body)),
        None => (StatusCode::NOT_FOUND, "no journal configured").into_response(),
    }
}

async fn journal_put(
    AxumState(app): AxumState<Arc<App>>,
    axum::extract::Path(id): axum::extract::Path<String>,
    axum::Json(body): axum::Json<serde_json::Value>,
) -> Response {
    match &app.journal {
        Some(j) => journal_answer(j.update(&id, &body)),
        None => (StatusCode::NOT_FOUND, "no journal configured").into_response(),
    }
}

async fn journal_delete(
    AxumState(app): AxumState<Arc<App>>,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> Response {
    match &app.journal {
        Some(j) => match j.delete(&id) {
            Ok(()) => no_store(axum::Json(json!({ "ok": true }))),
            Err(e) => journal_answer(Err(e)),
        },
        None => (StatusCode::NOT_FOUND, "no journal configured").into_response(),
    }
}

async fn reminders_get(AxumState(app): AxumState<Arc<App>>) -> Response {
    match &app.reminders {
        Some(r) => no_store(axum::Json(r.get())),
        None => (StatusCode::NOT_FOUND, "no reminders configured").into_response(),
    }
}

async fn reminders_post(
    AxumState(app): AxumState<Arc<App>>,
    axum::Json(change): axum::Json<serde_json::Value>,
) -> Response {
    let Some(r) = &app.reminders else {
        return (StatusCode::NOT_FOUND, "no reminders configured").into_response();
    };
    match r.apply(&change) {
        Ok(doc) => no_store(axum::Json(doc)),
        Err(e) => (StatusCode::BAD_REQUEST, axum::Json(json!({ "error": e }))).into_response(),
    }
}

async fn profiles_list(AxumState(app): AxumState<Arc<App>>) -> Response {
    match &app.profiles {
        Some(p) => no_store(axum::Json(p.list())),
        None => (StatusCode::NOT_FOUND, "no profiles directory configured").into_response(),
    }
}

async fn profiles_post(AxumState(app): AxumState<Arc<App>>, body: axum::body::Bytes) -> Response {
    let Some(p) = &app.profiles else {
        return (StatusCode::NOT_FOUND, "no profiles directory configured").into_response();
    };
    match p.store(&body) {
        Ok(id) => no_store(axum::Json(json!({ "id": id }))),
        Err(e) => (StatusCode::BAD_REQUEST, axum::Json(json!({ "error": e }))).into_response(),
    }
}

async fn profiles_get(
    AxumState(app): AxumState<Arc<App>>,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> Response {
    match app.profiles.as_ref().and_then(|p| p.get(&id)) {
        Some(b) => no_store(([(header::CONTENT_TYPE, "application/json")], b)),
        None => (StatusCode::NOT_FOUND, "no such recording").into_response(),
    }
}

/// Pass a request to the jobs service and its answer back.
async fn jobs_proxy(
    AxumState(app): AxumState<Arc<App>>,
    method: axum::http::Method,
    axum::extract::Path(rest): axum::extract::Path<String>,
    axum::extract::RawQuery(query): axum::extract::RawQuery,
    body: axum::body::Bytes,
) -> Response {
    pass(
        app.jobs.as_ref(),
        "jobs service",
        method,
        &rest,
        query,
        body,
    )
    .await
}

/// Pass a request to the job queue and its answer back.
async fn queue_proxy(
    AxumState(app): AxumState<Arc<App>>,
    method: axum::http::Method,
    axum::extract::Path(rest): axum::extract::Path<String>,
    axum::extract::RawQuery(query): axum::extract::RawQuery,
    body: axum::body::Bytes,
) -> Response {
    pass(app.queue.as_ref(), "job queue", method, &rest, query, body).await
}

async fn pass(
    to: Option<&(String, reqwest::Client)>,
    what: &str,
    method: axum::http::Method,
    rest: &str,
    query: Option<String>,
    body: axum::body::Bytes,
) -> Response {
    let Some((base, http)) = to else {
        return (
            StatusCode::NOT_FOUND,
            axum::Json(json!({ "error": format!("no {what} configured") })),
        )
            .into_response();
    };
    let url = match query {
        Some(q) => format!("{base}/{rest}?{q}"),
        None => format!("{base}/{rest}"),
    };
    let req = match method {
        axum::http::Method::POST | axum::http::Method::PATCH => http
            .request(method, url)
            .header(header::CONTENT_TYPE, "application/json")
            .body(body),
        _ => http.get(url),
    };
    match req.timeout(Duration::from_secs(120)).send().await {
        Ok(r) => {
            let code = StatusCode::from_u16(r.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
            let bytes = r.bytes().await.unwrap_or_default();
            no_store((code, [(header::CONTENT_TYPE, "application/json")], bytes))
        }
        Err(e) => (
            StatusCode::BAD_GATEWAY,
            axum::Json(json!({ "error": format!("{what}: {e}") })),
        )
            .into_response(),
    }
}

/// Approve a drive for the store: the one store call the console passes on.
async fn store_approve(AxumState(app): AxumState<Arc<App>>, body: axum::body::Bytes) -> Response {
    let Some(base) = &app.store else {
        return (
            StatusCode::NOT_FOUND,
            axum::Json(json!({ "error": "no store configured (CONSOLE_STORE)" })),
        )
            .into_response();
    };
    let mut req = reqwest::Client::new().post(format!("{base}/volumes/approve"));
    if let Some(t) = &app.store_token {
        req = req.bearer_auth(t);
    }
    let sent = req
        .header(header::CONTENT_TYPE, "application/json")
        .body(body)
        .timeout(Duration::from_secs(30))
        .send()
        .await;
    match sent {
        Ok(r) => {
            let code = StatusCode::from_u16(r.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
            let bytes = r.bytes().await.unwrap_or_default();
            no_store((code, [(header::CONTENT_TYPE, "application/json")], bytes))
        }
        Err(e) => (
            StatusCode::BAD_GATEWAY,
            axum::Json(json!({ "error": format!("store: {e}") })),
        )
            .into_response(),
    }
}

/// Which accounts have a gateway, and each gateway's state (from its welcome).
async fn gateways_list(AxumState(app): AxumState<Arc<App>>) -> Response {
    let mut out = Vec::new();
    for account in app.gateways.addrs.keys() {
        let state = match app
            .gateways
            .call(account, None, Duration::from_secs(5))
            .await
        {
            Ok((w, _)) => {
                json!({ "account": account, "state": w["state"], "environment": w["environment"] })
            }
            Err(e) => json!({ "account": account, "error": e }),
        };
        out.push(state);
    }
    no_store(axum::Json(json!({ "gateways": out })))
}

/// For the Spread builder: whether the console can trade, and each paper
/// account's gateway state with the console's own open orders (`tt-`).
async fn orders_list(AxumState(app): AxumState<Arc<App>>) -> Response {
    let can = app.gateways.can_trade();
    let mut out = Vec::new();
    for account in app.gateways.addrs.keys() {
        let who = if can {
            gateways::As::Trade
        } else {
            gateways::As::Ops
        };
        let req = can.then(|| json!({ "type": "open_orders" }));
        out.push(match app.gateways.call_as(who, account, req, Duration::from_secs(5)).await {
            Ok((w, r)) => {
                let mine: Vec<serde_json::Value> = r
                    .as_ref()
                    .and_then(|r| r["orders"].as_array())
                    .into_iter()
                    .flatten()
                    .filter(|o| o["client_order_id"].as_str().is_some_and(|i| i.starts_with(app.gateways.prefix.as_str())))
                    .map(|o| json!({ "client_order_id": o["client_order_id"], "status": o["status"], "filled_qty": o["filled_qty"], "avg_price": o["avg_price"], "legs": o["legs"], "limit_price": o["broker_raw"]["limit_price"], "qty": o["broker_raw"]["qty"], "submitted_at": o["broker_raw"]["submitted_at"] }))
                    .collect();
                // The gateway's count of orders that have not resolved (new opens are blocked); null when it doesn't say.
                let unresolved = r.as_ref().map_or(serde_json::Value::Null, |r| r["unresolved"].clone());
                json!({ "account": account, "state": w["state"], "environment": w["environment"], "broker": w["broker"], "unresolved": unresolved, "orders": mine })
            }
            Err(e) => json!({ "account": account, "error": e }),
        });
    }
    no_store(axum::Json(
        json!({ "trading": can, "prefix": app.gateways.prefix, "accounts": out }),
    ))
}

/// Every route of the console's own: the page, its reads, and the writes
/// behind the [`WriteGate`].
fn routes(write_gate: WriteGate) -> Router<Arc<App>> {
    Router::new()
        .route("/", get(page))
        .route("/docs", get(page))
        .route("/docs/", get(page))
        .route("/vendor/{file}", get(file))
        .route("/lib/{file}", get(file))
        .route("/widgets/{file}", get(file))
        .route("/api/topology", get(topology))
        .route("/api/state", get(state))
        .route("/api/events/{service}", get(events))
        .route("/api/bars/{service}", get(bars))
        .route("/api/stream/{service}/{stream}", get(stream))
        .route("/api/collections", get(list_collections))
        .route("/api/collections/{id}/{index}", get(collection_doc))
        .route("/api/jobs/{*rest}", get(jobs_proxy))
        .route("/api/queue/{*rest}", get(queue_proxy))
        .route("/api/reminders", get(reminders_get))
        .route("/api/prefs", get(prefs_get))
        .route("/api/journal", get(journal_get))
        .route("/api/profiles", get(profiles_list))
        .route("/api/profiles/{id}", get(profiles_get))
        .route("/api/accounts/gateways", get(gateways_list))
        .route("/api/orders", get(orders_list))
        .route("/api/write-gate", get(write_gate_get))
        // Every write, including the orders, behind WriteGate.
        .merge(write_routes(write_gate))
        // The console's writes (jobs, the queue, drive approval, reminders, flatten) can't be
        // forged by another page open in the browser.
        .layer(axum::middleware::from_fn(
            tinytrader_observe::guard::json_writes,
        ))
}

/// Who may write. Every write route (see [`write_routes`]) is checked here
/// before anything is forwarded or sent to a gateway:
///
/// - the Host: the request's `Host` must be one of `CONSOLE_HOSTS`
///   (comma-separated host names, each with or without a port; without one,
///   any port matches). This stops a page that rebinds its own DNS name to
///   the console's address from writing. Empty or unset refuses every write.
/// - the Tailscale login: `Tailscale-User-Login` must be one of
///   `CONSOLE_ORDER_LOGINS` (comma-separated, exact match). Empty or unset
///   refuses every write.
///
/// The login header is trusted only because `tailscale serve` removes any
/// copy a client sends and sets its own. Anything that reaches the console's
/// port without going through `tailscale serve` can send any login it likes:
/// a process on the host, or another container on the same Docker network.
/// So the port must stay on loopback and off any shared network.
///
/// A live account's order, cancel, flatten or resume also needs the order
/// token: `X-Order-Token` must equal `CONSOLE_ORDER_TOKEN` (or the file
/// `CONSOLE_ORDER_TOKEN_FILE` names). So does a paper account's flatten or
/// resume, unless `CONSOLE_TOKEN_FOR_PAPER_FLATTEN` is `false` (the default
/// is `true`). Paper orders and cancels need only the login.
/// Unset, those writes are refused with a message saying so. Whether an account
/// is live is decided by its handler (see [`account_live`]) from the accounts
/// service's environment and the gateway's own welcome, never from the request:
/// either one not saying `paper` makes it live.
///
/// GET routes stay open. A refusal says which check failed in plain words
/// (403 for the host or the login, 401 for the token), never what was
/// expected, and carries `X-Write-Gate: refused` and `"gate": true`, so the
/// page can tell it from a refusal by a service behind the console.
/// `GET /api/write-gate` tells the page whether writes are on, whether this
/// viewer's request may make them, and whether live orders are on.
#[derive(Clone)]
struct WriteGate {
    logins: Vec<String>,
    /// The allowed Host names, lowercased, each with its port if one was given.
    hosts: Vec<String>,
    token: Option<String>,
    /// Whether a paper account's flatten or resume needs the order token.
    paper_flatten_token: bool,
}

/// A refusal: a status and the message to send.
type Refusal = (StatusCode, &'static str);

/// The refusal for every write while `CONSOLE_ORDER_LOGINS` is empty.
const NO_LOGINS: &str =
    "writes are disabled: the console has no allowed logins configured (CONSOLE_ORDER_LOGINS)";
/// The refusal for every write while `CONSOLE_HOSTS` is empty.
const NO_HOSTS: &str =
    "writes are disabled: the console has no allowed host names configured (CONSOLE_HOSTS)";
/// The refusal for a request whose Host isn't on the list (or has none).
const BAD_HOST: &str = "writes are not allowed through this host name";
/// The refusal for a login that isn't on the list (or no login at all).
const NOT_ALLOWED: &str = "writes are not allowed for this login";
/// The refusal for a write whose order token is missing or wrong.
const BAD_TOKEN: &str = "this write needs the console's order token";
/// The refusal for a write that needs the token while none is configured.
const NO_TOKEN: &str = "this write needs the order token, and the console has none configured";

/// The response header that marks the gate's own refusals.
const GATE_HEADER: &str = "x-write-gate";

impl WriteGate {
    fn from_env() -> Self {
        let list = |var: &str| -> Vec<String> {
            env(var, "")
                .split(',')
                .map(str::trim)
                .filter(|l| !l.is_empty())
                .map(str::to_owned)
                .collect()
        };
        let logins = list("CONSOLE_ORDER_LOGINS");
        let hosts = list("CONSOLE_HOSTS")
            .iter()
            .filter_map(|h| normal_host(h))
            .collect();
        // An unreadable token file leaves live orders disabled; the console still starts.
        let token = match tinytrader_observe::optional_secret("CONSOLE_ORDER_TOKEN") {
            Ok(t) => t,
            Err(e) => {
                eprintln!("console: {e}");
                None
            }
        };
        let paper_flatten_token = !matches!(
            env("CONSOLE_TOKEN_FOR_PAPER_FLATTEN", "true")
                .trim()
                .to_ascii_lowercase()
                .as_str(),
            "false" | "0" | "no" | "off"
        );
        Self {
            logins,
            hosts,
            token,
            paper_flatten_token,
        }
    }

    /// Whether `login` is one of the allowed Tailscale logins.
    fn login_allowed(&self, login: Option<&str>) -> bool {
        login.is_some_and(|l| self.logins.iter().any(|a| a == l))
    }

    /// Whether the request's `host` (its `Host` header) is one of the
    /// allowed names: an entry with a port must match it exactly, one
    /// without matches the name on any port.
    fn host_allowed(&self, host: Option<&str>) -> bool {
        let Some(host) = host.and_then(normal_host) else {
            return false;
        };
        let name = host_name(&host);
        self.hosts
            .iter()
            .any(|a| *a == host || (host_name(a) == a.as_str() && a == name))
    }

    /// Why a write is refused, if it is, from its Host and login, before the
    /// token is looked at. No logins or no hosts configured refuses every
    /// write, and says so.
    fn refuse_request(&self, host: Option<&str>, login: Option<&str>) -> Option<Refusal> {
        if self.logins.is_empty() {
            Some((StatusCode::FORBIDDEN, NO_LOGINS))
        } else if self.hosts.is_empty() {
            Some((StatusCode::FORBIDDEN, NO_HOSTS))
        } else if !self.host_allowed(host) {
            Some((StatusCode::FORBIDDEN, BAD_HOST))
        } else {
            self.refuse_login(login)
        }
    }

    /// Why a write from `login` is refused, if it is.
    fn refuse_login(&self, login: Option<&str>) -> Option<Refusal> {
        if self.logins.is_empty() {
            Some((StatusCode::FORBIDDEN, NO_LOGINS))
        } else if self.login_allowed(login) {
            None
        } else {
            Some((StatusCode::FORBIDDEN, NOT_ALLOWED))
        }
    }

    /// Whether a write from `login`, carrying `token`, may go ahead, when it
    /// needs the order token (`needs_token`: a live account's write, or a
    /// flatten or resume) or not. Any other write needs only the login.
    fn decide(
        &self,
        login: Option<&str>,
        token: Option<&str>,
        needs_token: bool,
    ) -> Option<Refusal> {
        if let Some(refusal) = self.refuse_login(login) {
            return Some(refusal);
        }
        if !needs_token {
            return None;
        }
        let Some(want) = self.token.as_deref() else {
            return Some((StatusCode::FORBIDDEN, NO_TOKEN));
        };
        if constant_time_eq(token.unwrap_or("").as_bytes(), want.as_bytes()) {
            None
        } else {
            Some((StatusCode::UNAUTHORIZED, BAD_TOKEN))
        }
    }

    /// Whether a flatten or resume on an account that is `live` or not needs
    /// the order token.
    fn flatten_needs_token(&self, live: bool) -> bool {
        live || self.paper_flatten_token
    }
}

/// A Host as compared: trimmed, lowercased, without a trailing dot on the
/// name; `None` when empty.
fn normal_host(host: &str) -> Option<String> {
    let h = host.trim().to_ascii_lowercase();
    let name = host_name(&h);
    let port = &h[name.len()..];
    let name = name.strip_suffix('.').unwrap_or(name);
    (!name.is_empty()).then(|| format!("{name}{port}"))
}

/// The name part of a Host, without its port (`[::1]:8080` gives `[::1]`).
fn host_name(host: &str) -> &str {
    if host.starts_with('[') {
        return host.find(']').map_or(host, |i| &host[..=i]);
    }
    match host.rsplit_once(':') {
        Some((name, port)) if !port.is_empty() && port.bytes().all(|b| b.is_ascii_digit()) => name,
        _ => host,
    }
}

/// The request's Host: its `Host` header, else the URI's authority (HTTP/2).
fn request_host<'a>(
    headers: &'a axum::http::HeaderMap,
    uri: &'a axum::http::Uri,
) -> Option<&'a str> {
    headers
        .get(header::HOST)
        .and_then(|v| v.to_str().ok())
        .or_else(|| uri.authority().map(axum::http::uri::Authority::as_str))
}

/// The request's Tailscale login and order token, if sent.
fn login_and_token(headers: &axum::http::HeaderMap) -> (Option<&str>, Option<&str>) {
    (
        headers
            .get("tailscale-user-login")
            .and_then(|v| v.to_str().ok()),
        headers.get("x-order-token").and_then(|v| v.to_str().ok()),
    )
}

/// The refusal response for a write that needs the order token or not (as
/// its handler found it), if any.
fn refused_for(app: &App, headers: &axum::http::HeaderMap, needs_token: bool) -> Option<Response> {
    let (login, token) = login_and_token(headers);
    app.gate.decide(login, token, needs_token).map(refusal)
}

/// The gate's refusal, marked as the gate's (header and body) so the page
/// can tell it from a service's own 401 or 403.
fn refusal((status, why): Refusal) -> Response {
    (
        status,
        [(GATE_HEADER, "refused")],
        axum::Json(json!({ "error": why, "gate": true })),
    )
        .into_response()
}

/// The write gate as this viewer meets it: `writes` (any logins configured),
/// `hosts` (any host names configured), `allowed` (this request's Host and
/// login may write), `live_orders` (an order token is configured). Never the
/// logins, the host names or the token themselves.
async fn write_gate_get(
    AxumState(app): AxumState<Arc<App>>,
    uri: axum::http::Uri,
    headers: axum::http::HeaderMap,
) -> Response {
    let (login, _) = login_and_token(&headers);
    let host = request_host(&headers, &uri);
    no_store(axum::Json(json!({
        "writes": !app.gate.logins.is_empty(),
        "hosts": !app.gate.hosts.is_empty(),
        "allowed": app.gate.refuse_request(host, login).is_none(),
        "live_orders": app.gate.token.is_some(),
    })))
}

/// Compare without stopping at the first difference (the time taken says
/// nothing about how much of a guess was right). Both sides are hashed first,
/// so neither does the length of the guess.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    use sha2::{Digest, Sha256};
    let (a, b) = (Sha256::digest(a), Sha256::digest(b));
    a.iter().zip(b.iter()).fold(0u8, |d, (x, y)| d | (x ^ y)) == 0
}

/// Middleware: refuse a write through a Host or from a login that isn't
/// allowed. The order token
/// for a live account is checked by its handler, which knows the account.
async fn write_gate(
    AxumState(gate): AxumState<WriteGate>,
    req: axum::extract::Request,
    next: axum::middleware::Next,
) -> Response {
    let (login, _) = login_and_token(req.headers());
    match gate.refuse_request(request_host(req.headers(), req.uri()), login) {
        Some(r) => refusal(r),
        None => next.run(req).await,
    }
}

/// The write routes, each behind the [`WriteGate`]: orders (submit, cancel,
/// flatten, resume), the jobs service and the queue (POST, and PATCH for the
/// queue), drive approval, reminders, profile recordings and settings (PUT).
/// Their GET routes are merged in separately and stay open.
fn write_routes(gate: WriteGate) -> Router<Arc<App>> {
    Router::new()
        .route("/api/orders/{account}", axum::routing::post(order_submit))
        .route(
            "/api/orders/{account}/{id}/cancel",
            axum::routing::post(order_cancel),
        )
        .route(
            "/api/accounts/{account}/flatten",
            axum::routing::post(account_flatten),
        )
        .route(
            "/api/accounts/{account}/resume",
            axum::routing::post(account_resume),
        )
        .route("/api/jobs/{*rest}", axum::routing::post(jobs_proxy))
        .route(
            "/api/queue/{*rest}",
            axum::routing::post(queue_proxy).patch(queue_proxy),
        )
        .route(
            "/api/store/volumes/approve",
            axum::routing::post(store_approve),
        )
        .route("/api/reminders", axum::routing::post(reminders_post))
        .route("/api/prefs/{key}", axum::routing::put(prefs_put))
        .route("/api/journal", axum::routing::post(journal_post))
        .route(
            "/api/journal/{id}",
            axum::routing::put(journal_put).delete(journal_delete),
        )
        .route(
            "/api/profiles",
            axum::routing::post(profiles_post).layer(axum::extract::DefaultBodyLimit::max(
                profiles::MAX_BYTES + 1024,
            )),
        )
        .route_layer(axum::middleware::from_fn_with_state(gate, write_gate))
}

/// Send one of the Spread builder's orders to a paper account's gateway as
/// the console's trade client. The body names the account again in
/// `confirm`; the id is made here. The gateway's answer is returned as is
/// (`accepted` is not a fill).
async fn order_submit(
    AxumState(app): AxumState<Arc<App>>,
    axum::extract::Path(account): axum::extract::Path<String>,
    headers: axum::http::HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    let ask: serde_json::Value = serde_json::from_slice(&body).unwrap_or_default();
    if ask["confirm"].as_str() != Some(account.as_str()) {
        return (
            StatusCode::BAD_REQUEST,
            axum::Json(json!({ "error": "confirm the account" })),
        )
            .into_response();
    }
    if !app.gateways.addrs.contains_key(&account) {
        return no_gateway();
    }
    let live = match account_live(&app, &account, gateways::As::Trade).await {
        Ok(live) => live,
        Err(e) => return gateway_failed(e),
    };
    if let Some(refusal) = refused_for(&app, &headers, live) {
        return refusal;
    }
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let id = format!(
        "{}{}-{:05}",
        app.gateways.prefix,
        now.as_secs(),
        now.subsec_nanos() % 100_000
    );
    // A live account's order also says so, from the ticket's typed confirmation.
    if ask["live"] != true && live {
        return (
            StatusCode::BAD_REQUEST,
            axum::Json(json!({ "error": "a live account's order must be confirmed as live" })),
        )
            .into_response();
    }
    let intent = match gateways::intent(&ask["order"], &id) {
        Ok(i) => i,
        Err(e) => {
            return (StatusCode::BAD_REQUEST, axum::Json(json!({ "error": e }))).into_response();
        }
    };
    trade_request(
        &app,
        &account,
        json!({ "type": "submit", "intent": intent }),
        30,
    )
    .await
}

/// Cancel one of the console's orders.
async fn order_cancel(
    AxumState(app): AxumState<Arc<App>>,
    axum::extract::Path((account, id)): axum::extract::Path<(String, String)>,
    headers: axum::http::HeaderMap,
) -> Response {
    if !id.starts_with(app.gateways.prefix.as_str()) {
        return (
            StatusCode::BAD_REQUEST,
            axum::Json(json!({ "error": "not one of the console's orders" })),
        )
            .into_response();
    }
    if !app.gateways.addrs.contains_key(&account) {
        return no_gateway();
    }
    let live = match account_live(&app, &account, gateways::As::Trade).await {
        Ok(live) => live,
        Err(e) => return gateway_failed(e),
    };
    if let Some(refusal) = refused_for(&app, &headers, live) {
        return refusal;
    }
    trade_request(
        &app,
        &account,
        json!({ "type": "cancel", "client_order_id": id }),
        15,
    )
    .await
}

async fn trade_request(
    app: &App,
    account: &str,
    request: serde_json::Value,
    wait: u64,
) -> Response {
    if !app.gateways.addrs.contains_key(account) {
        return no_gateway();
    }
    match app
        .gateways
        .call_as(
            gateways::As::Trade,
            account,
            Some(request),
            Duration::from_secs(wait),
        )
        .await
    {
        Ok((_, Some(r))) => no_store(axum::Json(r)),
        Ok((_, None)) => (
            StatusCode::BAD_GATEWAY,
            axum::Json(json!({ "error": "no response" })),
        )
            .into_response(),
        Err(e) => (StatusCode::BAD_GATEWAY, axum::Json(json!({ "error": e }))).into_response(),
    }
}

/// Close every position of an account at market through its gateway: halts
/// opens, cancels open orders, liquidates. The body must name the account in
/// `confirm`. It needs the order token on a live account, and on a paper one
/// unless `CONSOLE_TOKEN_FOR_PAPER_FLATTEN` is `false`.
async fn account_flatten(
    AxumState(app): AxumState<Arc<App>>,
    axum::extract::Path(account): axum::extract::Path<String>,
    headers: axum::http::HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    let ask: serde_json::Value = serde_json::from_slice(&body).unwrap_or_default();
    if !confirmed(&ask, &account) {
        return (
            StatusCode::BAD_REQUEST,
            axum::Json(json!({ "error": "type the account's name to confirm" })),
        )
            .into_response();
    }
    if !app.gateways.addrs.contains_key(&account) {
        return no_gateway();
    }
    let live = match account_live(&app, &account, gateways::As::Ops).await {
        Ok(live) => live,
        Err(e) => return gateway_failed(e),
    };
    if let Some(refusal) = refused_for(&app, &headers, app.gate.flatten_needs_token(live)) {
        return refusal;
    }
    let reason = ask["reason"]
        .as_str()
        .filter(|r| !r.is_empty())
        .unwrap_or("flatten from the console");
    gateway_request(
        &app,
        &account,
        json!({ "type": "flatten", "reason": reason }),
        60,
    )
    .await
}

/// Allow opening orders again on an account (the gateway's `unkill`). A live
/// account's resume also needs `confirm` naming the account. The order token
/// is needed as for a flatten.
async fn account_resume(
    AxumState(app): AxumState<Arc<App>>,
    axum::extract::Path(account): axum::extract::Path<String>,
    headers: axum::http::HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    let ask: serde_json::Value = serde_json::from_slice(&body).unwrap_or_default();
    if !app.gateways.addrs.contains_key(&account) {
        return no_gateway();
    }
    let live = match account_live(&app, &account, gateways::As::Ops).await {
        Ok(live) => live,
        Err(e) => return gateway_failed(e),
    };
    if let Some(refusal) = refused_for(&app, &headers, app.gate.flatten_needs_token(live)) {
        return refusal;
    }
    if live && !confirmed(&ask, &account) {
        return (
            StatusCode::BAD_REQUEST,
            axum::Json(json!({ "error": "type the account's name to confirm" })),
        )
            .into_response();
    }
    let reason = ask["reason"]
        .as_str()
        .filter(|r| !r.is_empty())
        .unwrap_or("resumed from the console");
    gateway_request(
        &app,
        &account,
        json!({ "type": "unkill", "reason": reason }),
        15,
    )
    .await
}

fn no_gateway() -> Response {
    (
        StatusCode::NOT_FOUND,
        axum::Json(json!({ "error": "this account has no order gateway" })),
    )
        .into_response()
}

/// Whether the body's `confirm` names the account exactly.
fn confirmed(ask: &serde_json::Value, account: &str) -> bool {
    ask["confirm"].as_str() == Some(account)
}

/// The account's `environment` as the accounts service reports it
/// (`alpaca-account`, in each service's `accounts.accounts`, matched by
/// `name`), from the console's latest snapshot. Its value only when every
/// report agrees; `None` when no service reports the account, or they differ.
fn account_environment(app: &App, account: &str) -> Option<String> {
    let snapshot = app.poller.snapshot_json();
    let reports: Vec<Option<String>> = snapshot["services"]
        .as_object()
        .into_iter()
        .flat_map(|services| services.values())
        .filter_map(|svc| svc.pointer("/accounts/accounts")?.as_array())
        .flatten()
        .filter(|a| a["name"].as_str() == Some(account))
        .map(|a| a["environment"].as_str().map(str::to_owned))
        .collect();
    let first = reports.first()?;
    if reports.iter().all(|r| r == first) {
        first.clone()
    } else {
        None
    }
}

/// Whether an account trades live, from its accounts-service `environment`:
/// `paper` is paper; `live`, any other value, or none (unknown) is live.
fn is_live(environment: Option<&str>) -> bool {
    environment != Some("paper")
}

/// Whether a write on the account is a live one: the accounts service's
/// environment (see [`account_environment`]) or the gateway's welcome, as the
/// client `who` that will send it, not saying `paper`. Both must say `paper`
/// for a paper write, so a gateway mapped to the wrong account still needs the
/// token. A gateway that can't be reached is an error, not a guess.
async fn account_live(app: &App, account: &str, who: gateways::As) -> Result<bool, String> {
    if is_live(account_environment(app, account).as_deref()) {
        return Ok(true);
    }
    match app
        .gateways
        .call_as(who, account, None, Duration::from_secs(5))
        .await
    {
        Ok((welcome, _)) => Ok(is_live(welcome["environment"].as_str())),
        Err(e) => Err(e),
    }
}

/// A gateway that failed or couldn't be reached, as the API's error document.
fn gateway_failed(e: String) -> Response {
    (StatusCode::BAD_GATEWAY, axum::Json(json!({ "error": e }))).into_response()
}

async fn gateway_request(
    app: &App,
    account: &str,
    request: serde_json::Value,
    wait: u64,
) -> Response {
    if !app.gateways.addrs.contains_key(account) {
        return no_gateway();
    }
    match app
        .gateways
        .call(account, Some(request), Duration::from_secs(wait))
        .await
    {
        Ok((_, Some(r))) => no_store(axum::Json(r)),
        Ok((_, None)) => (
            StatusCode::BAD_GATEWAY,
            axum::Json(json!({ "error": "no response" })),
        )
            .into_response(),
        Err(e) => (StatusCode::BAD_GATEWAY, axum::Json(json!({ "error": e }))).into_response(),
    }
}

fn no_store(body: impl IntoResponse) -> Response {
    ([(header::CACHE_CONTROL, "no-store")], body).into_response()
}

fn env(key: &str, default: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    const ME: &str = "me@example.net";

    fn gate(token: Option<&str>) -> WriteGate {
        WriteGate {
            logins: vec![ME.to_owned()],
            hosts: vec!["127.0.0.1".to_owned()],
            token: token.map(str::to_owned),
            paper_flatten_token: true,
        }
    }

    #[test]
    fn tokens_compare_whole() {
        assert!(constant_time_eq(b"made-up", b"made-up"));
        assert!(!constant_time_eq(b"made-up", b"made-uq"));
        assert!(!constant_time_eq(b"made-up", b"made"));
        assert!(!constant_time_eq(b"", b"made-up"));
    }

    #[test]
    fn a_paper_write_needs_only_the_login() {
        let g = gate(Some("secret"));
        assert_eq!(g.decide(Some(ME), None, false), None);
        // A settings or reminder write is a paper-style write: the login is enough.
        let no_token = gate(None);
        assert_eq!(no_token.decide(Some(ME), None, false), None);
    }

    #[test]
    fn environment_live_needs_the_token() {
        let g = gate(Some("secret"));
        assert_eq!(
            g.decide(Some(ME), None, true),
            Some((StatusCode::UNAUTHORIZED, BAD_TOKEN))
        );
        assert_eq!(
            g.decide(Some(ME), Some("guess"), true),
            Some((StatusCode::UNAUTHORIZED, BAD_TOKEN))
        );
        assert_eq!(g.decide(Some(ME), Some("secret"), true), None);
    }

    #[test]
    fn environment_missing_is_treated_as_live() {
        let g = gate(Some("secret"));
        assert!(is_live(None));
        assert!(is_live(Some("live")));
        assert!(is_live(Some("sandbox")));
        assert!(!is_live(Some("paper")));
        assert_eq!(
            g.decide(Some(ME), None, is_live(None)),
            Some((StatusCode::UNAUTHORIZED, BAD_TOKEN))
        );
        assert_eq!(g.decide(Some(ME), Some("secret"), is_live(None)), None);
    }

    #[test]
    fn a_live_write_with_no_token_configured_says_so() {
        let g = gate(None);
        assert_eq!(
            g.decide(Some(ME), Some("anything"), true),
            Some((StatusCode::FORBIDDEN, NO_TOKEN,))
        );
    }

    #[test]
    fn a_live_flatten_needs_the_account_name_typed() {
        let g = gate(Some("secret"));
        // The token passes the gate; the typed confirmation is checked by the handler.
        assert_eq!(g.decide(Some(ME), Some("secret"), true), None);
        assert!(confirmed(&json!({ "confirm": "paper-1" }), "paper-1"));
        assert!(!confirmed(&json!({ "confirm": "paper-2" }), "paper-1"));
        assert!(!confirmed(&json!({ "confirm": "Paper-1" }), "paper-1"));
        assert!(!confirmed(&json!({}), "paper-1"));
    }

    #[test]
    fn an_empty_allowlist_refuses_all_writes() {
        let g = WriteGate {
            logins: Vec::new(),
            hosts: vec!["127.0.0.1".to_owned()],
            token: Some("secret".to_owned()),
            paper_flatten_token: true,
        };
        assert_eq!(
            g.decide(Some(ME), None, false),
            Some((StatusCode::FORBIDDEN, NO_LOGINS))
        );
        assert_eq!(
            g.decide(Some(ME), Some("secret"), true),
            Some((StatusCode::FORBIDDEN, NO_LOGINS))
        );
        assert_eq!(
            g.refuse_login(None),
            Some((StatusCode::FORBIDDEN, NO_LOGINS))
        );
    }

    #[test]
    fn a_wrong_or_missing_login_is_refused() {
        let g = gate(Some("secret"));
        assert_eq!(
            g.decide(None, Some("secret"), true),
            Some((StatusCode::FORBIDDEN, NOT_ALLOWED))
        );
        assert_eq!(
            g.decide(Some("other@example.net"), None, false),
            Some((StatusCode::FORBIDDEN, NOT_ALLOWED))
        );
        assert_eq!(
            g.decide(Some("other@example.net"), Some("secret"), true),
            Some((StatusCode::FORBIDDEN, NOT_ALLOWED))
        );
    }

    /// The console's routes on a free port, with no services configured.
    async fn serve(gate: WriteGate) -> String {
        serve_with(gate, None, gateways::Gateways::for_test(Default::default())).await
    }

    /// The console's routes on a free port, with `upstream` as both the jobs
    /// service and the queue (if given), and these gateways.
    async fn serve_with(
        gate: WriteGate,
        upstream: Option<String>,
        gateways: gateways::Gateways,
    ) -> String {
        let to = |u: &Option<String>| u.clone().map(|u| (u, reqwest::Client::new()));
        let app = Arc::new(App {
            poller: Poller::new(PollerConfig {
                topology: "/nonexistent/topology.json".to_owned(),
                docker: None,
                project: None,
            }),
            jobs: to(&upstream),
            queue: to(&upstream),
            store: None,
            store_token: None,
            prefs: None,
            reminders: None,
            journal: None,
            profiles: None,
            gateways,
            gate: gate.clone(),
            polls: AtomicU64::new(0),
            delta: delta::Delta::new(0),
            collections: Vec::new(),
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, routes(gate).with_state(app))
                .await
                .unwrap();
        });
        format!("http://{addr}")
    }

    /// A request's status and its `error`, if any.
    async fn ask(method: reqwest::Method, url: &str, login: Option<&str>) -> (u16, Option<String>) {
        let (status, error, _) = ask_full(method, url, login, &[], "{}").await;
        (status, error)
    }

    /// A request's status, its `error`, and whether the write gate marked
    /// it as its own refusal (header and body agree).
    async fn ask_full(
        method: reqwest::Method,
        url: &str,
        login: Option<&str>,
        headers: &[(&str, &str)],
        body: &str,
    ) -> (u16, Option<String>, bool) {
        let mut req = reqwest::Client::new()
            .request(method, url)
            .header("content-type", "application/json")
            .body(body.to_owned());
        if let Some(l) = login {
            req = req.header("tailscale-user-login", l);
        }
        for (k, v) in headers {
            req = req.header(*k, *v);
        }
        let r = req.send().await.unwrap();
        let status = r.status().as_u16();
        let marked = r.headers().get(GATE_HEADER).is_some_and(|v| v == "refused");
        let v: serde_json::Value = r.json().await.unwrap_or_default();
        assert_eq!(marked, v["gate"] == true, "header and body disagree");
        (status, v["error"].as_str().map(str::to_owned), marked)
    }

    /// Every write route (path, methods), as the page sends them.
    const WRITES: &[(&str, &[&str])] = &[
        ("/api/orders/paper-1", &["POST"]),
        ("/api/orders/paper-1/tt-1/cancel", &["POST"]),
        ("/api/accounts/paper-1/flatten", &["POST"]),
        ("/api/accounts/paper-1/resume", &["POST"]),
        ("/api/jobs/submit", &["POST"]),
        ("/api/queue/jobs", &["POST", "PATCH"]),
        ("/api/store/volumes/approve", &["POST"]),
        ("/api/reminders", &["POST"]),
        ("/api/prefs/tinytrader-theme", &["PUT"]),
        ("/api/journal", &["POST"]),
        ("/api/journal/j1", &["PUT", "DELETE"]),
        ("/api/profiles", &["POST"]),
    ];

    #[tokio::test]
    async fn with_no_allowlist_every_write_is_refused_and_reads_stay_open() {
        let base = serve(WriteGate {
            logins: Vec::new(),
            hosts: vec!["127.0.0.1".to_owned()],
            token: Some("made-up-token".to_owned()),
            paper_flatten_token: true,
        })
        .await;
        for (path, methods) in WRITES {
            for m in *methods {
                let method = reqwest::Method::from_bytes(m.as_bytes()).unwrap();
                let (status, error) = ask(method, &format!("{base}{path}"), Some(ME)).await;
                assert_eq!(status, 403, "{m} {path}");
                assert_eq!(error.as_deref(), Some(NO_LOGINS), "{m} {path}");
            }
        }
        // Reads are not gated: they answer as they would (404 here: nothing configured).
        for path in [
            "/api/journal",
            "/api/prefs",
            "/api/reminders",
            "/api/profiles",
        ] {
            let (status, _) = ask(reqwest::Method::GET, &format!("{base}{path}"), None).await;
            assert_eq!(status, 404, "GET {path}");
        }
        let r: serde_json::Value = reqwest::get(format!("{base}/api/write-gate"))
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(
            r,
            json!({ "writes": false, "hosts": true, "allowed": false, "live_orders": true })
        );
    }

    #[tokio::test]
    async fn every_write_route_needs_an_allowed_login() {
        let base = serve(gate(None)).await;
        for (path, methods) in WRITES {
            for m in *methods {
                let method = reqwest::Method::from_bytes(m.as_bytes()).unwrap();
                let url = format!("{base}{path}");
                for login in [None, Some("other@example.net")] {
                    let (status, error) = ask(method.clone(), &url, login).await;
                    assert_eq!(status, 403, "{m} {path} as {login:?}");
                    assert_eq!(error.as_deref(), Some(NOT_ALLOWED), "{m} {path}");
                }
                // An allowed login gets past the gate (to a 4xx from the route itself here).
                let (status, error) = ask(method, &url, Some(ME)).await;
                assert_ne!(error.as_deref(), Some(NOT_ALLOWED), "{m} {path}");
                assert_ne!(status, 403, "{m} {path}");
            }
        }
        let r: serde_json::Value = reqwest::Client::new()
            .get(format!("{base}/api/write-gate"))
            .header("tailscale-user-login", ME)
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(
            r,
            json!({ "writes": true, "hosts": true, "allowed": true, "live_orders": false })
        );
    }

    #[test]
    fn hosts_match_by_name_and_port() {
        let g = WriteGate {
            hosts: ["Console.Example.ts.net", "127.0.0.1:8080", "[::1]"]
                .iter()
                .filter_map(|h| normal_host(h))
                .collect(),
            ..gate(None)
        };
        for ok in [
            "console.example.ts.net",
            "console.example.ts.net:443",
            "CONSOLE.example.ts.net.",
            "127.0.0.1:8080",
            "[::1]:9000",
            "[::1]",
        ] {
            assert!(g.host_allowed(Some(ok)), "{ok}");
        }
        for bad in [
            "evil.example",
            "console.example.ts.net.evil.example",
            "127.0.0.1",
            "127.0.0.1:8081",
            "[::2]",
            "",
        ] {
            assert!(!g.host_allowed(Some(bad)), "{bad}");
        }
        assert!(!g.host_allowed(None));
        assert_eq!(
            g.refuse_request(Some("evil.example"), Some(ME)),
            Some((StatusCode::FORBIDDEN, BAD_HOST))
        );
        assert_eq!(g.refuse_request(Some("127.0.0.1:8080"), Some(ME)), None);
        assert_eq!(
            WriteGate {
                hosts: Vec::new(),
                ..gate(None)
            }
            .refuse_request(Some("127.0.0.1"), Some(ME)),
            Some((StatusCode::FORBIDDEN, NO_HOSTS))
        );
    }

    #[test]
    fn a_paper_flatten_needs_the_token_unless_turned_off() {
        let g = gate(Some("secret"));
        assert!(g.flatten_needs_token(false));
        assert_eq!(
            g.decide(Some(ME), None, g.flatten_needs_token(false)),
            Some((StatusCode::UNAUTHORIZED, BAD_TOKEN))
        );
        assert_eq!(
            g.decide(Some(ME), Some("secret"), g.flatten_needs_token(false)),
            None
        );
        let off = WriteGate {
            paper_flatten_token: false,
            ..gate(Some("secret"))
        };
        assert!(!off.flatten_needs_token(false));
        assert!(off.flatten_needs_token(true));
        assert_eq!(
            off.decide(Some(ME), None, off.flatten_needs_token(false)),
            None
        );
    }

    #[tokio::test]
    async fn writes_need_an_allowed_host() {
        // Reached as 127.0.0.1:<port>, which is not on this list.
        let base = serve(WriteGate {
            hosts: vec![
                "console.example.ts.net".to_owned(),
                "127.0.0.1:1".to_owned(),
            ],
            ..gate(None)
        })
        .await;
        let url = format!("{base}/api/reminders");
        let (status, error, marked) =
            ask_full(reqwest::Method::POST, &url, Some(ME), &[], "{}").await;
        assert_eq!(
            (status, error.as_deref(), marked),
            (403, Some(BAD_HOST), true)
        );
        let rebound = [("host", "attacker.example")];
        let (status, error, marked) =
            ask_full(reqwest::Method::POST, &url, Some(ME), &rebound, "{}").await;
        assert_eq!(
            (status, error.as_deref(), marked),
            (403, Some(BAD_HOST), true)
        );
        // The configured name, on any port, gets past the gate (to the route's own 404).
        let named = [("host", "console.example.ts.net:443")];
        let (status, _, marked) =
            ask_full(reqwest::Method::POST, &url, Some(ME), &named, "{}").await;
        assert_eq!((status, marked), (404, false));

        // No host names configured: every write is refused, and the page is told.
        let base = serve(WriteGate {
            hosts: Vec::new(),
            ..gate(None)
        })
        .await;
        for (path, methods) in WRITES {
            for m in *methods {
                let method = reqwest::Method::from_bytes(m.as_bytes()).unwrap();
                let (status, error, marked) =
                    ask_full(method, &format!("{base}{path}"), Some(ME), &[], "{}").await;
                assert_eq!(
                    (status, error.as_deref(), marked),
                    (403, Some(NO_HOSTS), true),
                    "{m} {path}"
                );
            }
        }
        let r: serde_json::Value = reqwest::Client::new()
            .get(format!("{base}/api/write-gate"))
            .header("tailscale-user-login", ME)
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(
            r,
            json!({ "writes": true, "hosts": false, "allowed": false, "live_orders": false })
        );
    }

    /// A stand-in for the jobs service and the queue: a GET answers with its
    /// path; any write is refused with the upstream's own 401.
    async fn upstream() -> String {
        let router = Router::new().fallback(
            |method: axum::http::Method, uri: axum::http::Uri| async move {
                if method == axum::http::Method::GET {
                    axum::Json(json!({ "from": "upstream", "path": uri.path() })).into_response()
                } else {
                    (
                        StatusCode::UNAUTHORIZED,
                        axum::Json(json!({ "error": "upstream says no" })),
                    )
                        .into_response()
                }
            },
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        format!("http://{addr}")
    }

    #[tokio::test]
    async fn job_and_queue_reads_reach_their_proxies_and_upstream_refusals_are_not_the_gates() {
        let up = upstream().await;
        let base = serve_with(
            gate(None),
            Some(up),
            gateways::Gateways::for_test(Default::default()),
        )
        .await;
        for path in ["jobs/list", "queue/jobs"] {
            // No login: reads are open.
            let r = reqwest::get(format!("{base}/api/{path}")).await.unwrap();
            assert_eq!(r.status().as_u16(), 200, "GET /api/{path}");
            let v: serde_json::Value = r.json().await.unwrap();
            assert_eq!(v["from"], "upstream", "GET /api/{path}");
            assert_eq!(v["path"], format!("/{}", path.split_once('/').unwrap().1));
        }
        // A write past the gate that the upstream refuses is not marked as the gate's.
        let (status, error, marked) = ask_full(
            reqwest::Method::POST,
            &format!("{base}/api/jobs/submit"),
            Some(ME),
            &[],
            "{}",
        )
        .await;
        assert_eq!(
            (status, error.as_deref(), marked),
            (401, Some("upstream says no"), false)
        );
        // The gate's own refusal is.
        let (status, _, marked) = ask_full(
            reqwest::Method::POST,
            &format!("{base}/api/jobs/submit"),
            None,
            &[],
            "{}",
        )
        .await;
        assert_eq!((status, marked), (403, true));
    }

    #[tokio::test]
    async fn a_live_order_without_the_token_is_refused_and_never_sent() {
        // A gateway that counts connections and answers nothing.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let seen = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let count = seen.clone();
        tokio::spawn(async move {
            while let Ok((conn, _)) = listener.accept().await {
                count.fetch_add(1, Ordering::SeqCst);
                drop(conn);
            }
        });
        let gateways =
            gateways::Gateways::for_test([("live-1".to_owned(), addr.to_string())].into());
        let base = serve_with(gate(Some("made-up-token")), None, gateways).await;
        // No accounts service reports live-1, so it counts as live.
        let order = json!({ "confirm": "live-1", "live": true, "order": {
            "qty": 1, "limit_price": 2.1, "legs": [{ "symbol": "XSP261016C00780000", "side": "buy" }]
        } })
        .to_string();
        let url = format!("{base}/api/orders/live-1");
        for token in [None, Some("guess")] {
            let headers: Vec<(&str, &str)> =
                token.map(|t| ("x-order-token", t)).into_iter().collect();
            let (status, error, marked) =
                ask_full(reqwest::Method::POST, &url, Some(ME), &headers, &order).await;
            assert_eq!(
                (status, error.as_deref(), marked),
                (401, Some(BAD_TOKEN), true),
                "{token:?}"
            );
        }
        let cancel = format!("{base}/api/orders/live-1/tt-1/cancel");
        let (status, _, marked) =
            ask_full(reqwest::Method::POST, &cancel, Some(ME), &[], "{}").await;
        assert_eq!((status, marked), (401, true));
        assert_eq!(
            seen.load(Ordering::SeqCst),
            0,
            "nothing reached the gateway"
        );
        // With the token, the same order is sent (the stand-in drops it: 502).
        let (status, _, marked) = ask_full(
            reqwest::Method::POST,
            &url,
            Some(ME),
            &[("x-order-token", "made-up-token")],
            &order,
        )
        .await;
        assert_eq!((status, marked), (502, false));
        assert_eq!(seen.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn the_page_and_its_files_cannot_be_framed() {
        let base = serve(gate(None)).await;
        for path in ["/", "/docs", "/lib/app.js", "/lib/api.js"] {
            let r = reqwest::get(format!("{base}{path}")).await.unwrap();
            assert_eq!(r.status().as_u16(), 200, "{path}");
            let h = r.headers();
            assert_eq!(h["content-security-policy"], CSP, "{path}");
            assert_eq!(h["x-frame-options"], "DENY", "{path}");
        }
    }
}

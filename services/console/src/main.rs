//! tinytrader console backend.
//!
//! Serves the console page and a small read-only API:
//!
//! - `GET /api/topology`: the deployment as drawn: hosts, services, stores,
//!   edges and panels.
//! - `GET /api/state`: the latest snapshot of every service: container state
//!   from the Docker Engine API, plus each service's `/health`, `/metrics`
//!   and optional `/recent` endpoints.
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
//! - `CONSOLE_JOBS`: the Databento jobs service, e.g. `http://databento-jobs:9121`
//!   (optional). The one exception to read-only: `/api/jobs/...` is passed to
//!   it, so the Data page can price, submit and download jobs. The jobs
//!   service itself insists on an exact confirmation before any charge.

mod collections;

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use axum::Router;
use axum::extract::State as AxumState;
use axum::http::{StatusCode, header};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::get;
use serde_json::json;
use tinytrader_fleet::{EventsError, Poller, PollerConfig};
use tinytrader_observe::{Observer, State};

const PAGE: &str = include_str!("../web/index.html");

struct App {
    poller: Poller,
    /// The jobs service's base URL, and a client to reach it.
    jobs: Option<(String, reqwest::Client)>,
    polls: AtomicU64,
    collections: Vec<collections::Collection>,
}

#[tokio::main]
async fn main() {
    let listen = tinytrader_observe::listen_address("CONSOLE_LISTEN", "127.0.0.1:8080");
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
        collections: collections::parse(&env("CONSOLE_COLLECTIONS", "")),
        jobs: std::env::var("CONSOLE_JOBS")
            .ok()
            .filter(|s| !s.is_empty())
            .map(|u| (u.trim_end_matches('/').to_owned(), reqwest::Client::new())),
    });

    let observer = Observer::new("console", env!("CARGO_PKG_VERSION"));
    observer.describe(json!({
        "role": "The operator console: polls every service and Docker every two seconds.",
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

    let router = Router::new()
        .route("/", get(page))
        .route("/api/topology", get(topology))
        .route("/api/state", get(state))
        .route("/api/events/{service}", get(events))
        .route("/api/collections", get(list_collections))
        .route("/api/collections/{id}/{index}", get(collection_doc))
        .route("/api/jobs/{*rest}", get(jobs_proxy).post(jobs_proxy))
        .with_state(app)
        .merge(observer.router());
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
    loop {
        tick.tick().await;
        let problem = app.poller.poll().await;
        app.polls.fetch_add(1, Ordering::Relaxed);
        match problem {
            None => observer.set_state(State::Ready, "polling"),
            Some(p) => {
                observer.record_error(&p);
                observer.set_state(State::Degraded, &p);
            }
        }
    }
}

async fn page() -> Html<&'static str> {
    Html(PAGE)
}

async fn topology(AxumState(app): AxumState<Arc<App>>) -> Response {
    no_store(axum::Json(app.poller.topology_json()))
}

async fn state(AxumState(app): AxumState<Arc<App>>) -> Response {
    no_store(axum::Json(app.poller.snapshot_json()))
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
        Err(e @ EventsError::NotDeclared(_)) => {
            (StatusCode::NOT_FOUND, e.to_string()).into_response()
        }
        Err(e @ EventsError::Unreachable(_)) => {
            (StatusCode::BAD_GATEWAY, e.to_string()).into_response()
        }
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
            || (StatusCode::NOT_FOUND, "no such document").into_response(),
            |d| no_store(axum::Json(d)),
        )
}

/// Pass a request to the jobs service and its answer back.
async fn jobs_proxy(
    AxumState(app): AxumState<Arc<App>>,
    method: axum::http::Method,
    axum::extract::Path(rest): axum::extract::Path<String>,
    axum::extract::RawQuery(query): axum::extract::RawQuery,
    body: axum::body::Bytes,
) -> Response {
    let Some((base, http)) = &app.jobs else {
        return (StatusCode::NOT_FOUND, "no jobs service configured").into_response();
    };
    let url = match query {
        Some(q) => format!("{base}/{rest}?{q}"),
        None => format!("{base}/{rest}"),
    };
    let req = if method == axum::http::Method::POST {
        http.post(url)
            .header(header::CONTENT_TYPE, "application/json")
            .body(body)
    } else {
        http.get(url)
    };
    match req.timeout(Duration::from_secs(120)).send().await {
        Ok(r) => {
            let code = StatusCode::from_u16(r.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
            let bytes = r.bytes().await.unwrap_or_default();
            no_store((code, [(header::CONTENT_TYPE, "application/json")], bytes))
        }
        Err(e) => (StatusCode::BAD_GATEWAY, format!("jobs service: {e}")).into_response(),
    }
}

fn no_store(body: impl IntoResponse) -> Response {
    ([(header::CACHE_CONTROL, "no-store")], body).into_response()
}

fn env(key: &str, default: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.to_owned())
}

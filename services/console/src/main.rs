//! Glacier Labs console backend.
//!
//! Serves the console page and a small read-only API:
//!
//! - `GET /api/topology`: the deployment description (hosts, services, stores, edges).
//! - `GET /api/state`: the latest snapshot of every service: container state
//!   from the Docker Engine API, plus each service's `/health`, `/metrics`
//!   and optional `/recent` endpoints.
//!
//! A background task refreshes the snapshot every two seconds, so any number
//! of viewers cost one round of polling. The console never writes to Docker
//! or to any service; it should reach Docker through a read-only proxy.
//!
//! Configuration (environment):
//! - `CONSOLE_LISTEN`: address to serve on (default `127.0.0.1:8080`)
//! - `CONSOLE_TOPOLOGY`: path to the topology JSON (default `topology.json`)
//! - `CONSOLE_DOCKER`: Docker Engine API base URL, e.g. `http://docker-proxy:2375` (optional)
//! - `CONSOLE_COMPOSE_PROJECT`: only show containers from this Compose project (optional)

use std::collections::HashMap;
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;

use axum::Router;
use axum::extract::State as AxumState;
use axum::http::header;
use axum::response::{Html, IntoResponse, Response};
use axum::routing::get;
use glacier_observe::{Observer, State, unix_seconds};
use serde_json::{Value, json};

const PAGE: &str = include_str!("../web/index.html");

struct Config {
    topology: String,
    docker: Option<String>,
    project: Option<String>,
}

struct App {
    config: Config,
    http: reqwest::Client,
    snapshot: RwLock<Value>,
    /// Previous CPU readings per container id: (container total, system total).
    cpu: Mutex<HashMap<String, (f64, f64)>>,
    polls: std::sync::atomic::AtomicU64,
}

#[tokio::main]
async fn main() {
    let listen = env("CONSOLE_LISTEN", "127.0.0.1:8080");
    let config = Config {
        topology: env("CONSOLE_TOPOLOGY", "topology.json"),
        docker: std::env::var("CONSOLE_DOCKER")
            .ok()
            .filter(|s| !s.is_empty()),
        project: std::env::var("CONSOLE_COMPOSE_PROJECT")
            .ok()
            .filter(|s| !s.is_empty()),
    };
    let http = reqwest::Client::builder()
        .timeout(Duration::from_millis(1500))
        .build()
        .expect("http client");
    let app = Arc::new(App {
        config,
        http,
        snapshot: RwLock::new(json!({ "generated_at": null, "services": {} })),
        cpu: Mutex::new(HashMap::new()),
        polls: std::sync::atomic::AtomicU64::new(0),
    });

    let observer = Observer::new("console", env!("CARGO_PKG_VERSION"));
    {
        let a = app.clone();
        observer.set_metrics(move |w| {
            w.counter(
                "glacier_console_polls_total",
                "Snapshot refreshes since start.",
                &[],
                a.polls.load(std::sync::atomic::Ordering::Relaxed) as f64,
            );
        });
    }

    tokio::spawn(poll_forever(app.clone(), observer.clone()));

    let router = Router::new()
        .route("/", get(page))
        .route("/api/topology", get(topology))
        .route("/api/state", get(state))
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

async fn page() -> Html<&'static str> {
    Html(PAGE)
}

async fn topology(AxumState(app): AxumState<Arc<App>>) -> Response {
    match read_topology(&app.config.topology) {
        Ok(v) => axum::Json(v).into_response(),
        Err(e) => (axum::http::StatusCode::INTERNAL_SERVER_ERROR, e).into_response(),
    }
}

async fn state(AxumState(app): AxumState<Arc<App>>) -> Response {
    let body = app
        .snapshot
        .read()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .to_string();
    (
        [
            (header::CONTENT_TYPE, "application/json"),
            (header::CACHE_CONTROL, "no-store"),
        ],
        body,
    )
        .into_response()
}

fn read_topology(path: &str) -> Result<Value, String> {
    let text =
        std::fs::read_to_string(path).map_err(|e| format!("cannot read topology {path}: {e}"))?;
    serde_json::from_str(&text).map_err(|e| format!("invalid topology {path}: {e}"))
}

async fn poll_forever(app: Arc<App>, observer: Observer) {
    let mut tick = tokio::time::interval(Duration::from_secs(2));
    loop {
        tick.tick().await;
        let (snapshot, problem) = poll_once(&app).await;
        *app.snapshot
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = snapshot;
        app.polls.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        match problem {
            None => observer.set_state(State::Ready, "polling"),
            Some(p) => {
                observer.record_error(&p);
                observer.set_state(State::Degraded, &p);
            }
        }
    }
}

/// Build one snapshot. Returns it with a description of any console-level problem.
async fn poll_once(app: &App) -> (Value, Option<String>) {
    let topo = match read_topology(&app.config.topology) {
        Ok(t) => t,
        Err(e) => {
            return (
                json!({ "generated_at": unix_seconds(), "error": e, "services": {} }),
                Some(e),
            );
        }
    };
    let (containers, docker_error) = match &app.config.docker {
        Some(base) => match containers(app, base).await {
            Ok(c) => (c, None),
            Err(e) => (HashMap::new(), Some(e)),
        },
        None => (HashMap::new(), None),
    };

    let mut services = serde_json::Map::new();
    let empty = Vec::new();
    for svc in topo["services"].as_array().unwrap_or(&empty) {
        let Some(id) = svc["id"].as_str() else {
            continue;
        };
        let container_name = svc["container"].as_str().unwrap_or(id);
        let mut entry = serde_json::Map::new();
        entry.insert(
            "container".into(),
            containers
                .get(container_name)
                .cloned()
                .unwrap_or(Value::Null),
        );
        if let Some(url) = svc["health"].as_str() {
            match app.http.get(url).send().await {
                Ok(r) => {
                    let code = r.status().as_u16();
                    let body = r.json::<Value>().await.unwrap_or(Value::Null);
                    entry.insert(
                        "health".into(),
                        json!({ "http_status": code, "body": body }),
                    );
                }
                Err(e) => {
                    entry.insert("health".into(), json!({ "error": short_error(&e) }));
                }
            }
        }
        if let Some(url) = svc["metrics"].as_str()
            && let Ok(r) = app.http.get(url).send().await
            && let Ok(text) = r.text().await
        {
            entry.insert("metrics".into(), parse_metrics(&text));
        }
        if let Some(url) = svc["recent"].as_str()
            && let Ok(r) = app.http.get(url).send().await
        {
            entry.insert(
                "recent".into(),
                r.json::<Value>().await.unwrap_or(Value::Null),
            );
        }
        services.insert(id.to_owned(), Value::Object(entry));
    }

    let snapshot = json!({
        "generated_at": unix_seconds(),
        "docker": { "configured": app.config.docker.is_some(), "error": docker_error },
        "services": services,
    });
    let problem = docker_error.map(|e| format!("docker api: {e}"));
    (snapshot, problem)
}

/// Containers keyed by Compose service name (or container name), with state,
/// uptime, restarts and resource use.
async fn containers(app: &App, base: &str) -> Result<HashMap<String, Value>, String> {
    let mut url = format!("{base}/containers/json?all=1");
    if let Some(p) = &app.config.project {
        let filter = json!({ "label": [format!("com.docker.compose.project={p}")] }).to_string();
        url.push_str("&filters=");
        url.push_str(&percent_encode(&filter));
    }
    let list: Vec<Value> = get_json(app, &url).await?;
    let mut out = HashMap::new();
    for c in list {
        let Some(id) = c["Id"].as_str() else { continue };
        let name = c["Labels"]["com.docker.compose.service"]
            .as_str()
            .map(str::to_owned)
            .or_else(|| {
                c["Names"][0]
                    .as_str()
                    .map(|n| n.trim_start_matches('/').to_owned())
            })
            .unwrap_or_else(|| id[..12.min(id.len())].to_owned());
        let inspect: Value = get_json(app, &format!("{base}/containers/{id}/json"))
            .await
            .unwrap_or(Value::Null);
        let running = c["State"].as_str() == Some("running");
        let mut cpu_percent = Value::Null;
        let mut mem = Value::Null;
        if running
            && let Ok(stats) = get_json::<Value>(
                app,
                &format!("{base}/containers/{id}/stats?stream=false&one-shot=true"),
            )
            .await
        {
            cpu_percent = cpu(app, id, &stats).map_or(Value::Null, Value::from);
            let usage = stats["memory_stats"]["usage"].as_f64();
            let limit = stats["memory_stats"]["limit"].as_f64();
            if let Some(u) = usage {
                mem = json!({ "usage_bytes": u, "limit_bytes": limit });
            }
        }
        out.insert(
            name,
            json!({
                "id": &id[..12.min(id.len())],
                "image": c["Image"],
                "state": c["State"],
                "status": c["Status"],
                "started_at": inspect["State"]["StartedAt"],
                "restart_count": inspect["RestartCount"],
                "health": inspect["State"]["Health"]["Status"],
                "cpu_percent": cpu_percent,
                "memory": mem,
            }),
        );
    }
    Ok(out)
}

/// CPU use since the previous poll, as a percentage of one core.
fn cpu(app: &App, id: &str, stats: &Value) -> Option<f64> {
    let total = stats["cpu_stats"]["cpu_usage"]["total_usage"].as_f64()?;
    let system = stats["cpu_stats"]["system_cpu_usage"].as_f64()?;
    let cpus = stats["cpu_stats"]["online_cpus"].as_f64().unwrap_or(1.0);
    let mut prev = app
        .cpu
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let result = prev.get(id).and_then(|(pt, ps)| {
        let dt = total - pt;
        let ds = system - ps;
        (ds > 0.0 && dt >= 0.0).then(|| dt / ds * cpus * 100.0)
    });
    prev.insert(id.to_owned(), (total, system));
    result
}

async fn get_json<T: serde::de::DeserializeOwned>(app: &App, url: &str) -> Result<T, String> {
    let r = app
        .http
        .get(url)
        .send()
        .await
        .map_err(|e| short_error(&e))?;
    if !r.status().is_success() {
        return Err(format!("{} from {url}", r.status()));
    }
    r.json::<T>().await.map_err(|e| short_error(&e))
}

/// Prometheus text to `{ name: [{ labels: {..}, value }] }`.
fn parse_metrics(text: &str) -> Value {
    let mut out: serde_json::Map<String, Value> = serde_json::Map::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (series, value) = match line.rsplit_once(' ') {
            Some(x) => x,
            None => continue,
        };
        let Ok(value) = value.parse::<f64>() else {
            continue;
        };
        let (name, labels) = match series.split_once('{') {
            Some((n, rest)) => (n, parse_labels(rest.trim_end_matches('}'))),
            None => (series, serde_json::Map::new()),
        };
        let v = if value.is_finite() {
            json!(value)
        } else {
            Value::Null
        };
        out.entry(name.to_owned())
            .or_insert_with(|| Value::Array(Vec::new()))
            .as_array_mut()
            .expect("inserted as array")
            .push(json!({ "labels": labels, "value": v }));
    }
    Value::Object(out)
}

fn parse_labels(s: &str) -> serde_json::Map<String, Value> {
    let mut out = serde_json::Map::new();
    let mut rest = s;
    while let Some((key, after)) = rest.split_once("=\"") {
        let mut value = String::new();
        let mut chars = after.char_indices();
        let mut end = after.len();
        while let Some((i, c)) = chars.next() {
            match c {
                '\\' => {
                    if let Some((_, n)) = chars.next() {
                        value.push(if n == 'n' { '\n' } else { n });
                    }
                }
                '"' => {
                    end = i + 1;
                    break;
                }
                _ => value.push(c),
            }
        }
        out.insert(
            key.trim_start_matches(',').trim().to_owned(),
            Value::String(value),
        );
        rest = &after[end..];
    }
    out
}

fn percent_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len() * 3);
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

fn short_error(e: &reqwest::Error) -> String {
    if e.is_timeout() {
        "timed out".into()
    } else if e.is_connect() {
        "connection refused or host unreachable".into()
    } else {
        e.to_string()
    }
}

fn env(key: &str, default: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_prometheus_text() {
        let v = parse_metrics(
            "# HELP x X.\n# TYPE x counter\nx{product=\"BTC-USD\",kind=\"a\\\"b\"} 3\nup 1\nbad line here\n",
        );
        assert_eq!(v["x"][0]["labels"]["product"], "BTC-USD");
        assert_eq!(v["x"][0]["labels"]["kind"], "a\"b");
        assert_eq!(v["x"][0]["value"], 3.0);
        assert_eq!(v["up"][0]["value"], 1.0);
    }

    #[test]
    fn encodes_filters() {
        assert_eq!(percent_encode("{\"a\":1}"), "%7B%22a%22%3A1%7D");
    }
}

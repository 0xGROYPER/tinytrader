//! The fleet data layer shared by every Glacier front end.
//!
//! [`Poller`] discovers a deployment and polls its services: with Docker it
//! lists the project's containers, asks those labelled `glacier.observe` to
//! `/describe` themselves, applies the optional topology overrides, and
//! collects each service's health, metrics and series into a snapshot. The
//! web console serves what the poller collects; a terminal UI can embed a
//! poller directly or read a running console through [`Source::Console`].
//!
//! The JSON it produces is the console API contract (`glacier.fleet/1`);
//! [`Topology`], [`Snapshot`] and [`EventsPage`] are its typed form.

mod model;
mod source;

pub use model::{
    Container, Edge, EventsPage, FleetEvent, Health, HealthResult, Host, Node, ServiceInfo,
    ServiceState, Snapshot, Status, Topology,
};
pub use source::Source;

use std::collections::HashMap;
use std::sync::{Mutex, RwLock};
use std::time::{Duration, Instant};

use glacier_observe::unix_seconds;
use serde_json::{Value, json};

/// Where a poller finds the deployment.
#[derive(Clone, Debug, Default)]
pub struct PollerConfig {
    /// Path of the optional topology overrides file.
    pub topology: String,
    /// Docker Engine API base URL (ideally a read-only proxy), for discovery.
    pub docker: Option<String>,
    /// Only consider containers from this Compose project.
    pub project: Option<String>,
}

/// Discovers and polls a deployment. Call [`Poller::poll`] periodically.
pub struct Poller {
    config: PollerConfig,
    http: reqwest::Client,
    snapshot: RwLock<Value>,
    /// The deployment as currently drawn: discovered, then overridden.
    topology: RwLock<Value>,
    /// `/describe` documents per container id, with when they were fetched.
    describes: Mutex<HashMap<String, (Instant, Value)>>,
    /// Previous CPU readings per container id: (container total, system total).
    cpu: Mutex<HashMap<String, (f64, f64)>>,
}

impl Poller {
    #[must_use]
    pub fn new(config: PollerConfig) -> Self {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_millis(1500))
            .build()
            .expect("http client");
        Self {
            config,
            http,
            snapshot: RwLock::new(json!({ "generated_at": null, "services": {} })),
            topology: RwLock::new(json!({ "services": [] })),
            describes: Mutex::new(HashMap::new()),
            cpu: Mutex::new(HashMap::new()),
        }
    }

    #[must_use]
    pub fn config(&self) -> &PollerConfig {
        &self.config
    }

    /// Refresh the topology and snapshot once. Returns a description of any
    /// poller-level problem (an unreadable overrides file, Docker unavailable).
    pub async fn poll(&self) -> Option<String> {
        let (snapshot, problem) = poll_once(self).await;
        *self
            .snapshot
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = snapshot;
        problem
    }

    /// The deployment as JSON (`/api/topology`).
    #[must_use]
    pub fn topology_json(&self) -> Value {
        read(&self.topology).clone()
    }

    /// The latest snapshot as JSON (`/api/state`).
    #[must_use]
    pub fn snapshot_json(&self) -> Value {
        read(&self.snapshot).clone()
    }

    /// The deployment, typed.
    #[must_use]
    pub fn topology(&self) -> Topology {
        serde_json::from_value(self.topology_json()).unwrap_or_default()
    }

    /// The latest snapshot, typed.
    #[must_use]
    pub fn snapshot(&self) -> Snapshot {
        serde_json::from_value(self.snapshot_json()).unwrap_or_default()
    }

    /// Events from a service's live tail, newer than `after`
    /// (`/api/events/<service>`). Errors name the problem.
    pub async fn events_json(
        &self,
        service: &str,
        after: u64,
        limit: u64,
    ) -> Result<Value, EventsError> {
        let url = self.topology_json()["services"]
            .as_array()
            .and_then(|list| list.iter().find(|s| s["id"].as_str() == Some(service)))
            .and_then(|s| s["events"].as_str())
            .map(str::to_owned)
            .ok_or_else(|| EventsError::NotDeclared(service.to_owned()))?;
        let sep = if url.contains('?') { '&' } else { '?' };
        let r = self
            .http
            .get(format!("{url}{sep}after={after}&limit={limit}"))
            .send()
            .await
            .map_err(|e| EventsError::Unreachable(short_error(&e)))?;
        r.json::<Value>()
            .await
            .map_err(|e| EventsError::Unreachable(short_error(&e)))
    }
}

/// Why a service's events could not be fetched.
#[derive(Debug)]
pub enum EventsError {
    /// The service declares no event stream.
    NotDeclared(String),
    /// The service did not answer.
    Unreachable(String),
}

impl std::fmt::Display for EventsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotDeclared(s) => write!(f, "service {s} declares no events"),
            Self::Unreachable(e) => write!(f, "service unreachable: {e}"),
        }
    }
}

/// The optional topology file: absent means no overrides.
fn read_overrides(path: &str) -> Result<Value, String> {
    match std::fs::read_to_string(path) {
        Ok(text) => {
            serde_json::from_str(&text).map_err(|e| format!("invalid topology {path}: {e}"))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(json!({})),
        Err(e) => Err(format!("cannot read topology {path}: {e}")),
    }
}

/// Build the deployment from running containers and their `/describe` documents.
async fn discover(app: &Poller, containers: &HashMap<String, Value>) -> Value {
    let mut names: Vec<&String> = containers.keys().collect();
    names.sort();
    let (mut services, mut external, mut stores, mut edges, mut panels) =
        (Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new());
    let mut observed = Vec::new();
    let mut console = None;
    for name in names {
        let c = &containers[name];
        let labels = &c["labels"];
        let label = |k: &str| labels[k].as_str().map(str::to_owned);
        let host = label("glacier.host").unwrap_or_else(|| "local".into());
        let mut svc = json!({
            "id": name,
            "host": host,
            "label": label("glacier.label").unwrap_or_else(|| name.clone()),
            "role": label("glacier.role"),
        });
        if let Some(port) = label("glacier.observe") {
            let base = format!("http://{name}:{port}");
            let d = describe(app, c["id"].as_str().unwrap_or(name), &base).await;
            if d["service"].as_str() == Some("console") {
                console = Some(name.clone());
            }
            observed.push(name.clone());
            svc["health"] = json!(format!("{base}/health"));
            svc["metrics"] = json!(format!("{base}/metrics"));
            if let Some(r) = d["role"].as_str() {
                svc["role"] = json!(r);
            }
            if let Some(path) = d["streams"]["events"].as_str() {
                svc["events"] = json!(format!("{base}{path}"));
            }
            if let Some(path) = d["streams"]["series"].as_str() {
                svc["recent"] = json!(format!("{base}{path}"));
            }
            for u in d["upstream"].as_array().into_iter().flatten() {
                let kind = u["kind"].as_str().unwrap_or("flow");
                let from = if let Some(other) = u["service"].as_str() {
                    other.to_owned()
                } else if let Some(id) = u["id"].as_str() {
                    if !external.iter().any(|e: &Value| e["id"] == id) {
                        external.push(json!({ "id": id, "label": u["label"].as_str().unwrap_or(id), "sub": u["sub"], "role": u["role"] }));
                    }
                    id.to_owned()
                } else {
                    continue;
                };
                edges.push(json!({ "from": from, "to": name, "kind": kind, "label": u["via"] }));
            }
            for st in d["stores"].as_array().into_iter().flatten() {
                let id = format!("{name}-{}", st["id"].as_str().unwrap_or("store"));
                let mut store = json!({ "id": id, "host": svc["host"], "label": st["label"], "sub": st["sub"] });
                if let Some(m) = st["metric"].as_str() {
                    store["metric"] = json!({ "service": name, "name": m, "label": "Bytes written", "format": "bytes", "suffix": " written" });
                }
                stores.push(store);
                edges.push(json!({ "from": name, "to": id, "kind": "flow" }));
            }
            for p in d["panels"].as_array().into_iter().flatten() {
                let mut p = p.clone();
                p["service"] = json!(name);
                p["source"] = json!("recent");
                panels.push(p);
            }
        }
        services.push(svc);
    }
    if let Some(console) = &console {
        for name in observed.iter().filter(|n| *n != console) {
            edges.push(
                json!({ "from": name, "to": console, "kind": "read", "label": "health · metrics" }),
            );
        }
    }
    json!({
        "name": app.config.project.clone().unwrap_or_else(|| "deployment".into()),
        "hosts": [{ "id": "local", "label": "local", "sub": "Docker Compose" }],
        "external": external,
        "services": services,
        "stores": stores,
        "edges": edges,
        "panels": panels,
    })
}

/// A container's `/describe` document, refreshed at most every 30 seconds.
async fn describe(app: &Poller, id: &str, base: &str) -> Value {
    if let Some((at, d)) = lock(&app.describes).get(id)
        && at.elapsed() < Duration::from_secs(30)
    {
        return d.clone();
    }
    match get_json::<Value>(app, &format!("{base}/describe")).await {
        Ok(d) => {
            lock(&app.describes).insert(id.to_owned(), (Instant::now(), d.clone()));
            d
        }
        // A stopped or restarting service keeps its last known description;
        // one never described is retried on every poll.
        Err(_) => lock(&app.describes)
            .get(id)
            .map_or_else(|| json!({}), |(_, d)| d.clone()),
    }
}

/// Layer the topology file's overrides over the discovered deployment.
fn apply_overrides(mut topo: Value, o: &Value) -> Value {
    for key in ["name", "focus", "hosts"] {
        if !o[key].is_null() {
            topo[key] = o[key].clone();
        }
    }
    for list in ["services", "external", "stores"] {
        let mut items = topo[list].as_array().cloned().unwrap_or_default();
        for item in o[list].as_array().into_iter().flatten() {
            match items.iter_mut().find(|i| i["id"] == item["id"]) {
                Some(existing) => {
                    if let (Some(e), Some(n)) = (existing.as_object_mut(), item.as_object()) {
                        for (k, v) in n {
                            e.insert(k.clone(), v.clone());
                        }
                    }
                }
                None => items.push(item.clone()),
            }
        }
        topo[list] = json!(items);
    }
    let mut edges = topo["edges"].as_array().cloned().unwrap_or_default();
    for e in o["edges"].as_array().into_iter().flatten() {
        if !edges
            .iter()
            .any(|x| x["from"] == e["from"] && x["to"] == e["to"])
        {
            edges.push(e.clone());
        }
    }
    topo["edges"] = json!(edges);
    if let Some(own) = o["panels"].as_array() {
        let overridden: Vec<&Value> = own.iter().map(|p| &p["service"]).collect();
        let mut panels: Vec<Value> = topo["panels"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|p| !overridden.contains(&&p["service"]))
            .cloned()
            .collect();
        panels.extend(own.iter().cloned());
        topo["panels"] = json!(panels);
    }
    if let Some(hide) = o["hide"].as_array() {
        let hidden = |v: &Value| hide.contains(v);
        for list in ["services", "external", "stores"] {
            let kept: Vec<Value> = topo[list]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|i| !hidden(&i["id"]))
                .cloned()
                .collect();
            topo[list] = json!(kept);
        }
        let kept: Vec<Value> = topo["edges"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|e| !hidden(&e["from"]) && !hidden(&e["to"]))
            .cloned()
            .collect();
        topo["edges"] = json!(kept);
        let kept: Vec<Value> = topo["panels"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|p| !hidden(&p["service"]))
            .cloned()
            .collect();
        topo["panels"] = json!(kept);
    }
    topo
}

/// Build one snapshot. Returns it with a description of any console-level problem.
async fn poll_once(app: &Poller) -> (Value, Option<String>) {
    let overrides = match read_overrides(&app.config.topology) {
        Ok(o) => o,
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
    let discovered = if app.config.docker.is_some() {
        discover(app, &containers).await
    } else {
        json!({ "name": "deployment", "services": [] })
    };
    let topo = apply_overrides(discovered, &overrides);
    *app.topology
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = topo.clone();

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
async fn containers(app: &Poller, base: &str) -> Result<HashMap<String, Value>, String> {
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
                "labels": c["Labels"],
                "cpu_percent": cpu_percent,
                "memory": mem,
            }),
        );
    }
    Ok(out)
}

/// CPU use since the previous poll, as a percentage of one core.
fn cpu(app: &Poller, id: &str, stats: &Value) -> Option<f64> {
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

async fn get_json<T: serde::de::DeserializeOwned>(app: &Poller, url: &str) -> Result<T, String> {
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

fn read<T>(l: &RwLock<T>) -> std::sync::RwLockReadGuard<'_, T> {
    l.read().unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
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
    fn overrides_merge_hide_and_replace_panels() {
        let found = json!({
            "name": "proj",
            "services": [{ "id": "a", "label": "a" }, { "id": "b", "label": "b" }],
            "external": [],
            "stores": [],
            "edges": [{ "from": "a", "to": "b" }],
            "panels": [{ "service": "a", "heading": "found" }],
        });
        let o = json!({
            "name": "mine",
            "services": [{ "id": "a", "label": "Alpha" }, { "id": "c", "label": "c" }],
            "panels": [{ "service": "a", "heading": "mine" }],
            "hide": ["b"],
        });
        let t = apply_overrides(found, &o);
        assert_eq!(t["name"], "mine");
        assert_eq!(
            t["services"],
            json!([{ "id": "a", "label": "Alpha" }, { "id": "c", "label": "c" }])
        );
        assert_eq!(t["edges"], json!([]));
        assert_eq!(t["panels"], json!([{ "service": "a", "heading": "mine" }]));
    }

    #[test]
    fn encodes_filters() {
        assert_eq!(percent_encode("{\"a\":1}"), "%7B%22a%22%3A1%7D");
    }
}

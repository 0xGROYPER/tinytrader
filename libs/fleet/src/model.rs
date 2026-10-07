//! Typed form of the console API (`tinytrader.fleet/1`). Every field is
//! optional on the wire, so older and newer documents both parse; open-ended
//! parts (metrics, series, panel specs) stay as JSON values.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The deployment: what exists and how it connects (`/api/topology`).
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct Topology {
    pub name: String,
    pub focus: Option<String>,
    pub hosts: Vec<Host>,
    pub external: Vec<Node>,
    pub services: Vec<ServiceInfo>,
    pub stores: Vec<Node>,
    pub edges: Vec<Edge>,
    pub panels: Vec<Value>,
    /// Bundles (decision 0011): services sharing hours, with a work class.
    pub bundles: Vec<Value>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct Host {
    pub id: String,
    pub label: Option<String>,
    pub sub: Option<String>,
    /// URL of the host's node_exporter metrics, if it runs one.
    pub exporter: Option<String>,
}

/// An external source or a store.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct Node {
    pub id: String,
    pub label: Option<String>,
    pub sub: Option<String>,
    pub role: Option<String>,
    pub host: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct ServiceInfo {
    pub id: String,
    pub host: Option<String>,
    pub label: Option<String>,
    pub role: Option<String>,
    pub health: Option<String>,
    pub metrics: Option<String>,
    pub events: Option<String>,
    pub recent: Option<String>,
    pub catalog: Option<String>,
    pub news: Option<String>,
    pub accounts: Option<String>,
    pub calendar: Option<String>,
    /// What the service runs and when (`/describe`'s `schedule`).
    pub schedule: Option<Value>,
    /// One symbol's one-minute bars, on demand (relays).
    pub bars: Option<String>,
}

impl ServiceInfo {
    /// The name to show: its label, or its id.
    #[must_use]
    pub fn name(&self) -> &str {
        self.label.as_deref().unwrap_or(&self.id)
    }
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct Edge {
    pub from: String,
    pub to: String,
    pub kind: Option<String>,
    pub label: Option<String>,
}

/// The latest state of every service (`/api/state`).
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct Snapshot {
    pub generated_at: Option<f64>,
    pub error: Option<String>,
    pub docker: Option<Value>,
    pub services: BTreeMap<String, ServiceState>,
    /// Machine summaries for hosts with a node_exporter.
    pub hosts: BTreeMap<String, HostState>,
}

/// A machine's CPU, memory, swap, load, uptime and filesystems.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct HostState {
    pub error: Option<String>,
    pub cpu_percent: Option<f64>,
    pub cores: Option<u32>,
    pub load: Vec<Option<f64>>,
    pub memory: Usage,
    pub swap: Usage,
    pub uptime_seconds: Option<f64>,
    pub disks: Vec<Disk>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct Usage {
    pub total_bytes: Option<f64>,
    pub used_bytes: Option<f64>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct Disk {
    pub mount: String,
    pub total_bytes: f64,
    pub available_bytes: f64,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct ServiceState {
    pub container: Option<Container>,
    pub health: Option<HealthResult>,
    pub metrics: Option<Value>,
    pub recent: Option<Value>,
    /// The service's catalog of data roots, if it serves one.
    pub catalog: Option<Value>,
    /// The service's news stories, if it serves them.
    pub news: Option<Value>,
    /// The brokerage account it reads, if it serves one.
    pub accounts: Option<Value>,
    /// The market calendar it serves, if any.
    pub calendar: Option<Value>,
    /// Scheduled economic releases it serves, if any (`{ "events": [...] }`).
    pub econ: Option<Value>,
    /// A relay's connected clients (`{ "clients": [...] }`).
    pub clients: Option<Value>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct Container {
    pub id: Option<String>,
    pub image: Option<String>,
    pub state: Option<String>,
    pub status: Option<String>,
    pub started_at: Option<String>,
    pub restart_count: Option<u64>,
    pub cpu_percent: Option<f64>,
    pub memory: Option<Value>,
}

impl Container {
    /// Memory in use and the container's limit, in bytes.
    #[must_use]
    pub fn memory_use(&self) -> (Option<f64>, Option<f64>) {
        let m = self.memory.as_ref();
        (
            m.and_then(|m| m["usage_bytes"].as_f64()),
            m.and_then(|m| m["limit_bytes"].as_f64()),
        )
    }
}

/// The outcome of asking a service for `/health`.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct HealthResult {
    pub http_status: Option<u16>,
    pub body: Option<Health>,
    pub error: Option<String>,
}

/// The `tinytrader.observe/1` health document.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct Health {
    pub service: Option<String>,
    pub state: Option<String>,
    pub detail: Option<String>,
    pub build: Option<String>,
    pub uptime_seconds: Option<f64>,
    pub last_error: Option<String>,
    pub last_error_at: Option<f64>,
}

/// A summary status, the same rule the web console colours by.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    Ok,
    Warn,
    Crit,
    Idle,
}

impl ServiceState {
    #[must_use]
    pub fn status(&self) -> Status {
        let c = self.container.as_ref().and_then(|c| c.state.as_deref());
        if matches!(c, Some("exited" | "dead")) {
            return Status::Crit;
        }
        if c.is_some_and(|s| s != "running") {
            return Status::Warn;
        }
        match &self.health {
            Some(h) if h.error.is_some() => {
                if c.is_some() {
                    Status::Warn
                } else {
                    Status::Crit
                }
            }
            Some(h) => match h.body.as_ref().and_then(|b| b.state.as_deref()) {
                Some("ready") | None => Status::Ok,
                Some("idle") => Status::Idle,
                Some(_) => Status::Warn,
            },
            None if c.is_some() => Status::Ok,
            None => Status::Idle,
        }
    }

    /// A one-line description of what the service is doing now.
    #[must_use]
    pub fn detail(&self) -> String {
        if let Some(d) = self
            .health
            .as_ref()
            .and_then(|h| h.body.as_ref())
            .and_then(|b| b.detail.clone())
        {
            return d;
        }
        if self.health.as_ref().is_some_and(|h| h.error.is_some()) {
            return "unreachable".into();
        }
        self.container
            .as_ref()
            .and_then(|c| c.status.clone())
            .unwrap_or_default()
    }
}

/// A page of a service's live tail (`/api/events/<service>`).
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct EventsPage {
    pub events: Vec<FleetEvent>,
    pub next: u64,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct FleetEvent {
    pub seq: u64,
    pub received_ms: f64,
    pub summary: String,
    pub raw: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn status_follows_container_and_health() {
        let s: ServiceState = serde_json::from_value(json!({
            "container": { "state": "running" },
            "health": { "http_status": 200, "body": { "state": "ready", "detail": "streaming" } }
        }))
        .unwrap();
        assert_eq!(s.status(), Status::Ok);
        assert_eq!(s.detail(), "streaming");
        let down: ServiceState = serde_json::from_value(
            json!({ "container": { "state": "exited", "status": "Exited (0)" } }),
        )
        .unwrap();
        assert_eq!(down.status(), Status::Crit);
        let unknown: ServiceState = serde_json::from_value(json!({})).unwrap();
        assert_eq!(unknown.status(), Status::Idle);
    }

    #[test]
    fn tolerates_unknown_and_missing_fields() {
        let t: Topology = serde_json::from_value(
            json!({ "name": "x", "future": 1, "services": [{ "id": "a" }] }),
        )
        .unwrap();
        assert_eq!(t.services[0].name(), "a");
    }
}

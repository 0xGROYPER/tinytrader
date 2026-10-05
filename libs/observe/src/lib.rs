//! Health and metrics endpoints shared by Glacier services.
//!
//! Every long-running service exposes two read-only routes:
//!
//! - `GET /health`: JSON describing the service and its state. HTTP 200 when
//!   the service is ready, 503 otherwise.
//! - `GET /metrics`: Prometheus text exposition format.
//!
//! Metric names and the health document are a versioned contract
//! (`glacier.observe/1`): changes are additive only. Labels stay low
//! cardinality: service, venue, product; never per-order or per-instrument ids.

use std::collections::{HashMap, VecDeque};
use std::fmt::Write as _;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use axum::Router;
use axum::extract::State as AxumState;
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;

/// Schema identifier for the health document and metric names.
pub const SCHEMA: &str = "glacier.observe/1";

/// Lifecycle state reported by `/health`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    Starting,
    Ready,
    Degraded,
    Recovering,
    Stopping,
    Failed,
}

impl State {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            State::Starting => "starting",
            State::Ready => "ready",
            State::Degraded => "degraded",
            State::Recovering => "recovering",
            State::Stopping => "stopping",
            State::Failed => "failed",
        }
    }
}

struct Status {
    state: State,
    detail: String,
    last_error: Option<String>,
    last_error_at: Option<f64>,
}

type Render = Box<dyn Fn(&mut MetricsWriter) + Send + Sync>;

/// Shared handle a service uses to report its state and expose metrics.
#[derive(Clone)]
pub struct Observer {
    inner: Arc<Inner>,
}

struct Inner {
    service: String,
    build: String,
    started_wall: f64,
    started: Instant,
    status: Mutex<Status>,
    render: Mutex<Option<Render>>,
}

impl Observer {
    /// Create an observer for `service`, starting in [`State::Starting`].
    #[must_use]
    pub fn new(service: &str, build: &str) -> Self {
        Self {
            inner: Arc::new(Inner {
                service: service.to_owned(),
                build: build.to_owned(),
                started_wall: unix_seconds(),
                started: Instant::now(),
                status: Mutex::new(Status {
                    state: State::Starting,
                    detail: String::new(),
                    last_error: None,
                    last_error_at: None,
                }),
                render: Mutex::new(None),
            }),
        }
    }

    /// Set the current state and a short human-readable detail.
    pub fn set_state(&self, state: State, detail: &str) {
        let mut s = lock(&self.inner.status);
        s.state = state;
        detail.clone_into(&mut s.detail);
    }

    /// Record the most recent error, with its time, without changing the state.
    pub fn record_error(&self, error: &str) {
        let mut s = lock(&self.inner.status);
        s.last_error = Some(error.to_owned());
        s.last_error_at = Some(unix_seconds());
    }

    /// Register the function that writes this service's own metrics.
    pub fn set_metrics<F>(&self, render: F)
    where
        F: Fn(&mut MetricsWriter) + Send + Sync + 'static,
    {
        *lock(&self.inner.render) = Some(Box::new(render));
    }

    #[must_use]
    pub fn state(&self) -> State {
        lock(&self.inner.status).state
    }

    /// The health document served at `/health`.
    #[must_use]
    pub fn health_json(&self) -> serde_json::Value {
        let s = lock(&self.inner.status);
        serde_json::json!({
            "schema": SCHEMA,
            "service": self.inner.service,
            "state": s.state.as_str(),
            "detail": s.detail,
            "build": self.inner.build,
            "started_at": self.inner.started_wall,
            "uptime_seconds": self.inner.started.elapsed().as_secs_f64(),
            "last_error": s.last_error,
            "last_error_at": s.last_error_at,
        })
    }

    /// The Prometheus text served at `/metrics`.
    #[must_use]
    pub fn metrics_text(&self) -> String {
        let mut w = MetricsWriter::default();
        w.gauge(
            "glacier_observe_schema_version",
            "Version of the glacier.observe metric contract.",
            &[],
            1.0,
        );
        w.gauge(
            "process_start_time_seconds",
            "Start time of the process since the Unix epoch.",
            &[],
            self.inner.started_wall,
        );
        let state = self.state();
        w.gauge(
            "glacier_ready",
            "1 when the service reports ready, otherwise 0.",
            &[("service", &self.inner.service)],
            if state == State::Ready { 1.0 } else { 0.0 },
        );
        if let Some(render) = lock(&self.inner.render).as_ref() {
            render(&mut w);
        }
        w.out
    }

    /// Router with `/health` and `/metrics`, to merge into the service's app.
    pub fn router(&self) -> Router {
        Router::new()
            .route("/health", get(health))
            .route("/metrics", get(metrics))
            .with_state(self.clone())
    }
}

async fn health(AxumState(o): AxumState<Observer>) -> Response {
    let code = if o.state() == State::Ready {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };
    (code, axum::Json(o.health_json())).into_response()
}

async fn metrics(AxumState(o): AxumState<Observer>) -> Response {
    (
        [(header::CONTENT_TYPE, "text/plain; version=0.0.4")],
        o.metrics_text(),
    )
        .into_response()
}

/// Writes metrics in Prometheus text format, emitting HELP and TYPE once per name.
#[derive(Default)]
pub struct MetricsWriter {
    out: String,
    seen: Vec<String>,
}

impl MetricsWriter {
    pub fn counter(&mut self, name: &str, help: &str, labels: &[(&str, &str)], value: f64) {
        self.sample(name, help, "counter", labels, value);
    }

    pub fn gauge(&mut self, name: &str, help: &str, labels: &[(&str, &str)], value: f64) {
        self.sample(name, help, "gauge", labels, value);
    }

    fn sample(&mut self, name: &str, help: &str, kind: &str, labels: &[(&str, &str)], value: f64) {
        if !self.seen.iter().any(|n| n == name) {
            let _ = writeln!(self.out, "# HELP {name} {help}");
            let _ = writeln!(self.out, "# TYPE {name} {kind}");
            self.seen.push(name.to_owned());
        }
        self.out.push_str(name);
        if !labels.is_empty() {
            self.out.push('{');
            for (i, (k, v)) in labels.iter().enumerate() {
                if i > 0 {
                    self.out.push(',');
                }
                let escaped = v
                    .replace('\\', "\\\\")
                    .replace('"', "\\\"")
                    .replace('\n', "\\n");
                let _ = write!(self.out, "{k}=\"{escaped}\"");
            }
            self.out.push('}');
        }
        let _ = writeln!(self.out, " {value}");
    }
}

/// A bounded log of recent events, served at `GET /events` for live tails.
///
/// `GET /events?after=<seq>&limit=<n>` returns events newer than `after`,
/// newest last, at most `limit` (default 100, capped at the capacity):
/// `{ "events": [{ "seq", "received_ms", "summary", "raw" }], "next": <latest seq> }`.
#[derive(Clone)]
pub struct Events {
    inner: Arc<EventsInner>,
}

struct EventsInner {
    capacity: usize,
    next_seq: AtomicU64,
    log: Mutex<VecDeque<Event>>,
}

struct Event {
    seq: u64,
    received_ms: f64,
    summary: String,
    raw: String,
}

impl Events {
    #[must_use]
    pub fn new(capacity: usize) -> Self {
        Self {
            inner: Arc::new(EventsInner {
                capacity: capacity.max(1),
                next_seq: AtomicU64::new(0),
                log: Mutex::new(VecDeque::new()),
            }),
        }
    }

    /// Default capacity: the last 500 events.
    pub const DEFAULT_CAPACITY: usize = 500;

    /// Record an event: a one-line summary and the raw message it came from.
    pub fn push(&self, summary: String, raw: String) {
        let seq = self.inner.next_seq.fetch_add(1, Ordering::Relaxed) + 1;
        let mut log = lock(&self.inner.log);
        log.push_back(Event {
            seq,
            received_ms: unix_seconds() * 1000.0,
            summary,
            raw,
        });
        if log.len() > self.inner.capacity {
            log.pop_front();
        }
    }

    /// Events newer than `after`, newest last, at most `limit`.
    #[must_use]
    pub fn since(&self, after: u64, limit: usize) -> serde_json::Value {
        let log = lock(&self.inner.log);
        let newer: Vec<&Event> = log.iter().filter(|e| e.seq > after).collect();
        let start = newer.len().saturating_sub(limit.min(self.inner.capacity));
        let list: Vec<serde_json::Value> = newer[start..]
            .iter()
            .map(|e| {
                serde_json::json!({
                    "seq": e.seq,
                    "received_ms": e.received_ms,
                    "summary": e.summary,
                    "raw": e.raw,
                })
            })
            .collect();
        serde_json::json!({ "events": list, "next": self.inner.next_seq.load(Ordering::Relaxed) })
    }

    /// Router with `/events`, to merge into the service's app.
    pub fn router(&self) -> Router {
        Router::new()
            .route("/events", get(events))
            .with_state(self.clone())
    }
}

impl Default for Events {
    fn default() -> Self {
        Self::new(Self::DEFAULT_CAPACITY)
    }
}

async fn events(
    AxumState(e): AxumState<Events>,
    axum::extract::Query(q): axum::extract::Query<HashMap<String, String>>,
) -> axum::Json<serde_json::Value> {
    let after = q.get("after").and_then(|v| v.parse().ok()).unwrap_or(0);
    let limit = q.get("limit").and_then(|v| v.parse().ok()).unwrap_or(100);
    axum::Json(e.since(after, limit))
}

/// Seconds since the Unix epoch, as a float.
#[must_use]
pub fn unix_seconds() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0.0, |d| d.as_secs_f64())
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metrics_text_has_help_once_and_escapes_labels() {
        let o = Observer::new("demo", "test");
        o.set_metrics(|w| {
            w.counter(
                "demo_total",
                "Demo counter.",
                &[("product", "BTC-USD")],
                3.0,
            );
            w.counter("demo_total", "Demo counter.", &[("product", "a\"b")], 1.0);
        });
        let text = o.metrics_text();
        assert_eq!(text.matches("# HELP demo_total").count(), 1);
        assert!(text.contains("demo_total{product=\"BTC-USD\"} 3"));
        assert!(text.contains("demo_total{product=\"a\\\"b\"} 1"));
        assert!(text.contains("glacier_ready{service=\"demo\"} 0"));
    }

    #[test]
    fn events_are_bounded_and_resumable() {
        let e = Events::new(3);
        for i in 0..5 {
            e.push(format!("event {i}"), String::new());
        }
        let all = e.since(0, 100);
        assert_eq!(all["next"], 5);
        assert_eq!(all["events"].as_array().unwrap().len(), 3);
        assert_eq!(all["events"][0]["seq"], 3);
        let newer = e.since(4, 100);
        assert_eq!(newer["events"].as_array().unwrap().len(), 1);
        assert_eq!(newer["events"][0]["summary"], "event 4");
    }

    #[test]
    fn health_reports_state() {
        let o = Observer::new("demo", "test");
        o.set_state(State::Ready, "streaming");
        let h = o.health_json();
        assert_eq!(h["state"], "ready");
        assert_eq!(h["schema"], SCHEMA);
        assert!(h["last_error_at"].is_null());
        o.record_error("boom");
        assert_eq!(h["last_error"], serde_json::Value::Null);
        let h = o.health_json();
        assert_eq!(h["last_error"], "boom");
        assert!(h["last_error_at"].as_f64().is_some());
    }
}

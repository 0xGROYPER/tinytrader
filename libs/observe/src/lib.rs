//! Health and metrics endpoints shared by tinytrader services.
//!
//! Every long-running service exposes two read-only routes:
//!
//! - `GET /health`: JSON describing the service and its state. HTTP 200 when
//!   the service is ready, 503 otherwise.
//! - `GET /metrics`: Prometheus text exposition format.
//! - `GET /describe`: what the service is and can show, so a console can
//!   draw it without configuration (see [`Observer::describe`]).
//!
//! Metric names and the health document are a versioned contract
//! (`tinytrader.observe/1`): changes are additive only. Labels stay low
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
pub const SCHEMA: &str = "tinytrader.observe/1";

/// Lifecycle state reported by `/health`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    Starting,
    Ready,
    Degraded,
    Recovering,
    Stopping,
    Failed,
    /// Deliberately inactive, such as outside its active hours: not a fault.
    Idle,
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
            State::Idle => "idle",
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
    description: Mutex<serde_json::Value>,
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
                description: Mutex::new(serde_json::json!({})),
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

    /// Describe the service for `/describe`. Recognised fields, all optional:
    ///
    /// - `role`: one sentence on what this instance does.
    /// - `upstream`: sources it reads from, each `{ id, label, sub, kind, via }`
    ///   for something outside the deployment, or `{ service, kind, via }` for
    ///   another service in it. `kind` is `flow` (data) or `read`; `via` labels
    ///   the connection.
    /// - `streams`: paths of optional endpoints, `{ "events": "/events",
    ///   "series": "/recent" }`.
    /// - `stores`: places it writes, each `{ id, label, sub, metric }`, where
    ///   `metric` names a metric with the bytes written.
    /// - `panels`: suggested charts over `series`, each `{ heading, note,
    ///   decimals, footer }` (footer entries as in the topology format).
    pub fn describe(&self, description: serde_json::Value) {
        *lock(&self.inner.description) = description;
    }

    /// The document served at `/describe`.
    #[must_use]
    pub fn description_json(&self) -> serde_json::Value {
        let mut d = lock(&self.inner.description).clone();
        if let Some(map) = d.as_object_mut() {
            map.insert("schema".into(), SCHEMA.into());
            map.insert("service".into(), self.inner.service.clone().into());
            map.insert("build".into(), self.inner.build.clone().into());
        }
        d
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
            "tinytrader_observe_schema_version",
            "Version of the tinytrader.observe metric contract.",
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
            "tinytrader_ready",
            "1 when the service reports ready, otherwise 0.",
            &[("service", &self.inner.service)],
            if state == State::Ready { 1.0 } else { 0.0 },
        );
        if let Some(render) = lock(&self.inner.render).as_ref() {
            render(&mut w);
        }
        w.out
    }

    /// Router with `/health`, `/metrics` and `/describe`, to merge into the service's app.
    pub fn router(&self) -> Router {
        Router::new()
            .route("/health", get(health))
            .route("/metrics", get(metrics))
            .route("/describe", get(describe))
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

async fn describe(AxumState(o): AxumState<Observer>) -> axum::Json<serde_json::Value> {
    axum::Json(o.description_json())
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

/// Live time series for charts, served at `GET /recent` as
/// `{ "<key>": [[unix_ms, value], ...] }`.
///
/// Each key keeps at most `points` samples, taken at most once per
/// `interval_ms`. `/recent` returns the `max_keys` keys with the most
/// updates, so a feed with many instruments charts its most active ones.
/// Up to 50 times `max_keys` keys are tracked; beyond that, the key updated
/// least recently is dropped.
#[derive(Clone)]
pub struct Series {
    inner: Arc<Mutex<SeriesInner>>,
}

struct SeriesInner {
    points: usize,
    interval_ms: f64,
    max_keys: usize,
    keys: HashMap<String, SeriesKey>,
}

struct SeriesKey {
    samples: VecDeque<(f64, f64)>,
    last_sample_ms: f64,
    updated_ms: f64,
    updates: u64,
}

impl Series {
    #[must_use]
    pub fn new(points: usize, interval_ms: u64, max_keys: usize) -> Self {
        Self {
            inner: Arc::new(Mutex::new(SeriesInner {
                points: points.max(2),
                interval_ms: interval_ms as f64,
                max_keys: max_keys.max(1),
                keys: HashMap::new(),
            })),
        }
    }

    /// Record a value for `key` now.
    pub fn push(&self, key: &str, value: f64) {
        if !value.is_finite() {
            return;
        }
        self.push_at(key, value, unix_seconds() * 1000.0);
    }

    fn push_at(&self, key: &str, value: f64, now_ms: f64) {
        let mut s = lock(&self.inner);
        let (points, interval) = (s.points, s.interval_ms);
        if !s.keys.contains_key(key) && s.keys.len() >= s.max_keys * 50 {
            let oldest = s
                .keys
                .iter()
                .min_by(|a, b| a.1.updated_ms.total_cmp(&b.1.updated_ms))
                .map(|(k, _)| k.clone());
            if let Some(k) = oldest {
                s.keys.remove(&k);
            }
        }
        let k = s.keys.entry(key.to_owned()).or_insert(SeriesKey {
            samples: VecDeque::new(),
            last_sample_ms: f64::NEG_INFINITY,
            updated_ms: now_ms,
            updates: 0,
        });
        k.updated_ms = now_ms;
        k.updates += 1;
        if now_ms - k.last_sample_ms >= interval {
            k.last_sample_ms = now_ms;
            k.samples.push_back((now_ms, value));
            if k.samples.len() > points {
                k.samples.pop_front();
            }
        } else if let Some(last) = k.samples.back_mut() {
            last.1 = value; // keep the latest value within the interval
        }
    }

    /// The most active series, as served at `/recent`.
    #[must_use]
    pub fn snapshot(&self) -> serde_json::Value {
        let s = lock(&self.inner);
        let mut active: Vec<(&String, &SeriesKey)> = s.keys.iter().collect();
        active.sort_by(|a, b| b.1.updates.cmp(&a.1.updates).then_with(|| a.0.cmp(b.0)));
        let map: serde_json::Map<String, serde_json::Value> = active
            .into_iter()
            .take(s.max_keys)
            .map(|(k, v)| {
                let pts: Vec<[f64; 2]> = v.samples.iter().map(|(t, x)| [*t, *x]).collect();
                (k.clone(), serde_json::json!(pts))
            })
            .collect();
        serde_json::Value::Object(map)
    }

    /// Router with `/recent`, to merge into the service's app.
    pub fn router(&self) -> Router {
        Router::new()
            .route("/recent", get(recent))
            .with_state(self.clone())
    }
}

impl Default for Series {
    /// 15 minutes of one-second samples for up to 16 keys.
    fn default() -> Self {
        Self::new(900, 1000, 16)
    }
}

async fn recent(AxumState(s): AxumState<Series>) -> axum::Json<serde_json::Value> {
    axum::Json(s.snapshot())
}

/// The address a service serves its endpoints on: its own variable (such as
/// `TICKER_LISTEN`) if set, otherwise `TINYTRADER_LISTEN`, which deployment
/// tooling sets for every service, otherwise `default`.
#[must_use]
pub fn listen_address(service_var: &str, default: &str) -> String {
    std::env::var(service_var)
        .or_else(|_| std::env::var("TINYTRADER_LISTEN"))
        .unwrap_or_else(|_| default.to_owned())
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
        assert!(text.contains("tinytrader_ready{service=\"demo\"} 0"));
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
    fn series_samples_and_evicts() {
        let s = Series::new(3, 1000, 2);
        s.push_at("a", 1.0, 0.0);
        s.push_at("a", 2.0, 500.0); // within the interval: updates the last sample
        s.push_at("a", 3.0, 1000.0);
        let snap = s.snapshot();
        assert_eq!(snap["a"], serde_json::json!([[0.0, 2.0], [1000.0, 3.0]]));
        s.push_at("b", 1.0, 2000.0);
        s.push_at("b", 1.0, 3000.0);
        s.push_at("c", 1.0, 4000.0); // only the two most active keys are served
        let snap = s.snapshot();
        assert!(snap.get("a").is_some() && snap.get("b").is_some());
        assert!(snap.get("c").is_none());
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

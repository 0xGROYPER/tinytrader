//! Databento batch jobs: price a request, submit it, follow it, and download
//! the finished files into the store.
//!
//! - `GET /jobs`: Databento's jobs (not expired), each with what the store
//!   already holds of it and any download in progress.
//! - `GET /datasets`, `GET /schemas?dataset=`: for the request form.
//! - `POST /price` with `{ dataset, schema, symbols, stype_in, start, end }`
//!   (dates, `end` inclusive): the cost and billable size, and a quote ID,
//!   with the requested days the store already holds (`held_days`) and the
//!   earlier purchases it overlaps (`bought`).
//! - `POST /submit` with `{ quote, confirm_usd, again? }`: prices the quoted
//!   request again and submits it only if the price is unchanged and, when it
//!   costs anything, `confirm_usd` is exactly that price. Never what's had
//!   already (see `had`): it's refused if it overlaps anything the store
//!   holds, any job on Databento's list that isn't expired, or any purchase
//!   in the ledger, or if any of those can't be read in full. A purchase in
//!   the ledger whose job is expired or no longer listed may be bought again
//!   only by naming its job in `again` (`["OPRA-…"]`). A submission is never
//!   retried; if its outcome is uncertain it blocks until found on the job
//!   list or resolved with `POST /ledger/resolve`.
//! - `POST /price-missing` with `{ dataset, schema, symbol, start, end, rule? }`
//!   for an option parent (`SPY.OPT`): for each day, the contracts `rule`
//!   wants (from the store's definitions of that day) less those the store
//!   holds and those in jobs on Databento's list, priced as one
//!   contract-list request per day (decision 0018). `POST /submit-missing`
//!   with `{ quote, confirm_usd }` plans it again and submits it only if
//!   every day and the price are unchanged. One purchase at a time: both
//!   submit paths hold one lock from their last check to the ledger's last
//!   word.
//! - `POST /ledger/resolve` with `{ request, at, job, note }`: the operator's
//!   word on a submission whose outcome isn't known.
//! - `POST /download/{job}`: streams the job's files into the store, each
//!   checked against Databento's size and SHA-256, then has the store file
//!   the batch. Jobs submitted here download by themselves once done.
//!   Downloading an already purchased job never charges again.
//!
//! The API key comes from `DATABENTO_API_KEY` or `DATABENTO_API_KEY_FILE`.
//! The store's write token, when the store requires one, from
//! `JOBS_STORE_TOKEN` or `JOBS_STORE_TOKEN_FILE`: sent, as a bearer token, on
//! the uploads and filing only, never to Databento.

mod api;
#[cfg(test)]
mod guard_tests;
mod had;
mod ledger;
mod missing;

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tinytrader_observe::{lock, secret};

use axum::extract::{Path as UrlPath, Query, State as AxState};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use clap::Parser;
use databento::HistoricalClient;
use databento::dbn::{SType, Schema};
use databento::historical::batch::{BatchJob, JobState, SubmitJobParams};
use databento::historical::metadata::GetCostParams;
use futures_util::StreamExt as _;
use serde::Deserialize;
use serde_json::{Value, json};
use tinytrader_observe::{Events, Observer, State, listen_address};

/// Databento batch jobs, downloaded into the store.
#[derive(Parser, Debug)]
#[command(version, about)]
struct Args {
    /// The store's address.
    #[arg(long, env = "JOBS_STORE", default_value = "http://127.0.0.1:9120")]
    store: String,

    /// How often to refresh the jobs list, in seconds.
    #[arg(long, env = "JOBS_REFRESH_SECONDS", default_value_t = 30)]
    refresh: u64,

    /// Capture services to yield to (decision 0011), as NAME=URL of each
    /// one's observe endpoint, separated by `;`. While any of them is live
    /// (its health says anything but idle), the service stays off the
    /// network: it doesn't poll Databento, and downloads wait. One that
    /// doesn't answer isn't running, so isn't capturing.
    #[arg(long, env = "JOBS_YIELD_TO", value_delimiter = ';')]
    yield_to: Vec<String>,

    /// The capture hours (New York time, e.g. `07:55-17:05`, set relative to
    /// the regular session), used to look ahead: a file isn't started if
    /// those hours begin before it could finish. Trading days come from
    /// `--calendar`.
    #[arg(long, env = "JOBS_CAPTURE_HOURS")]
    capture_hours: Option<tinytrader_schedule::Schedule>,

    /// Market calendar (`{ "days": [...] }`); without it, every weekday is a
    /// trading day.
    #[arg(long, env = "JOBS_CALENDAR")]
    calendar: Option<String>,

    /// The submissions ledger (JSON lines): each submission is written down
    /// before it is sent. Without one, nothing is submitted.
    #[arg(long, env = "JOBS_LEDGER")]
    ledger: Option<std::path::PathBuf>,
}

/// How long a price stays good for submitting.
const QUOTE_TTL: Duration = Duration::from_secs(15 * 60);

/// A request as the form sends it.
#[derive(Clone, Debug, Deserialize)]
struct Request {
    dataset: String,
    schema: String,
    symbols: String,
    #[serde(default = "raw_symbol")]
    stype_in: String,
    /// First day, `YYYY-MM-DD`.
    start: String,
    /// Last day, inclusive.
    end: String,
}

fn raw_symbol() -> String {
    "raw_symbol".into()
}

/// A request checked and turned into the API's terms.
struct Checked {
    dataset: String,
    schema: Schema,
    symbols: Vec<String>,
    stype_in: SType,
    start: time::Date,
    /// Exclusive.
    end: time::Date,
}

impl Request {
    fn check(&self) -> Result<Checked, String> {
        let fmt = time::macros::format_description!("[year]-[month]-[day]");
        let day = |s: &str| {
            time::Date::parse(s.trim(), fmt)
                .map_err(|_| format!("{s:?} is not a date (YYYY-MM-DD)"))
        };
        let (start, last) = (day(&self.start)?, day(&self.end)?);
        if last < start {
            return Err("the last day is before the first".into());
        }
        let symbols: Vec<String> = self
            .symbols
            .split([',', ' '])
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
            .collect();
        if symbols.is_empty() {
            // Never a whole dataset by accident.
            return Err("name a symbol".into());
        }
        if symbols.len() > 1 {
            // The store files one symbol per batch (decision 0002).
            return Err("one symbol per request: the store files each symbol on its own, so request each separately".into());
        }
        let stype_in: SType = self
            .stype_in
            .parse()
            .map_err(|_| format!("unknown symbol type {}", self.stype_in))?;
        // Only these can be checked against what's held (decision 0019).
        if !matches!(stype_in, SType::Parent | SType::RawSymbol) {
            return Err(format!(
                "a request by {} can't be checked against what's held, so it isn't bought here; use parent or raw_symbol",
                stype_in.as_str()
            ));
        }
        let symbols = if stype_in == SType::Parent {
            symbols.iter().map(|s| s.to_ascii_uppercase()).collect()
        } else {
            symbols
        };
        Ok(Checked {
            dataset: self.dataset.trim().to_ascii_uppercase(),
            schema: self
                .schema
                .parse()
                .map_err(|_| format!("unknown schema {}", self.schema))?,
            stype_in,
            symbols,
            start,
            end: last.next_day().ok_or("bad last day")?,
        })
    }
}

impl Checked {
    fn window(&self) -> had::Window {
        (had::day_window(self.start).0, had::day_window(self.end).0)
    }
}

struct Quote {
    request: Request,
    cost_usd: f64,
    at: Instant,
}

/// A download into the store.
#[derive(Clone, Default)]
struct Transfer {
    state: String,
    file: String,
    done_bytes: u64,
    total_bytes: u64,
    error: Option<String>,
}

struct Jobs {
    db: Arc<dyn api::Databento>,
    /// Held across a whole purchase, from its last check to the ledger's
    /// last word, by `/submit` and `/submit-missing` alike.
    purchase: tokio::sync::Mutex<()>,
    /// Databento's dataset list, and when it was read.
    dataset_list: Mutex<Option<(Instant, Vec<String>)>>,
    key: String,
    http: reqwest::Client,
    store: String,
    /// The store's write token, if it requires one.
    store_token: Option<String>,
    quotes: Mutex<HashMap<String, Quote>>,
    missing_quotes: Mutex<HashMap<String, MissingQuote>>,
    transfers: Mutex<HashMap<String, Transfer>>,
    /// Jobs submitted here, downloaded once done.
    follow: Mutex<HashSet<String>>,
    /// Submissions, written down before they are sent (see `ledger`).
    ledger: Mutex<Option<ledger::Ledger>>,
    /// Full job details, kept once a job is done (they no longer change).
    details: Mutex<HashMap<String, BatchJob>>,
    /// Bytes to download (compressed, as delivered) of done jobs.
    delivered: Mutex<HashMap<String, u64>>,
    listed: Mutex<Value>,
    one_at_a_time: tokio::sync::Semaphore,
    /// While in the pause window, when it ends (ns since the epoch); else 0.
    /// Nonzero while yielding to live capture.
    paused_until: AtomicU64,
    yield_to: Vec<(String, String)>,
    capture_hours: Option<tinytrader_schedule::Schedule>,
    calendar: Mutex<tinytrader_schedule::Calendar>,
    events: Events,
    next_quote: AtomicU64,
}

impl Jobs {
    fn new(
        db: Arc<dyn api::Databento>,
        key: String,
        store: &str,
        store_token: Option<String>,
        ledger: Option<ledger::Ledger>,
    ) -> Self {
        Self {
            db: Arc::new(api::Timed::new(db)),
            purchase: tokio::sync::Mutex::new(()),
            dataset_list: Mutex::default(),
            key,
            http: reqwest::Client::builder()
                .connect_timeout(Duration::from_secs(10))
                .build()
                .expect("http client"),
            store: store.trim_end_matches('/').to_owned(),
            store_token,
            quotes: Mutex::default(),
            missing_quotes: Mutex::default(),
            transfers: Mutex::default(),
            follow: Mutex::default(),
            ledger: Mutex::new(ledger),
            details: Mutex::default(),
            delivered: Mutex::default(),
            listed: Mutex::new(json!({ "jobs": [], "updated_at": null })),
            one_at_a_time: tokio::sync::Semaphore::new(1),
            events: Events::default(),
            next_quote: AtomicU64::new(1),
            paused_until: AtomicU64::new(0),
            yield_to: Vec::new(),
            capture_hours: None,
            calendar: Mutex::new(tinytrader_schedule::Calendar::default()),
        }
    }

    /// A write to the store, with its token when it requires one.
    fn store_write(&self, req: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        match &self.store_token {
            Some(t) => req.bearer_auth(t),
            None => req,
        }
    }
}

type Shared = Arc<Jobs>;
type Reply = (StatusCode, Json<Value>);

fn refused(code: StatusCode, why: impl Into<String>) -> Reply {
    (code, Json(json!({ "error": why.into() })))
}

#[tokio::main]
async fn main() {
    let args = Args::parse();
    let _ = rustls::crypto::ring::default_provider().install_default();
    let key = match secret("DATABENTO_API_KEY") {
        Ok(k) => k,
        Err(e) => {
            eprintln!("databento-jobs: {e}");
            std::process::exit(2);
        }
    };
    let store_token = match tinytrader_observe::optional_secret("JOBS_STORE_TOKEN") {
        Ok(t) => t,
        Err(e) => {
            eprintln!("databento-jobs: {e}");
            std::process::exit(2);
        }
    };
    let db = match HistoricalClient::builder()
        .key(&key)
        .and_then(databento::historical::ClientBuilder::build)
    {
        Ok(c) => c,
        Err(e) => {
            eprintln!("databento-jobs: {e}");
            std::process::exit(2);
        }
    };
    let ledger = match &args.ledger {
        Some(path) => match ledger::Ledger::open(path) {
            Ok(l) => Some(l),
            Err(e) => {
                eprintln!("databento-jobs: cannot open the submissions ledger: {e}");
                std::process::exit(2);
            }
        },
        None => None,
    };
    if let Some(l) = &ledger {
        for e in l.written_off() {
            eprintln!(
                "databento-jobs: {} was written off as not submitted by an earlier version without anyone checking; it blocks again until found on Databento's list or resolved (POST /ledger/resolve)",
                e.request
            );
        }
    }
    let mut jobs = Jobs::new(
        Arc::new(api::Live(tokio::sync::Mutex::new(db))),
        key,
        &args.store,
        store_token,
        ledger,
    );
    jobs.yield_to = args
        .yield_to
        .iter()
        .filter_map(|y| {
            y.split_once('=').map(|(n, u)| {
                (
                    n.trim().to_owned(),
                    u.trim().trim_end_matches('/').to_owned(),
                )
            })
        })
        .collect();
    jobs.capture_hours = args.capture_hours;
    let jobs = Arc::new(jobs);
    let listen = listen_address("JOBS_LISTEN", "127.0.0.1:9121");
    let observer = Observer::new("databento-jobs", env!("CARGO_PKG_VERSION"));
    let mut schedule = vec![
        json!({ "name": "Databento job list", "kind": "interval", "every_seconds": args.refresh, "when": format!("every {} s, outside market hours", args.refresh) }),
        json!({ "name": "Download of jobs submitted here", "kind": "event", "when": "when Databento finishes them, outside market hours" }),
    ];
    if !args.yield_to.is_empty() {
        let names: Vec<&str> = args
            .yield_to
            .iter()
            .filter_map(|y| y.split_once('=').map(|(n, _)| n.trim()))
            .collect();
        let mut entry = json!({ "name": "Yields to live capture", "kind": "yield", "class": "background", "yields_to": names, "when": format!("off the network while any of {} is live", names.join(", ")) });
        if let Some(h) = args.capture_hours {
            entry["hours"] = json!(h.to_string());
        }
        schedule.insert(0, entry);
    }
    observer.describe(json!({
        "role": "Databento batch jobs: prices and submits requests, and downloads finished jobs into the store.",
        "upstream": [{ "id": "databento", "label": "databento", "sub": "batch API", "kind": "flow", "via": "https" }],
        "streams": { "jobs": "/jobs", "events": "/events" },
        "schedule": schedule,
    }));

    let app = Router::new()
        .route(
            "/jobs",
            get(|AxState(j): AxState<Shared>| async move { Json(jobs_view(&j)) }),
        )
        .route("/datasets", get(datasets))
        .route("/schemas", get(schemas))
        .route("/price", post(price))
        .route("/submit", post(submit))
        .route("/price-missing", post(price_missing))
        .route("/submit-missing", post(submit_missing))
        .route("/ledger/resolve", post(resolve))
        .route("/download/{job}", post(download))
        // A web page of another origin can't submit (or pay for) anything.
        .layer(axum::middleware::from_fn(
            tinytrader_observe::guard::same_origin_writes,
        ))
        .with_state(jobs.clone())
        .merge(observer.router())
        .merge(jobs.events.router());
    let listener = match tokio::net::TcpListener::bind(&listen).await {
        Ok(l) => l,
        Err(e) => {
            eprintln!("databento-jobs: cannot listen on {listen}: {e}");
            std::process::exit(2);
        }
    };
    eprintln!(
        "databento-jobs: store at {}; endpoints on http://{listen}",
        jobs.store
    );
    tokio::spawn(async move {
        if let Err(e) = axum::serve(listener, app).await {
            eprintln!("databento-jobs: http server stopped: {e}");
        }
    });

    let every = Duration::from_secs(args.refresh.max(10));
    let mut calendar = (tinytrader_schedule::Calendar::default(), None::<Instant>);
    loop {
        if let Some(url) = &args.calendar
            && calendar
                .1
                .is_none_or(|at| at.elapsed() > Duration::from_secs(3600))
            && let Ok(r) = jobs.http.get(url).send().await
            && let Ok(c) = r.json::<tinytrader_schedule::Calendar>().await
            && !c.days.is_empty()
        {
            *lock(&jobs.calendar) = c.clone();
            calendar = (c, Some(Instant::now()));
        }
        // Background work (decision 0011): off the network while capture is live.
        if let Some(who) = live_capture(&jobs).await {
            jobs.paused_until.store(1, Ordering::Relaxed);
            observer.set_state(State::Idle, &format!("yielding: {who} is live"));
            tokio::time::sleep(Duration::from_secs(15)).await;
            continue;
        }
        jobs.paused_until.store(0, Ordering::Relaxed);
        match refresh(&jobs).await {
            Ok(r) if !r.unread.is_empty() => observer.set_state(
                State::Degraded,
                &format!(
                    "{} jobs; Databento couldn't be read for {}",
                    r.jobs,
                    r.unread.join(", ")
                ),
            ),
            Ok(r) if !r.stale.is_empty() => observer.set_state(
                State::Ready,
                &format!(
                    "{} jobs (Databento slow; last-known details for {})",
                    r.jobs,
                    r.stale.len()
                ),
            ),
            Ok(r) => observer.set_state(State::Ready, &format!("{} jobs", r.jobs)),
            Err(e) => observer.set_state(State::Degraded, &e),
        }
        tokio::time::sleep(every).await;
    }
}

/// Resolve the ledger's unresolved submissions against the job list
/// (`out`: every job read, expired ones included), and follow (download when
/// done) every job submitted here. Nothing is resolved while any job couldn't
/// be read (`complete` false): the unread one may be the match. A submission
/// not found stays unresolved, blocking what it overlaps, until it shows or
/// the operator resolves it; never by time alone. Nor while a purchase is
/// in flight: its job may show before its own answer is written.
fn reconcile(jobs: &Shared, out: &[BatchJob], complete: bool) {
    let turn = jobs.purchase.try_lock();
    let mut guard = lock(&jobs.ledger);
    let Some(l) = guard.as_mut() else { return };
    for e in l
        .unresolved()
        .into_iter()
        .filter(|_| complete && turn.is_ok())
    {
        let claimed = l.submitted();
        // Its job was received after it was sent (with a margin for clocks).
        let sent = l.sent_at(&e.request).unwrap_or(e.at).min(e.at);
        let Some(j) = out.iter().find(|j| {
            !claimed.contains(&j.id)
                && had::same_request(&e.request, j)
                && j.ts_received.unix_timestamp() as f64 >= sent - 120.0
        }) else {
            continue;
        };
        let step = ledger::Entry {
            at: tinytrader_observe::unix_seconds(),
            request: e.request.clone(),
            state: "submitted".into(),
            job: Some(j.id.clone()),
            cost_usd: e.cost_usd,
            note: Some("found in Databento's job list".into()),
        };
        let text = format!("reconciled a submission: {}", j.id);
        match l.append(step) {
            Ok(()) => jobs.events.push(text.clone(), text),
            Err(err) => eprintln!("databento-jobs: cannot write the ledger: {err}"),
        }
    }
    lock(&jobs.follow).extend(l.submitted());
}

/// The first line of an API error, without an HTML error page.
fn brief(e: &str) -> String {
    e.split('<')
        .next()
        .unwrap_or(e)
        .trim()
        .trim_end_matches(':')
        .chars()
        .take(120)
        .collect()
}

/// How a refresh went: how many jobs, and which Databento couldn't answer for.
struct Refreshed {
    jobs: usize,
    stale: Vec<String>,
    unread: Vec<String>,
}

/// Read the jobs list and the store's holdings, reconcile the ledger, and
/// start downloads of followed jobs that are done.
async fn refresh(jobs: &Shared) -> Result<Refreshed, String> {
    let short = jobs
        .db
        .list_jobs(&api::every_job())
        .await
        .map_err(|e| format!("list jobs: {e}"))?;
    // Expired jobs are read only to match a submission whose outcome isn't
    // known: those received since the earliest of them.
    let since = lock(&jobs.ledger)
        .as_ref()
        .and_then(|l| l.unresolved().iter().map(|e| e.at).reduce(f64::min));
    // What the store already holds, first: only jobs not on disk need
    // their file lists (for the size a download would take).
    let held: Value = match jobs
        .http
        .get(format!("{}/batches/databento", jobs.store))
        .send()
        .await
    {
        Ok(r) => r
            .json::<Value>()
            .await
            .map(|v| v["batches"].clone())
            .unwrap_or(Value::Null),
        Err(_) => Value::Null,
    };
    let (mut out, mut expired) = (Vec::new(), Vec::new());
    // A finished job's details never change; others are read again each
    // round. When Databento can't answer for one, its last-known details
    // stand in, so one slow call doesn't blank the list.
    let (mut stale, mut unread) = (Vec::new(), Vec::new());
    for s in &short {
        if s.state == JobState::Expired
            && since.is_none_or(|t| (s.ts_received.unix_timestamp() as f64) < t - 120.0)
        {
            continue;
        }
        let known = lock(&jobs.details).get(&s.id).cloned();
        let job = match known {
            Some(j) if matches!(j.state, JobState::Done | JobState::Expired) => BatchJob {
                state: s.state,
                ..j
            },
            known => {
                let fetched = jobs.db.job(&s.id).await;
                match (fetched, known) {
                    (Ok(j), _) => j,
                    (Err(e), Some(j)) => {
                        stale.push(format!("{} ({})", s.id, brief(&e.to_string())));
                        j
                    }
                    (Err(e), None) => {
                        unread.push(format!("{} ({})", s.id, brief(&e.to_string())));
                        continue;
                    }
                }
            }
        };
        lock(&jobs.details).insert(job.id.clone(), job.clone());
        if job.state == JobState::Expired {
            expired.push(job);
            continue;
        }
        if job.state == JobState::Done
            && held[&job.id].is_null()
            && !lock(&jobs.delivered).contains_key(&job.id)
        {
            let files = jobs.db.files(&job.id).await;
            if let Ok(files) = files {
                lock(&jobs.delivered).insert(job.id.clone(), files.iter().map(|f| f.size).sum());
            }
        }
        out.push(job);
    }
    let delivered = lock(&jobs.delivered).clone();
    let rows: Vec<Value> = out
        .iter()
        .map(|j| row(j, &held, delivered.get(&j.id).copied()))
        .collect();
    *lock(&jobs.listed) = json!({ "jobs": rows, "store_reachable": !held.is_null(), "updated_at": tinytrader_observe::unix_seconds() });

    // Submissions whose outcome was unknown: match each to the job it
    // created.
    reconcile(
        jobs,
        &[out.as_slice(), &expired].concat(),
        unread.is_empty(),
    );

    let ready: Vec<String> = out
        .iter()
        .filter(|j| {
            j.state == JobState::Done
                && lock(&jobs.follow).contains(&j.id)
                && held[&j.id].is_null()
                && unfileable(j).is_none()
        })
        .map(|j| j.id.clone())
        .collect();
    for id in ready {
        lock(&jobs.follow).remove(&id);
        start_download(jobs, id);
    }
    Ok(Refreshed {
        jobs: out.len(),
        stale,
        unread,
    })
}

/// `bytes` is what downloading delivers (compressed); Databento's own job
/// size is the uncompressed size.
/// Why the store would refuse to file this job, if it would. Checked before
/// downloading, so nothing is fetched that can't be filed.
fn unfileable(j: &BatchJob) -> Option<String> {
    use databento::dbn::{Compression, Encoding};
    if j.encoding != Encoding::Dbn || j.compression != Compression::Zstd {
        return Some("not DBN with zstd; the store files only .dbn.zst".into());
    }
    if j.split_symbols || j.split_duration.as_str() != "day" {
        return Some(
            "not split by day (or split by symbol); the store files one file per day".into(),
        );
    }
    let n = j.symbols.to_api_string().split(',').count();
    (n > 1).then(|| format!("requests {n} symbols; the store files one symbol per batch"))
}

fn row(j: &BatchJob, held: &Value, delivered: Option<u64>) -> Value {
    let state = match j.state {
        JobState::Queued => "queued",
        JobState::Processing => "processing",
        JobState::Done => "done",
        JobState::Expired => "expired",
    };
    json!({
        "id": j.id,
        "state": state,
        "progress": j.progress,
        "dataset": j.dataset,
        "schema": j.schema.as_str(),
        "symbols": j.symbols.to_api_string(),
        "stype_in": j.stype_in.as_str(),
        "start": j.start.unix_timestamp(),
        "end": j.end.unix_timestamp(),
        "cost_usd": j.cost_usd,
        "bytes": delivered,
        "uncompressed_bytes": j.actual_size,
        "received": j.ts_received.unix_timestamp(),
        "expires": j.ts_expiration.map(time::OffsetDateTime::unix_timestamp),
        "split": j.split_duration.as_str(),
        "split_symbols": j.split_symbols,
        "unfileable": unfileable(j),
        "held": held.get(&j.id).map(|h| json!({ "volume": h["volume"], "how": h["how"] })),
    })
}

fn jobs_view(jobs: &Jobs) -> Value {
    let mut v = lock(&jobs.listed).clone();
    let transfers = lock(&jobs.transfers).clone();
    let follow = lock(&jobs.follow).clone();
    for r in v["jobs"].as_array_mut().into_iter().flatten() {
        let id = r["id"].as_str().unwrap_or_default().to_owned();
        if let Some(t) = transfers.get(&id) {
            r["transfer"] = json!({ "state": t.state, "file": t.file, "done_bytes": t.done_bytes, "total_bytes": t.total_bytes, "error": t.error });
        }
        r["follow"] = json!(follow.contains(&id));
    }
    v
}

async fn datasets(AxState(jobs): AxState<Shared>) -> Reply {
    match jobs.db.datasets().await {
        Ok(d) => (StatusCode::OK, Json(json!({ "datasets": d }))),
        Err(e) => refused(StatusCode::BAD_GATEWAY, e.to_string()),
    }
}

#[derive(Deserialize)]
struct DatasetQuery {
    dataset: String,
}

async fn schemas(AxState(jobs): AxState<Shared>, Query(q): Query<DatasetQuery>) -> Reply {
    match jobs.db.schemas(&q.dataset).await {
        Ok(s) => (
            StatusCode::OK,
            Json(json!({ "schemas": s.iter().map(Schema::as_str).collect::<Vec<_>>() })),
        ),
        Err(e) => refused(StatusCode::BAD_GATEWAY, e.to_string()),
    }
}

/// The cost of a request, in US dollars, and its billable size.
async fn cost(
    jobs: &Jobs,
    dataset: &str,
    schema: Schema,
    symbols: &[String],
    stype_in: SType,
    w: had::Window,
) -> Result<(f64, u64), String> {
    let params = GetCostParams::builder()
        .dataset(dataset)
        .schema(schema)
        .symbols(symbols.to_vec())
        .stype_in(stype_in)
        .date_time_range((had::at(w.0), had::at(w.1)))
        .build();
    jobs.db.cost(&params).await
}

/// Before pricing: refuse a request outside what Databento has for the
/// schema, and warn about requested days it marks degraded, pending or
/// missing.
async fn availability(jobs: &Jobs, c: &Checked) -> Result<Vec<String>, String> {
    use databento::historical::metadata::{DatasetCondition, GetDatasetConditionParams};
    let mut warnings = Vec::new();
    match jobs.db.range(&c.dataset).await {
        Ok(r) => {
            let (from, to) = r
                .range_by_schema
                .get(&c.schema)
                .map_or((r.start, r.end), |x| (x.start, x.end));
            let (want_from, want_to) = (
                c.start.midnight().assume_utc(),
                c.end.midnight().assume_utc(),
            );
            if want_from < from.replace_time(time::Time::MIDNIGHT) || want_to > to {
                let last = (to - time::Duration::nanoseconds(1)).date();
                return Err(format!(
                    "Databento has {} {} from {} to {last}",
                    c.dataset,
                    c.schema.as_str(),
                    from.date()
                ));
            }
        }
        Err(e) => warnings.push(format!("couldn't check the available range: {e}")),
    }
    let params = GetDatasetConditionParams::builder()
        .dataset(&c.dataset)
        .date_range(c.start..c.end)
        .build();
    match jobs.db.conditions(&params).await {
        Ok(days) => {
            for (cond, label) in [
                (DatasetCondition::Degraded, "degraded"),
                (DatasetCondition::Pending, "pending"),
                (DatasetCondition::Missing, "missing"),
            ] {
                let list: Vec<String> = days
                    .iter()
                    .filter(|d| d.condition == cond)
                    .map(|d| d.date.to_string())
                    .collect();
                if !list.is_empty() {
                    warnings.push(format!(
                        "Databento marks {} {label}: {}",
                        if list.len() == 1 {
                            "1 day"
                        } else {
                            "these days"
                        },
                        list.join(", ")
                    ));
                }
            }
        }
        Err(e) => warnings.push(format!("couldn't check the days' condition: {e}")),
    }
    Ok(warnings)
}

/// A store answer about what it holds, refused unless it is 200 (the store
/// answers 503 until its first scan) and every volume the store knows is
/// present: `missing_volumes` empty, where its absence means a store too old
/// to say.
async fn store_json(jobs: &Jobs, path: &str, query: &[(&str, &str)]) -> Result<Value, String> {
    let url = reqwest::Url::parse_with_params(&format!("{}{path}", jobs.store), query)
        .map_err(|e| format!("the store's {path}: {e}"))?;
    let r = jobs
        .http
        .get(url)
        .timeout(Duration::from_secs(120))
        .send()
        .await
        .map_err(|e| format!("the store's {path}: {e}"))?;
    if !r.status().is_success() {
        return Err(format!("the store's {path}: HTTP {}", r.status()));
    }
    let v: Value = r
        .json()
        .await
        .map_err(|e| format!("the store's {path}: {e}"))?;
    match v["missing_volumes"].as_array() {
        Some(m) if m.is_empty() => Ok(v),
        Some(m) => Err(format!(
            "the store is missing volumes ({}), so what it holds isn't known",
            m.iter()
                .map(|x| x.as_str().map_or_else(|| x.to_string(), str::to_owned))
                .collect::<Vec<_>>()
                .join(", ")
        )),
        None => Err(format!(
            "the store's {path} doesn't say which volumes are missing (an older store), so what it holds isn't known"
        )),
    }
}

/// Databento's jobs that aren't expired, in their state as listed now, and
/// their IDs, from the whole job list read now. Fails unless every one of
/// them can be read, so nothing is bought blind.
async fn live_jobs(jobs: &Jobs) -> Result<(Vec<BatchJob>, HashSet<String>), String> {
    let short = jobs
        .db
        .list_jobs(&api::every_job())
        .await
        .map_err(|e| format!("Databento's job list: {e}"))?;
    let mut out = Vec::new();
    for sj in short.iter().filter(|sj| sj.state != JobState::Expired) {
        // A job's request (dataset, schema, symbols, times) never changes.
        let known = lock(&jobs.details).get(&sj.id).cloned();
        let job = match known {
            Some(j) => j,
            None => {
                let j = jobs
                    .db
                    .job(&sj.id)
                    .await
                    .map_err(|e| format!("Databento's job {}: {e}", sj.id))?;
                lock(&jobs.details).insert(j.id.clone(), j.clone());
                j
            }
        };
        out.push(BatchJob {
            state: sj.state,
            ..job
        });
    }
    let ids = out.iter().map(|j| j.id.clone()).collect();
    Ok((out, ids))
}

/// What's had already of `target` (a parent, or a plain symbol) in this
/// dataset and schema from `start` to `end` (exclusive): the store's
/// holdings, Databento's jobs that aren't expired, and every purchase in the
/// ledger, read now (see `had`). Fails when any of it can't be read in full.
async fn had(
    jobs: &Jobs,
    dataset: &str,
    schema: Schema,
    target: &str,
    start: time::Date,
    end: time::Date,
) -> Result<Vec<had::Piece>, String> {
    let last = end.previous_day().unwrap_or(end).to_string();
    let catalog = store_json(jobs, "/catalog", &[]).await?;
    let coverage = store_json(
        jobs,
        "/coverage",
        &[
            ("provider", "databento"),
            ("dataset", dataset),
            ("symbol", target),
            ("schema", schema.as_str()),
            ("start", &start.to_string()),
            ("end", &last),
        ],
    )
    .await?;
    let (live, ids) = live_jobs(jobs).await?;
    let purchases = lock(&jobs.ledger)
        .as_ref()
        .map(ledger::Ledger::purchases)
        .unwrap_or_default();
    let range = (had::day_window(start).0, had::day_window(end).0);
    let schema = schema.as_str();
    let mut pieces = had::store_pieces(&catalog, &coverage, dataset, schema, target, range);
    pieces.extend(had::job_pieces(&live, dataset, schema, target));
    pieces.extend(had::ledger_pieces(
        &purchases, dataset, schema, target, &ids,
    ));
    Ok(pieces)
}

/// The dataset in Databento's spelling, refused unless it is on Databento's
/// dataset list (read at most hourly).
async fn known_dataset(jobs: &Jobs, d: &str) -> Result<String, (StatusCode, String)> {
    let d = d.trim().to_ascii_uppercase();
    let cached = lock(&jobs.dataset_list)
        .as_ref()
        .filter(|(at, _)| at.elapsed() < Duration::from_secs(3600))
        .map(|(_, l)| l.clone());
    let list = match cached {
        Some(l) => l,
        None => {
            let l = jobs.db.datasets().await.map_err(|e| {
                (
                    StatusCode::SERVICE_UNAVAILABLE,
                    format!("couldn't read Databento's dataset list: {e}"),
                )
            })?;
            *lock(&jobs.dataset_list) = Some((Instant::now(), l.clone()));
            l
        }
    };
    if list.contains(&d) {
        Ok(d)
    } else {
        Err((
            StatusCode::BAD_REQUEST,
            format!("{d} isn't one of Databento's datasets"),
        ))
    }
}

/// Why a request overlapping `ov` may not be bought, if it may not: anything
/// held, pending on Databento or bought here; or bought here and gone
/// (expired, or no longer listed) unless `again` names exactly those jobs.
fn refuse_overlaps(ov: &[&had::Piece], again: &HashSet<String>) -> Option<Reply> {
    use had::Kind;
    let list = |f: &dyn Fn(&Kind) -> bool| -> Vec<String> {
        ov.iter()
            .filter(|p| f(&p.kind))
            .map(|p| p.what.clone())
            .collect()
    };
    let held = list(&|k| *k == Kind::Held);
    if !held.is_empty() {
        return Some(refused(
            StatusCode::CONFLICT,
            format!(
                "the store already holds some of this: {}; request only the rest (for an option parent, /price-missing buys only what's missing)",
                held.join("; ")
            ),
        ));
    }
    let bought = list(&|k| matches!(k, Kind::Pending | Kind::Bought));
    if !bought.is_empty() {
        return Some(refused(
            StatusCode::CONFLICT,
            format!("already bought: {}", bought.join("; ")),
        ));
    }
    let gone: std::collections::BTreeSet<&str> = ov
        .iter()
        .filter_map(|p| match &p.kind {
            Kind::Gone(j) => Some(j.as_str()),
            _ => None,
        })
        .collect();
    let unnamed: Vec<&str> = gone
        .iter()
        .copied()
        .filter(|j| !again.contains(*j))
        .collect();
    if !unnamed.is_empty() {
        return Some(refused(
            StatusCode::CONFLICT,
            format!(
                "this overlaps earlier purchases that are expired or no longer on Databento's list: {}. If they were never filed, submit with \"again\": {} to buy them again",
                list(&|k| matches!(k, Kind::Gone(_))).join("; "),
                json!(unnamed)
            ),
        ));
    }
    let mut extra: Vec<&String> = again
        .iter()
        .filter(|j| !gone.contains(j.as_str()))
        .collect();
    extra.sort();
    (!extra.is_empty()).then(|| {
        refused(
            StatusCode::BAD_REQUEST,
            format!(
                "\"again\" names {}, which this request doesn't buy again",
                json!(extra)
            ),
        )
    })
}

/// Refuses a price that isn't a number of dollars, that changed since the
/// quote, that is paused, or that isn't confirmed exactly. Only an exact $0
/// is free.
fn check_price(now: f64, quoted: f64, confirm: Option<f64>) -> Option<Reply> {
    let same = |a: f64, b: f64| (a - b).abs() <= 1e-9;
    if !now.is_finite() || now < 0.0 {
        return Some(refused(
            StatusCode::BAD_GATEWAY,
            format!("Databento priced this at {now}, which isn't a price; nothing is submitted"),
        ));
    }
    if !same(now, quoted) {
        return Some(refused(
            StatusCode::CONFLICT,
            format!("the price changed from ${quoted:.2} to ${now:.2}; price the request again"),
        ));
    }
    if let Some(r) = paused(now) {
        return Some(r);
    }
    (now != 0.0 && confirm.is_none_or(|c| !same(c, now))).then(|| {
        refused(
            StatusCode::PRECONDITION_REQUIRED,
            format!("this costs ${now:.2}; confirm that amount to submit it"),
        )
    })
}

/// A request for what's missing of an option parent (decision 0018).
#[derive(Clone, Debug, Deserialize)]
struct MissingRequest {
    dataset: String,
    schema: String,
    /// The parent, such as `SPY.OPT`.
    symbol: String,
    /// First day, `YYYY-MM-DD`.
    start: String,
    /// Last day, inclusive.
    end: String,
    #[serde(default)]
    rule: missing::Rule,
}

struct MissingQuote {
    request: MissingRequest,
    days: Vec<missing::Day>,
    cost_usd: f64,
    at: Instant,
}

/// Plan a request for what's missing: for each day the store has the
/// parent's definitions for, the contracts wanted less what's had (`had`),
/// priced per day: one request for the contracts missing all day, and one
/// for each stretch of hours missing of contracts partly had. Returns the
/// requests and notes (days with nothing to buy, days without definitions).
async fn plan(
    jobs: &Jobs,
    r: &MissingRequest,
) -> Result<(Vec<missing::Day>, Vec<String>), (StatusCode, String)> {
    let bad = |e: String| (StatusCode::BAD_REQUEST, e);
    let down = |e: String| (StatusCode::SERVICE_UNAVAILABLE, e);
    let fmt = time::macros::format_description!("[year]-[month]-[day]");
    let parse = |s: &str| {
        time::Date::parse(s.trim(), fmt)
            .map_err(|_| bad(format!("{s:?} is not a date (YYYY-MM-DD)")))
    };
    let (start, last) = (parse(&r.start)?, parse(&r.end)?);
    if last < start {
        return Err(bad("the last day is before the first".into()));
    }
    if (last - start).whole_days() > 366 {
        return Err(bad("at most a year at a time".into()));
    }
    if !r.symbol.ends_with(".OPT") {
        return Err(bad(
            "only option parents (ROOT.OPT) are bought by contract".into()
        ));
    }
    let schema: Schema = r
        .schema
        .parse()
        .map_err(|_| bad(format!("unknown schema {}", r.schema)))?;
    let dataset = known_dataset(jobs, &r.dataset).await?;
    let end = last.next_day().ok_or_else(|| bad("bad last day".into()))?;
    let pieces = had(jobs, &dataset, schema, &r.symbol, start, end)
        .await
        .map_err(down)?;
    let get = |url: String| async move {
        let resp = jobs
            .http
            .get(&url)
            .timeout(Duration::from_secs(120))
            .send()
            .await
            .map_err(|e| down(format!("store: {e}")))?;
        if !resp.status().is_success() {
            return Err(down(format!("store: HTTP {} for {url}", resp.status())));
        }
        Ok::<_, (StatusCode, String)>(resp)
    };
    let listing: Value = get(format!(
        "{}/files?provider=databento&pattern=*/{dataset}/{}/definition/*",
        jobs.store, r.symbol
    ))
    .await?
    .json()
    .await
    .map_err(|e| down(format!("store: {e}")))?;
    let defs: HashMap<String, (String, String)> = listing["files"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|f| {
            let path = f["path"].as_str()?;
            let day = path.rsplit('/').next()?.strip_suffix(".dbn.zst")?;
            Some((
                day.to_owned(),
                (f["volume"].as_str()?.to_owned(), path.to_owned()),
            ))
        })
        .collect();

    let mut days = Vec::new();
    let mut notes = Vec::new();
    let mut d = start;
    while d <= last {
        let key = d.to_string();
        let next = d.next_day().ok_or_else(|| bad("bad day".into()))?;
        let Some((volume, path)) = defs.get(&key) else {
            if !matches!(d.weekday(), time::Weekday::Saturday | time::Weekday::Sunday) {
                notes.push(format!(
                    "{key}: the store has no {} definitions for this day; buy those first",
                    r.symbol
                ));
            }
            d = next;
            continue;
        };
        let bytes = get(format!("{}/files/{volume}/{path}", jobs.store))
            .await?
            .bytes()
            .await
            .map_err(|e| down(format!("store: {e}")))?;
        let wanted = missing::wanted(&missing::contracts(&bytes).map_err(down)?, d, &r.rule);
        let whole = had::day_window(d);
        let need = had::need(&wanted, &pieces, whole);
        let mut parts: Vec<(had::Window, Vec<String>)> = Vec::new();
        if !need.whole.is_empty() {
            parts.push((whole, need.whole));
        }
        for (windows, contracts) in need.hours {
            parts.extend(windows.into_iter().map(|w| (w, contracts.clone())));
        }
        if parts.is_empty() {
            notes.push(format!(
                "{key}: nothing to buy ({} wanted, {} held, {} pending or bought)",
                wanted.len(),
                need.skipped_held,
                need.skipped_pending
            ));
        }
        for (window, contracts) in parts {
            for chunk in contracts.chunks(missing::MAX_SYMBOLS) {
                let (usd, bytes) = cost(jobs, &dataset, schema, chunk, SType::RawSymbol, window)
                    .await
                    .map_err(|e| (StatusCode::BAD_GATEWAY, e))?;
                days.push(missing::Day {
                    day: d,
                    window,
                    symbols: chunk.to_vec(),
                    cost_usd: usd,
                    bytes,
                    skipped_held: need.skipped_held,
                    skipped_pending: need.skipped_pending,
                });
            }
        }
        d = next;
    }
    Ok((days, notes))
}

async fn price_missing(AxState(jobs): AxState<Shared>, Json(mut r): Json<MissingRequest>) -> Reply {
    r.dataset = r.dataset.trim().to_ascii_uppercase();
    r.symbol = r.symbol.trim().to_ascii_uppercase();
    let (days, notes) = match plan(&jobs, &r).await {
        Ok(p) => p,
        Err((code, e)) => return refused(code, e),
    };
    let total: f64 = days.iter().map(|d| d.cost_usd).sum();
    let bytes: u64 = days.iter().map(|d| d.bytes).sum();
    let id = format!("m{}", jobs.next_quote.fetch_add(1, Ordering::Relaxed));
    let rows: Vec<Value> = days.iter().map(missing::Day::to_json).collect();
    {
        let mut q = lock(&jobs.missing_quotes);
        q.retain(|_, x| x.at.elapsed() < QUOTE_TTL);
        q.insert(
            id.clone(),
            MissingQuote {
                request: r,
                days,
                cost_usd: total,
                at: Instant::now(),
            },
        );
    }
    reply_ok(
        json!({ "quote": id, "cost_usd": total, "billable_bytes": bytes, "requests": rows.len(), "days": rows, "notes": notes, "valid_seconds": QUOTE_TTL.as_secs() }),
    )
}

fn reply_ok(v: Value) -> Reply {
    (StatusCode::OK, Json(v))
}

#[derive(Deserialize)]
struct SubmitMissing {
    quote: String,
    confirm_usd: Option<f64>,
}

/// Paid purchases are paused until the guards of decision 0019 are built:
/// anything not priced at exactly $0 is refused (a NaN or negative price
/// too); free requests go through.
fn paused(cost_usd: f64) -> Option<Reply> {
    let free = cost_usd == 0.0;
    (!free).then(|| {
        refused(
            StatusCode::FORBIDDEN,
            format!("this costs ${cost_usd:.2}, and paid purchases are paused (decision 0019)"),
        )
    })
}

const NO_LEDGER: &str =
    "no submissions ledger is configured (JOBS_LEDGER), so nothing is submitted";

/// Write a step of `request` to the ledger.
fn write_step(
    jobs: &Jobs,
    request: &str,
    state: &str,
    job: Option<String>,
    cost_usd: f64,
    note: Option<String>,
) -> Result<(), String> {
    lock(&jobs.ledger)
        .as_mut()
        .ok_or_else(|| NO_LEDGER.to_owned())?
        .append(ledger::Entry {
            at: tinytrader_observe::unix_seconds(),
            request: request.to_owned(),
            state: state.into(),
            job,
            cost_usd: Some(cost_usd),
            note,
        })
}

async fn submit_missing(AxState(jobs): AxState<Shared>, Json(s): Json<SubmitMissing>) -> Reply {
    let Some(quote) = lock(&jobs.missing_quotes).remove(&s.quote) else {
        return refused(
            StatusCode::CONFLICT,
            "that price has expired or was already used; price the request again",
        );
    };
    if quote.at.elapsed() >= QUOTE_TTL {
        return refused(
            StatusCode::CONFLICT,
            "that price has expired; price the request again",
        );
    }
    // One purchase at a time, from the last check to the ledger's last word.
    let _turn = jobs.purchase.lock().await;
    if lock(&jobs.ledger).is_none() {
        return refused(StatusCode::SERVICE_UNAVAILABLE, NO_LEDGER);
    }
    // Plan again: what's held, pending, bought and priced may have changed.
    let (days, _) = match plan(&jobs, &quote.request).await {
        Ok(p) => p,
        Err((_, e)) => {
            return refused(
                StatusCode::SERVICE_UNAVAILABLE,
                format!("couldn't plan it again, so nothing is submitted: {e}"),
            );
        }
    };
    let now: f64 = days.iter().map(|d| d.cost_usd).sum();
    let same = days.len() == quote.days.len()
        && days
            .iter()
            .zip(&quote.days)
            .all(|(a, b)| a.day == b.day && a.window == b.window && a.symbols == b.symbols);
    if !same {
        return refused(
            StatusCode::CONFLICT,
            format!(
                "what's missing or its price changed (now ${now:.2} for {} requests); price it again",
                days.len()
            ),
        );
    }
    if let Some(r) = check_price(now, quote.cost_usd, s.confirm_usd) {
        return r;
    }
    let r = &quote.request;
    let Ok(schema) = r.schema.parse::<Schema>() else {
        return refused(StatusCode::BAD_REQUEST, "unknown schema");
    };
    let mut submitted = Vec::new();
    for d in &days {
        let request = ledger::fingerprint(
            &r.dataset,
            schema.as_str(),
            &d.symbols,
            &had::stamp(d.window.0),
            &had::stamp(d.window.1),
        );
        if let Err(e) = write_step(&jobs, &request, "sending", None, d.cost_usd, None) {
            return refused(StatusCode::INTERNAL_SERVER_ERROR, json!({ "error": format!("could not write the submissions ledger, so {} wasn't submitted: {e}", d.day), "submitted": submitted }).to_string());
        }
        let params = SubmitJobParams::builder()
            .dataset(&r.dataset)
            .schema(schema)
            .symbols(d.symbols.clone())
            .stype_in(SType::RawSymbol)
            .date_time_range((had::at(d.window.0), had::at(d.window.1)))
            .build();
        let result = jobs.db.submit(&params).await;
        let (state, job, note) = match &result {
            Ok(job) => ("submitted", Some(job.id.clone()), None),
            Err(e) => ("uncertain", None, Some(brief(e))),
        };
        if let Err(e) = write_step(&jobs, &request, state, job, d.cost_usd, note) {
            // Unwritten, the "sending" step still blocks what it overlaps.
            eprintln!("databento-jobs: cannot write the ledger: {e}");
        }
        match result {
            Ok(job) => {
                lock(&jobs.follow).insert(job.id.clone());
                jobs.events.push(
                    format!(
                        "submitted {} ({} {} {} contracts of {} on {}, ${:.2})",
                        job.id,
                        r.dataset,
                        schema.as_str(),
                        d.symbols.len(),
                        r.symbol,
                        d.day,
                        d.cost_usd
                    ),
                    json!({ "job": job.id, "cost_usd": d.cost_usd }).to_string(),
                );
                submitted.push(json!({ "day": d.day.to_string(), "job": job.id, "contracts": d.symbols.len(), "cost_usd": d.cost_usd }));
            }
            Err(e) => {
                return refused(StatusCode::BAD_GATEWAY, json!({ "error": format!("{}: submission failed or its outcome is unknown ({e}); nothing after it was submitted; check the jobs list", d.day), "submitted": submitted }).to_string());
            }
        }
    }
    reply_ok(json!({ "submitted": submitted, "cost_usd": now }))
}

async fn price(AxState(jobs): AxState<Shared>, Json(request): Json<Request>) -> Reply {
    let checked = match request.check() {
        Ok(c) => c,
        Err(e) => return refused(StatusCode::BAD_REQUEST, e),
    };
    if let Err((code, e)) = known_dataset(&jobs, &checked.dataset).await {
        return refused(code, e);
    }
    let mut warnings = match availability(&jobs, &checked).await {
        Ok(w) => w,
        Err(e) => return refused(StatusCode::BAD_REQUEST, e),
    };
    // What's had already; submitting refuses it, so say so now.
    let window = checked.window();
    let (held, prior) = match had(
        &jobs,
        &checked.dataset,
        checked.schema,
        &checked.symbols[0],
        checked.start,
        checked.end,
    )
    .await
    {
        Ok(pieces) => {
            let ov = had::overlapping(&pieces, &had::Scope::All, window);
            let (held, other): (Vec<&had::Piece>, Vec<&had::Piece>) =
                ov.into_iter().partition(|p| p.kind == had::Kind::Held);
            let mut days: Vec<String> = held
                .iter()
                .map(|p| had::at(p.window.0.max(window.0)).date().to_string())
                .collect();
            days.sort();
            days.dedup();
            (days, other.iter().map(|p| p.what.clone()).collect())
        }
        Err(e) => {
            warnings.push(format!(
                "couldn't check what's held already ({e}); submitting will refuse until it can"
            ));
            (Vec::new(), Vec::new())
        }
    };
    if !held.is_empty() {
        warnings.push(format!(
            "the store already holds some of {} of these days ({}); request only the others (for an option parent, /price-missing buys only what's missing)",
            held.len(),
            held.join(", ")
        ));
    }
    if !prior.is_empty() {
        warnings.push(format!("overlaps earlier purchases: {}", prior.join("; ")));
    }
    let (usd, bytes) = match cost(
        &jobs,
        &checked.dataset,
        checked.schema,
        &checked.symbols,
        checked.stype_in,
        window,
    )
    .await
    {
        Ok(c) => c,
        Err(e) => return refused(StatusCode::BAD_GATEWAY, e),
    };
    let id = format!("q{}", jobs.next_quote.fetch_add(1, Ordering::Relaxed));
    let mut quotes = lock(&jobs.quotes);
    quotes.retain(|_, q| q.at.elapsed() < QUOTE_TTL);
    quotes.insert(
        id.clone(),
        Quote {
            request,
            cost_usd: usd,
            at: Instant::now(),
        },
    );
    (
        StatusCode::OK,
        Json(
            json!({ "quote": id, "warnings": warnings, "cost_usd": usd, "billable_bytes": bytes, "valid_seconds": QUOTE_TTL.as_secs(), "held_days": held, "bought": prior }),
        ),
    )
}

#[derive(Deserialize)]
struct Submit {
    quote: String,
    confirm_usd: Option<f64>,
    /// The expired (or no longer listed) jobs bought here that this buys
    /// again, by ID: `["OPRA-…"]`. Days the store holds are never bought
    /// again.
    #[serde(default)]
    again: Option<Value>,
}

/// The job IDs `again` names; a blanket `true` is refused.
fn again_ids(v: Option<&Value>) -> Result<HashSet<String>, Reply> {
    let refuse = || {
        refused(
            StatusCode::BAD_REQUEST,
            "\"again\" names the expired jobs this buys again, as [\"OPRA-…\"]; a blanket true is no longer accepted (decision 0019)",
        )
    };
    match v {
        None | Some(Value::Null | Value::Bool(false)) => Ok(HashSet::new()),
        Some(Value::Array(a)) => a
            .iter()
            .map(|x| x.as_str().map(str::to_owned).ok_or_else(refuse))
            .collect(),
        Some(_) => Err(refuse()),
    }
}

async fn submit(AxState(jobs): AxState<Shared>, Json(s): Json<Submit>) -> Reply {
    let again = match again_ids(s.again.as_ref()) {
        Ok(a) => a,
        Err(r) => return r,
    };
    // A quote is used once, whatever happens next.
    let Some(quote) = lock(&jobs.quotes).remove(&s.quote) else {
        return refused(
            StatusCode::CONFLICT,
            "that price has expired or was already used; price the request again",
        );
    };
    if quote.at.elapsed() >= QUOTE_TTL {
        return refused(
            StatusCode::CONFLICT,
            "that price has expired; price the request again",
        );
    }
    let Ok(checked) = quote.request.check() else {
        return refused(StatusCode::BAD_REQUEST, "the request is no longer valid");
    };
    // One purchase at a time, from the last check to the ledger's last word.
    let _turn = jobs.purchase.lock().await;
    if lock(&jobs.ledger).is_none() {
        return refused(StatusCode::SERVICE_UNAVAILABLE, NO_LEDGER);
    }
    if let Err((code, e)) = known_dataset(&jobs, &checked.dataset).await {
        return refused(code, e);
    }
    // Never buy what's had already, and never when that can't be checked.
    let window = checked.window();
    let pieces = match had(
        &jobs,
        &checked.dataset,
        checked.schema,
        &checked.symbols[0],
        checked.start,
        checked.end,
    )
    .await
    {
        Ok(p) => p,
        Err(e) => {
            return refused(
                StatusCode::SERVICE_UNAVAILABLE,
                format!("couldn't check what's had already, so nothing is submitted: {e}"),
            );
        }
    };
    if let Some(r) = refuse_overlaps(&had::overlapping(&pieces, &had::Scope::All, window), &again) {
        return r;
    }
    let now = match cost(
        &jobs,
        &checked.dataset,
        checked.schema,
        &checked.symbols,
        checked.stype_in,
        window,
    )
    .await
    {
        Ok((usd, _)) => usd,
        Err(e) => {
            return refused(
                StatusCode::BAD_GATEWAY,
                format!("could not price it again: {e}"),
            );
        }
    };
    if let Some(r) = check_price(now, quote.cost_usd, s.confirm_usd) {
        return r;
    }
    // Written down before it is sent.
    let request = ledger::fingerprint(
        &checked.dataset,
        checked.schema.as_str(),
        &checked.symbols,
        &checked.start.to_string(),
        &checked.end.to_string(),
    );
    if let Err(e) = write_step(&jobs, &request, "sending", None, now, None) {
        return refused(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("could not write the submissions ledger, so nothing was submitted: {e}"),
        );
    }
    let params = SubmitJobParams::builder()
        .dataset(&checked.dataset)
        .schema(checked.schema)
        .symbols(checked.symbols.clone())
        .stype_in(checked.stype_in)
        .date_time_range(checked.start..checked.end)
        .build();
    match jobs.db.submit(&params).await {
        Ok(job) => {
            if let Err(e) = write_step(
                &jobs,
                &request,
                "submitted",
                Some(job.id.clone()),
                now,
                None,
            ) {
                // The job exists; reconciliation will record it from the job list.
                eprintln!(
                    "databento-jobs: cannot write the ledger after submitting {}: {e}",
                    job.id
                );
            }
            lock(&jobs.follow).insert(job.id.clone());
            let text = format!(
                "submitted {} ({} {} {}, ${now:.2})",
                job.id,
                checked.dataset,
                checked.schema.as_str(),
                checked.symbols.join(",")
            );
            jobs.events
                .push(text, json!({ "job": job.id, "cost_usd": now }).to_string());
            (
                StatusCode::OK,
                Json(json!({ "job": job.id, "cost_usd": now })),
            )
        }
        Err(e) => {
            // Unwritten, the "sending" step still blocks what it overlaps.
            if let Err(w) = write_step(&jobs, &request, "uncertain", None, now, Some(brief(&e))) {
                eprintln!("databento-jobs: cannot write the ledger: {w}");
            }
            jobs.events
                .push(format!("submission failed: {e}"), e.clone());
            refused(
                StatusCode::BAD_GATEWAY,
                format!(
                    "submission failed or its outcome is unknown ({e}); it blocks what it overlaps until it shows on Databento's job list or is resolved (POST /ledger/resolve)"
                ),
            )
        }
    }
}

#[derive(Deserialize)]
struct Resolve {
    /// The request's fingerprint, as in the ledger.
    request: String,
    /// When its unresolved step was written, as in the ledger.
    at: f64,
    /// The job it made, or `null` for none.
    job: Option<String>,
    note: String,
}

/// The operator's word on a submission whose outcome isn't known: the job it
/// made (`submitted`), or none (`not-submitted`). Only a request's latest
/// step, if unresolved, can be resolved.
async fn resolve(AxState(jobs): AxState<Shared>, Json(r): Json<Resolve>) -> Reply {
    if r.note.trim().is_empty() {
        return refused(StatusCode::BAD_REQUEST, "say how you know, in note");
    }
    let _turn = jobs.purchase.lock().await;
    let mut guard = lock(&jobs.ledger);
    let Some(l) = guard.as_mut() else {
        return refused(StatusCode::SERVICE_UNAVAILABLE, NO_LEDGER);
    };
    let Some(e) = l
        .unresolved_step(&r.request)
        .filter(|e| (e.at - r.at).abs() < 1e-6)
        .cloned()
    else {
        return refused(
            StatusCode::NOT_FOUND,
            "no unresolved submission of that request at that time",
        );
    };
    let state = if r.job.is_some() {
        "submitted"
    } else {
        "not-submitted"
    };
    if let Err(err) = l.append(ledger::Entry {
        at: tinytrader_observe::unix_seconds(),
        request: e.request.clone(),
        state: state.into(),
        job: r.job.clone(),
        cost_usd: e.cost_usd,
        note: Some(format!("resolved by the operator: {}", r.note.trim())),
    }) {
        return refused(StatusCode::INTERNAL_SERVER_ERROR, err);
    }
    if let Some(j) = &r.job {
        lock(&jobs.follow).insert(j.clone());
    }
    let text = format!("resolved a submission: {state} ({})", e.request);
    jobs.events.push(text.clone(), text);
    reply_ok(json!({ "request": e.request, "state": state, "job": r.job }))
}

async fn download(AxState(jobs): AxState<Shared>, UrlPath(job): UrlPath<String>) -> Reply {
    let details = jobs.db.job(&job).await;
    match details {
        Ok(j) if j.state != JobState::Done => {
            return refused(StatusCode::CONFLICT, "the job isn't done yet");
        }
        Ok(j) => {
            if let Some(why) = unfileable(&j) {
                return refused(
                    StatusCode::UNPROCESSABLE_ENTITY,
                    format!("not downloaded: {why}"),
                );
            }
        }
        Err(e) => return refused(StatusCode::BAD_GATEWAY, format!("job {job}: {e}")),
    }
    let busy = lock(&jobs.transfers)
        .get(&job)
        .is_some_and(|t| t.error.is_none() && t.state != "filed");
    if busy {
        return refused(StatusCode::CONFLICT, "already downloading");
    }
    start_download(&jobs, job.clone());
    (StatusCode::ACCEPTED, Json(json!({ "job": job })))
}

fn set(jobs: &Jobs, job: &str, f: impl FnOnce(&mut Transfer)) {
    f(lock(&jobs.transfers).entry(job.to_owned()).or_default());
}

fn start_download(jobs: &Shared, job: String) {
    set(jobs, &job, |t| {
        *t = Transfer {
            state: "waiting".into(),
            ..Transfer::default()
        }
    });
    let jobs = jobs.clone();
    tokio::spawn(async move {
        let _turn = jobs.one_at_a_time.acquire().await;
        // Downloads wait while capture is live.
        while jobs.paused_until.load(Ordering::Relaxed) != 0 {
            set(&jobs, &job, |t| t.state = "waiting: capture is live".into());
            tokio::time::sleep(Duration::from_secs(15)).await;
        }
        let result = fetch_and_file(&jobs, &job).await;
        match result {
            Ok(n) => {
                set(&jobs, &job, |t| t.state = "filed".into());
                jobs.events
                    .push(format!("filed {job} in the store ({n} files)"), job.clone());
            }
            Err(e) => {
                set(&jobs, &job, |t| {
                    t.state = "failed".into();
                    t.error = Some(e.clone());
                });
                jobs.events
                    .push(format!("download of {job} failed: {e}"), e);
            }
        }
    });
}

/// Stream every file of a done job into the store, then have it filed.
async fn fetch_and_file(jobs: &Jobs, job: &str) -> Result<usize, String> {
    let files = jobs
        .db
        .files(job)
        .await
        .map_err(|e| format!("list files: {e}"))?;
    // What the store already has of this batch (verified on arrival).
    let incoming: Value = jobs
        .http
        .get(format!("{}/incoming", jobs.store))
        .send()
        .await
        .map_err(|e| format!("store: {e}"))?
        .json()
        .await
        .map_err(|e| format!("store: {e}"))?;
    let have: HashMap<String, u64> = incoming["batches"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|b| b["batch"] == job)
        .flat_map(|b| b["files"].as_array().cloned().unwrap_or_default())
        .filter_map(|f| Some((f["name"].as_str()?.to_owned(), f["bytes"].as_u64()?)))
        .collect();
    let total: u64 = files.iter().map(|f| f.size).sum();
    let needed = total
        - files
            .iter()
            .filter(|f| have.get(&f.filename) == Some(&f.size))
            .map(|f| f.size)
            .sum::<u64>();
    // Files are placed one by one on whichever drive has most room
    // (decision 0007): the pool must hold them all, and a drive the largest.
    let (pool, one) = room(jobs).await?;
    if needed > pool {
        return Err(format!(
            "needs {}, but the store has room for {} across its drives",
            gb(needed),
            gb(pool)
        ));
    }
    let largest = files
        .iter()
        .filter(|f| have.get(&f.filename) != Some(&f.size))
        .map(|f| f.size)
        .max()
        .unwrap_or(0);
    if largest > one {
        return Err(format!(
            "its largest file is {}, but no drive has room for more than {}",
            gb(largest),
            gb(one)
        ));
    }
    set(jobs, job, |t| {
        t.state = "downloading".into();
        t.total_bytes = total;
    });
    let mut done = 0;
    for f in &files {
        set(jobs, job, |t| t.file = f.filename.clone());
        if have.get(&f.filename) != Some(&f.size) {
            // Each file starts only while no capture is live, and only if the
            // capture hours don't begin before it could finish.
            loop {
                let why = match live_capture(jobs).await {
                    Some(who) => Some(format!("waiting: {who} is live")),
                    None => capture_soon(jobs, f.size).map(|at| {
                        format!("waiting: capture starts at {at}, before this file could finish")
                    }),
                };
                let Some(why) = why else { break };
                set(jobs, job, |t| t.state = why.clone());
                tokio::time::sleep(Duration::from_secs(15)).await;
            }
            set(jobs, job, |t| t.state = "downloading".into());
            let url = f
                .urls
                .get("https")
                .ok_or_else(|| format!("{}: no https URL", f.filename))?;
            let mut attempt = 0;
            loop {
                attempt += 1;
                match copy(jobs, job, &f.filename, url, f.size, &f.hash, done).await {
                    Ok(()) => break,
                    Err(e) if attempt < 3 => {
                        eprintln!("databento-jobs: {job}/{}: {e}; retrying", f.filename);
                        tokio::time::sleep(Duration::from_secs(5 * attempt)).await;
                    }
                    Err(e) => return Err(format!("{}: {e}", f.filename)),
                }
            }
        }
        done += f.size;
        set(jobs, job, |t| t.done_bytes = done);
    }
    // Databento's file list is the manifest (its download links aside).
    if !files.iter().any(|f| f.filename == "manifest.json") {
        let manifest = json!({
            "job_id": job,
            "files": files.iter().map(|f| json!({ "filename": f.filename, "size": f.size, "hash": f.hash })).collect::<Vec<_>>(),
        });
        let r = jobs
            .store_write(jobs.http.put(format!(
                "{}/incoming/databento/{job}/manifest.json",
                jobs.store
            )))
            .body(manifest.to_string())
            .send()
            .await
            .map_err(|e| format!("store: {e}"))?;
        if !r.status().is_success() {
            return Err(format!(
                "store refused the manifest: {}",
                r.text().await.unwrap_or_default()
            ));
        }
    }
    set(jobs, job, |t| {
        t.state = "filing".into();
        t.file.clear();
    });
    // A job bought as a list of one option parent's contracts is filed as
    // partial days of that parent (decision 0018).
    let parent = lock(&jobs.details).get(job).and_then(|j| {
        let symbols: Vec<String> = j
            .symbols
            .to_api_string()
            .split(',')
            .map(|s| s.trim().to_owned())
            .collect();
        (j.stype_in == SType::RawSymbol)
            .then(|| missing::parent_of(&symbols))
            .flatten()
    });
    let url = match &parent {
        Some(p) => format!("{}/incorporate/databento/{job}?parent={p}", jobs.store),
        None => format!("{}/incorporate/databento/{job}", jobs.store),
    };
    let r = jobs
        .store_write(jobs.http.post(url))
        .send()
        .await
        .map_err(|e| format!("store: {e}"))?;
    if !r.status().is_success() {
        let body: Value = r.json().await.unwrap_or_default();
        return Err(format!(
            "the store did not file it: {}",
            body["error"].as_str().unwrap_or("unknown error")
        ));
    }
    Ok(files.len())
}

/// What the store can take before its writable volumes reach the critical
/// threshold, in bytes: across them all, and on the roomiest one.
async fn room(jobs: &Jobs) -> Result<(u64, u64), String> {
    let r = jobs
        .http
        .get(format!("{}/catalog", jobs.store))
        .send()
        .await
        .map_err(|e| format!("store: {e}"))?;
    if !r.status().is_success() {
        return Err(format!("the store's catalog: HTTP {}", r.status()));
    }
    let c: Value = r.json().await.map_err(|e| format!("store: {e}"))?;
    let critical = c["thresholds"]["critical"].as_f64().unwrap_or(90.0) / 100.0;
    Ok(c["roots"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|r| r["writable"] == true)
        .filter_map(|r| {
            let (total, free) = (
                r["volume"]["total_bytes"].as_f64()?,
                r["volume"]["available_bytes"].as_f64()?,
            );
            Some((free - total * (1.0 - critical)).max(0.0) as u64)
        })
        .fold((0, 0), |(sum, max), r| (sum + r, max.max(r))))
}

/// The first capture service that is live, if any: its health says anything
/// but idle. One that doesn't answer isn't running, so isn't capturing; one
/// that answers with something unreadable might be, so counts as live.
async fn live_capture(jobs: &Jobs) -> Option<String> {
    for (name, url) in &jobs.yield_to {
        let Ok(r) = jobs
            .http
            .get(format!("{url}/health"))
            .timeout(Duration::from_secs(3))
            .send()
            .await
        else {
            continue;
        };
        let state = r
            .json::<Value>()
            .await
            .ok()
            .and_then(|v| v["state"].as_str().map(str::to_owned));
        if state.as_deref() != Some("idle") {
            return Some(name.clone());
        }
    }
    None
}

/// When the capture hours begin, if before a file of `size` bytes could be
/// downloaded (at a cautious 20 MB/s, and never less than 15 minutes).
fn capture_soon(jobs: &Jobs, size: u64) -> Option<String> {
    let hours = jobs.capture_hours?;
    let now = (tinytrader_observe::unix_seconds() * 1e9) as u64;
    let need = (size / 20_000_000).max(15 * 60) * 1_000_000_000;
    let (start, _) = hours.next(now, &lock(&jobs.calendar))?;
    (start > now && start - now < need).then(|| tinytrader_schedule::describe(start))
}

fn gb(bytes: u64) -> String {
    format!("{:.1} GB", bytes as f64 / 1e9)
}

/// Stream one file from Databento into the store, which checks its size and
/// hash before keeping it.
async fn copy(
    jobs: &Jobs,
    job: &str,
    name: &str,
    url: &str,
    size: u64,
    hash: &str,
    before: u64,
) -> Result<(), String> {
    let resp = jobs
        .http
        .get(url)
        .basic_auth(&jobs.key, None::<&str>)
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if !resp.status().is_success() {
        return Err(format!("Databento answered {}", resp.status()));
    }
    let progress = Arc::new(AtomicU64::new(0));
    let p = progress.clone();
    let body = resp.bytes_stream().map(move |c| {
        if let Ok(b) = &c {
            p.fetch_add(b.len() as u64, Ordering::Relaxed);
        }
        c
    });
    let sha = hash.trim_start_matches("sha256:");
    let put = jobs
        .store_write(jobs.http.put(format!(
            "{}/incoming/databento/{job}/{name}?size={size}&sha256={sha}",
            jobs.store
        )))
        .body(reqwest::Body::wrap_stream(body))
        .send();
    tokio::pin!(put);
    let mut tick = tokio::time::interval(Duration::from_secs(1));
    let r = loop {
        tokio::select! {
            r = &mut put => break r.map_err(|e| format!("store: {e}"))?,
            _ = tick.tick() => set(jobs, job, |t| t.done_bytes = before + progress.load(Ordering::Relaxed)),
        }
    };
    if r.status().is_success() {
        Ok(())
    } else {
        let body: Value = r.json().await.unwrap_or_default();
        Err(format!(
            "the store refused it: {}",
            body["error"].as_str().unwrap_or("unknown error")
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req(symbols: &str, start: &str, end: &str) -> Request {
        Request {
            dataset: "OPRA.PILLAR".into(),
            schema: "cmbp-1".into(),
            symbols: symbols.into(),
            stype_in: "parent".into(),
            start: start.into(),
            end: end.into(),
        }
    }

    #[test]
    fn the_last_day_is_inclusive() {
        let c = req("SPX.OPT", "2025-10-01", "2025-10-01").check().unwrap();
        assert_eq!(
            (c.start.to_string(), c.end.to_string()),
            ("2025-10-01".into(), "2025-10-02".into())
        );
    }

    #[test]
    fn refuses_requests_without_symbols_or_backwards() {
        assert!(req(" , ", "2025-10-01", "2025-10-01").check().is_err());
        assert!(req("SPX.OPT", "2025-10-02", "2025-10-01").check().is_err());
        assert!(
            req("SPY.OPT,QQQ.OPT", "2025-10-01", "2025-10-01")
                .check()
                .is_err()
        );
        assert!(
            Request {
                schema: "nope".into(),
                ..req("SPX.OPT", "2025-10-01", "2025-10-01")
            }
            .check()
            .is_err()
        );
    }
}

#[cfg(test)]
mod paused_tests {
    #[test]
    fn only_an_exact_zero_is_free() {
        assert!(super::paused(0.0).is_none());
        assert!(super::paused(-0.0).is_none());
        for c in [0.01, 1e-12, -1.0, f64::NAN, f64::INFINITY] {
            assert!(super::paused(c).is_some(), "{c} must be refused");
        }
    }
}

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
//!   already: it's refused if the store holds any of its days (or can't say),
//!   or if it overlaps a job on Databento's list (queued, processing, or done
//!   and waiting to download, wherever it was bought), always; or a purchase
//!   in the ledger no longer on that list, unless `again` is true (an expired
//!   job never filed, say). A submission is never retried; if its outcome is uncertain the
//!   jobs list says what happened.
//! - `POST /price-missing` with `{ dataset, schema, symbol, start, end, rule? }`
//!   for an option parent (`SPY.OPT`): for each day, the contracts `rule`
//!   wants (from the store's definitions of that day) less those the store
//!   holds and those in jobs on Databento's list, priced as one
//!   contract-list request per day (decision 0018). `POST /submit-missing`
//!   with `{ quote, confirm_usd }` plans it again and submits it only if
//!   every day and the price are unchanged.
//! - `POST /download/{job}`: streams the job's files into the store, each
//!   checked against Databento's size and SHA-256, then has the store file
//!   the batch. Jobs submitted here download by themselves once done.
//!   Downloading an already purchased job never charges again.
//!
//! The API key comes from `DATABENTO_API_KEY` or `DATABENTO_API_KEY_FILE`.
//! The store's write token, when the store requires one, from
//! `JOBS_STORE_TOKEN` or `JOBS_STORE_TOKEN_FILE`: sent, as a bearer token, on
//! the uploads and filing only, never to Databento.

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
use databento::historical::batch::{BatchJob, JobState, ListJobsParams, SubmitJobParams};
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
        Ok(Checked {
            dataset: self.dataset.trim().to_owned(),
            schema: self
                .schema
                .parse()
                .map_err(|_| format!("unknown schema {}", self.schema))?,
            stype_in: self
                .stype_in
                .parse()
                .map_err(|_| format!("unknown symbol type {}", self.stype_in))?,
            symbols,
            start,
            end: last.next_day().ok_or("bad last day")?,
        })
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
    db: tokio::sync::Mutex<HistoricalClient>,
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
    let jobs = Arc::new(Jobs {
        db: tokio::sync::Mutex::new(db),
        key,
        http: reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .build()
            .expect("http client"),
        store: args.store.trim_end_matches('/').to_owned(),
        store_token,
        quotes: Mutex::default(),
        missing_quotes: Mutex::default(),
        transfers: Mutex::default(),
        follow: Mutex::default(),
        ledger: Mutex::new(match &args.ledger {
            Some(path) => match ledger::Ledger::open(path) {
                Ok(l) => Some(l),
                Err(e) => {
                    eprintln!("databento-jobs: cannot read the submissions ledger: {e}");
                    std::process::exit(2);
                }
            },
            None => None,
        }),
        details: Mutex::default(),
        delivered: Mutex::default(),
        listed: Mutex::new(json!({ "jobs": [], "updated_at": null })),
        one_at_a_time: tokio::sync::Semaphore::new(1),
        events: Events::default(),
        next_quote: AtomicU64::new(1),
        paused_until: AtomicU64::new(0),
        yield_to: args
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
            .collect(),
        capture_hours: args.capture_hours,
        calendar: Mutex::new(tinytrader_schedule::Calendar::default()),
    });
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

/// Read the jobs list and the store's holdings, and start downloads of
/// followed jobs that are done.
/// A job's fingerprint, as its submission was written down.
fn job_fingerprint(j: &BatchJob) -> String {
    let symbols: Vec<String> = match &j.symbols {
        databento::Symbols::All => vec!["ALL_SYMBOLS".into()],
        databento::Symbols::Ids(ids) => ids.iter().map(u32::to_string).collect(),
        databento::Symbols::Symbols(s) => s.clone(),
    };
    ledger::fingerprint(
        &j.dataset,
        j.schema.as_str(),
        &symbols,
        &j.start.date().to_string(),
        &j.end.date().to_string(),
    )
}

/// Resolve the ledger's unresolved submissions against the job list, and
/// follow (download when done) every job submitted here.
fn reconcile(jobs: &Shared, out: &[BatchJob]) {
    let mut guard = lock(&jobs.ledger);
    let Some(l) = guard.as_mut() else { return };
    let now = tinytrader_observe::unix_seconds();
    for e in l.unresolved() {
        let found = out.iter().find(|j| {
            job_fingerprint(j) == e.request && j.ts_received.unix_timestamp() as f64 >= e.at - 120.0
        });
        let step = match found {
            Some(j) => Some(ledger::Entry {
                at: now,
                request: e.request.clone(),
                state: "submitted".into(),
                job: Some(j.id.clone()),
                cost_usd: e.cost_usd,
                note: Some("found in Databento's job list".into()),
            }),
            None if now - e.at > ledger::SETTLE_SECONDS => Some(ledger::Entry {
                at: now,
                request: e.request.clone(),
                state: "not-submitted".into(),
                job: None,
                cost_usd: None,
                note: Some("no such job appeared in Databento's job list".into()),
            }),
            None => None,
        };
        if let Some(step) = step {
            let text = match &step.job {
                Some(id) => format!("reconciled a submission: {id}"),
                None => format!("reconciled a submission: not made ({})", e.request),
            };
            match l.append(step) {
                Ok(()) => jobs.events.push(text.clone(), text),
                Err(err) => eprintln!("databento-jobs: cannot write the ledger: {err}"),
            }
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

async fn refresh(jobs: &Shared) -> Result<Refreshed, String> {
    let short = {
        let mut db = jobs.db.lock().await;
        db.batch()
            .list_jobs(&ListJobsParams::default())
            .await
            .map_err(|e| format!("list jobs: {e}"))?
    };
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
    let mut out = Vec::new();
    // A finished job's details never change; others are read again each
    // round. When Databento can't answer for one, its last-known details
    // stand in, so one slow call doesn't blank the list.
    let (mut stale, mut unread) = (Vec::new(), Vec::new());
    for s in &short {
        let known = lock(&jobs.details).get(&s.id).cloned();
        let job = match known {
            Some(j) if j.state == JobState::Done => j,
            known => {
                let fetched = {
                    let mut db = jobs.db.lock().await;
                    db.batch().get_job_details(&s.id).await
                };
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
        if job.state == JobState::Done
            && held[&job.id].is_null()
            && !lock(&jobs.delivered).contains_key(&job.id)
        {
            let files = jobs.db.lock().await.batch().list_files(&job.id).await;
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
    // created, or, once it would have appeared, record it as never made.
    reconcile(jobs, &out);

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
    let mut db = jobs.db.lock().await;
    match db.metadata().list_datasets(None).await {
        Ok(d) => (StatusCode::OK, Json(json!({ "datasets": d }))),
        Err(e) => refused(StatusCode::BAD_GATEWAY, e.to_string()),
    }
}

#[derive(Deserialize)]
struct DatasetQuery {
    dataset: String,
}

async fn schemas(AxState(jobs): AxState<Shared>, Query(q): Query<DatasetQuery>) -> Reply {
    let mut db = jobs.db.lock().await;
    match db.metadata().list_schemas(&q.dataset).await {
        Ok(s) => (
            StatusCode::OK,
            Json(json!({ "schemas": s.iter().map(Schema::as_str).collect::<Vec<_>>() })),
        ),
        Err(e) => refused(StatusCode::BAD_GATEWAY, e.to_string()),
    }
}

/// The cost of a checked request, in US dollars, and its billable size.
async fn cost(jobs: &Jobs, c: &Checked) -> Result<(f64, u64), String> {
    let params = GetCostParams::builder()
        .dataset(&c.dataset)
        .schema(c.schema)
        .symbols(c.symbols.clone())
        .stype_in(c.stype_in)
        .date_time_range(c.start..c.end)
        .build();
    let mut db = jobs.db.lock().await;
    let usd = db
        .metadata()
        .get_cost(&params)
        .await
        .map_err(|e| e.to_string())?;
    let bytes = db
        .metadata()
        .get_billable_size(&params)
        .await
        .map_err(|e| e.to_string())?;
    Ok((usd, bytes))
}

/// Before pricing: refuse a request outside what Databento has for the
/// schema, and warn about requested days it marks degraded, pending or
/// missing.
async fn availability(jobs: &Jobs, c: &Checked) -> Result<Vec<String>, String> {
    use databento::historical::metadata::{DatasetCondition, GetDatasetConditionParams};
    let mut db = jobs.db.lock().await;
    let mut warnings = Vec::new();
    match db.metadata().get_dataset_range(&c.dataset).await {
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
    match db.metadata().get_dataset_condition(&params).await {
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

/// The request's days, first to last, as `YYYY-MM-DD`.
fn days(c: &Checked) -> Vec<String> {
    let mut out = Vec::new();
    let mut d = c.start;
    while d < c.end {
        out.push(d.to_string());
        let Some(n) = d.next_day() else { break };
        d = n;
    }
    out
}

/// The requested days the store's catalog holds for this dataset, symbol and
/// schema, on any volume.
fn held_days(catalog: &Value, c: &Checked) -> Vec<String> {
    let want: HashSet<String> = days(c).into_iter().collect();
    let symbol = c.symbols.first().map_or("", String::as_str);
    let mut held: Vec<String> = catalog["roots"]
        .as_array()
        .into_iter()
        .flatten()
        .flat_map(|r| r["datasets"].as_array().into_iter().flatten())
        .filter(|d| d["id"] == c.dataset.as_str() && d["symbol"] == symbol)
        .filter_map(|d| d["schemas"][c.schema.as_str()]["days"].as_object())
        .flat_map(|days| days.keys().filter(|k| want.contains(*k)).cloned())
        .collect::<HashSet<_>>()
        .into_iter()
        .collect();
    held.sort();
    held
}

/// Earlier purchases this request overlaps, each as a line saying which:
/// jobs on Databento's list that aren't expired (queued, processing, or done
/// and waiting to be downloaded, wherever they were bought), and purchases in
/// the ledger that aren't on that list any more (`.1`: only these may be
/// bought `again`, as they can no longer be downloaded).
fn bought(purchases: &[ledger::Entry], listed: &Value, c: &Checked) -> (Vec<String>, Vec<String>) {
    let (from, to) = (c.start.to_string(), c.end.to_string());
    let symbol = c.symbols.first().map_or("", String::as_str);
    let on_list: HashSet<&str> = listed["jobs"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|j| j["state"] != "expired")
        .filter_map(|j| j["id"].as_str())
        .collect();
    let mut gone = Vec::new();
    for e in purchases {
        if e.job.as_deref().is_some_and(|j| on_list.contains(j)) {
            continue; // judged below, from the list
        }
        // dataset|schema|symbols|start|end (end exclusive), as `ledger::fingerprint`.
        let f: Vec<&str> = e.request.split('|').collect();
        if let [d, s, sym, a, b] = f[..]
            && d == c.dataset
            && s == c.schema.as_str()
            && sym == symbol
            && a < to.as_str()
            && from.as_str() < b
        {
            gone.push(format!(
                "{} (bought here, {a} to before {b}; no longer on Databento's list)",
                e.job.as_deref().unwrap_or("a job")
            ));
        }
    }
    let (lo, hi) = (
        c.start.midnight().assume_utc().unix_timestamp(),
        c.end.midnight().assume_utc().unix_timestamp(),
    );
    let mut live = Vec::new();
    for j in listed["jobs"].as_array().into_iter().flatten() {
        if j["state"] == "expired" {
            continue;
        }
        if j["dataset"] == c.dataset.as_str()
            && j["schema"] == c.schema.as_str()
            && j["symbols"] == symbol
            && j["start"].as_i64().is_some_and(|s| s < hi)
            && j["end"].as_i64().is_some_and(|e| lo < e)
        {
            live.push(format!(
                "{} ({} on Databento; download it rather than buy it again)",
                j["id"].as_str().unwrap_or_default(),
                j["state"].as_str().unwrap_or("listed")
            ));
        }
    }
    (live, gone)
}

/// The store's catalog: which days each volume holds.
async fn catalog(jobs: &Jobs) -> Result<Value, String> {
    let r = jobs
        .http
        .get(format!("{}/catalog", jobs.store))
        .timeout(Duration::from_secs(30))
        .send()
        .await
        .map_err(|e| format!("the store's catalog: {e}"))?;
    if !r.status().is_success() {
        return Err(format!("the store's catalog: HTTP {}", r.status()));
    }
    r.json::<Value>()
        .await
        .map_err(|e| format!("the store's catalog: {e}"))
}

/// Databento's jobs that aren't expired, as rows (see `row`), read now.
async fn live_jobs(jobs: &Jobs) -> Result<Vec<Value>, String> {
    let short = {
        let mut db = jobs.db.lock().await;
        db.batch()
            .list_jobs(&ListJobsParams::default())
            .await
            .map_err(|e| format!("Databento's job list: {e}"))?
    };
    // Each job's request (dataset, schema, symbols, days) never changes:
    // from the details already read, else read now. A job whose request
    // can't be read fails the check, so nothing is bought blind.
    let mut rows = Vec::new();
    for sj in short.iter().filter(|sj| sj.state != JobState::Expired) {
        let known = lock(&jobs.details).get(&sj.id).cloned();
        let job = match known {
            Some(j) => j,
            None => {
                let mut db = jobs.db.lock().await;
                db.batch()
                    .get_job_details(&sj.id)
                    .await
                    .map_err(|e| format!("Databento's job {}: {e}", sj.id))?
            }
        };
        // Its state as listed now (the details may be from earlier).
        rows.push(row(
            &BatchJob {
                state: sj.state,
                ..job
            },
            &Value::Null,
            None,
        ));
    }
    Ok(rows)
}

/// What's had already of a request: the days the store holds, and earlier
/// purchases it overlaps (see `bought`), from the store's catalog and
/// Databento's job list as they are now.
async fn had(
    jobs: &Jobs,
    c: &Checked,
) -> Result<(Vec<String>, (Vec<String>, Vec<String>)), String> {
    let held = held_days(&catalog(jobs).await?, c);
    let listed = json!({ "jobs": live_jobs(jobs).await? });
    let purchases: Vec<ledger::Entry> = lock(&jobs.ledger)
        .as_ref()
        .map(|l| l.purchases().into_iter().cloned().collect())
        .unwrap_or_default();
    Ok((held, bought(&purchases, &listed, c)))
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
/// parent's definitions for, the contracts wanted less those held and those
/// pending on Databento, priced per day. Returns the days to buy and notes
/// (days with nothing to buy, days without definitions).
async fn plan(jobs: &Jobs, r: &MissingRequest) -> Result<(Vec<missing::Day>, Vec<String>), String> {
    let fmt = time::macros::format_description!("[year]-[month]-[day]");
    let parse = |s: &str| {
        time::Date::parse(s.trim(), fmt).map_err(|_| format!("{s:?} is not a date (YYYY-MM-DD)"))
    };
    let (start, last) = (parse(&r.start)?, parse(&r.end)?);
    if last < start {
        return Err("the last day is before the first".into());
    }
    if (last - start).whole_days() > 366 {
        return Err("at most a year at a time".into());
    }
    if !r.symbol.ends_with(".OPT") {
        return Err("only option parents (ROOT.OPT) are bought by contract".into());
    }
    let schema: Schema = r
        .schema
        .parse()
        .map_err(|_| format!("unknown schema {}", r.schema))?;
    let get = |url: String| async move {
        let resp = jobs
            .http
            .get(&url)
            .timeout(Duration::from_secs(120))
            .send()
            .await
            .map_err(|e| format!("store: {e}"))?;
        if !resp.status().is_success() {
            return Err(format!("store: HTTP {} for {url}", resp.status()));
        }
        Ok::<_, String>(resp)
    };
    // What the store holds of each day, and where its definitions are.
    let coverage: Value = get(format!(
        "{}/coverage?dataset={}&symbol={}&schema={}&start={start}&end={last}",
        jobs.store, r.dataset, r.symbol, r.schema
    ))
    .await?
    .json()
    .await
    .map_err(|e| format!("store: {e}"))?;
    let held: HashMap<String, Value> = coverage["days"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|d| Some((d["day"].as_str()?.to_owned(), d.clone())))
        .collect();
    let listing: Value = get(format!(
        "{}/files?pattern=*/{}/{}/definition/*",
        jobs.store, r.dataset, r.symbol
    ))
    .await?
    .json()
    .await
    .map_err(|e| format!("store: {e}"))?;
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
    let live: Vec<Value> = live_jobs(jobs)
        .await?
        .into_iter()
        .filter(|j| j["dataset"] == r.dataset.as_str() && j["schema"] == schema.as_str())
        .collect();

    let mut days = Vec::new();
    let mut notes = Vec::new();
    let mut d = start;
    while d <= last {
        let key = d.to_string();
        let next = d.next_day().ok_or("bad day")?;
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
            .map_err(|e| format!("store: {e}"))?;
        let wanted = missing::wanted(&missing::contracts(&bytes)?, d, &r.rule);
        let (lo, hi) = (
            d.midnight().assume_utc().unix_timestamp(),
            next.midnight().assume_utc().unix_timestamp(),
        );
        let pending: Vec<Option<std::collections::BTreeSet<String>>> = live
            .iter()
            .filter(|j| {
                j["start"].as_i64().is_some_and(|s| s < hi)
                    && j["end"].as_i64().is_some_and(|e| lo < e)
            })
            .map(|j| {
                let syms = j["symbols"].as_str().unwrap_or_default();
                (syms != r.symbol).then(|| syms.split(',').map(|s| s.trim().to_owned()).collect())
            })
            .collect();
        let (rest, skipped_held, skipped_pending) =
            missing::to_buy(&wanted, held.get(&key), &pending);
        if rest.is_empty() {
            notes.push(format!(
                "{key}: nothing to buy ({} wanted, {skipped_held} held, {skipped_pending} pending)",
                wanted.len()
            ));
            d = next;
            continue;
        }
        let rest: Vec<String> = rest.into_iter().collect();
        for chunk in rest.chunks(missing::MAX_SYMBOLS) {
            let c = Checked {
                dataset: r.dataset.clone(),
                schema,
                symbols: chunk.to_vec(),
                stype_in: SType::RawSymbol,
                start: d,
                end: next,
            };
            let (usd, bytes) = cost(jobs, &c).await?;
            days.push(missing::Day {
                day: d,
                symbols: chunk.to_vec(),
                cost_usd: usd,
                bytes,
                skipped_held,
                skipped_pending,
            });
        }
        d = next;
    }
    Ok((days, notes))
}

async fn price_missing(AxState(jobs): AxState<Shared>, Json(r): Json<MissingRequest>) -> Reply {
    let (days, notes) = match plan(&jobs, &r).await {
        Ok(p) => p,
        Err(e) => return refused(StatusCode::BAD_REQUEST, e),
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
    // Plan again: what's held, pending and priced may have changed.
    let (days, _) = match plan(&jobs, &quote.request).await {
        Ok(p) => p,
        Err(e) => {
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
            .all(|(a, b)| a.day == b.day && a.symbols == b.symbols)
        && (now - quote.cost_usd).abs() < 1e-9;
    if !same {
        return refused(
            StatusCode::CONFLICT,
            format!(
                "what's missing or its price changed (now ${now:.2} for {} requests); price it again",
                days.len()
            ),
        );
    }
    if let Some(r) = paused(now) {
        return r;
    }
    if now > 0.0 && s.confirm_usd.is_none_or(|c| (c - now).abs() > 1e-9) {
        return refused(
            StatusCode::PRECONDITION_REQUIRED,
            format!("this costs ${now:.2}; confirm that amount to submit it"),
        );
    }
    let r = &quote.request;
    let Ok(schema) = r.schema.parse::<Schema>() else {
        return refused(StatusCode::BAD_REQUEST, "unknown schema");
    };
    let mut submitted = Vec::new();
    for d in &days {
        let next = d.day.next_day().unwrap_or(d.day);
        let request = ledger::fingerprint(
            &r.dataset,
            schema.as_str(),
            &d.symbols,
            &d.day.to_string(),
            &next.to_string(),
        );
        {
            let mut guard = lock(&jobs.ledger);
            let Some(l) = guard.as_mut() else {
                return refused(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "no submissions ledger is configured (JOBS_LEDGER), so nothing is submitted",
                );
            };
            if l.blocking(&request).is_some() {
                return refused(StatusCode::CONFLICT, json!({ "error": format!("{}: an earlier submission of it isn't reconciled yet", d.day), "submitted": submitted }).to_string());
            }
            if let Err(e) = l.append(ledger::Entry {
                at: tinytrader_observe::unix_seconds(),
                request: request.clone(),
                state: "sending".into(),
                job: None,
                cost_usd: Some(d.cost_usd),
                note: None,
            }) {
                return refused(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!(
                        "could not write the submissions ledger, so {} wasn't submitted: {e}",
                        d.day
                    ),
                );
            }
        }
        let params = SubmitJobParams::builder()
            .dataset(&r.dataset)
            .schema(schema)
            .symbols(d.symbols.clone())
            .stype_in(SType::RawSymbol)
            .date_time_range(d.day..next)
            .build();
        let result = {
            let mut db = jobs.db.lock().await;
            db.batch().submit_job(&params).await
        };
        let (state, job, note) = match &result {
            Ok(job) => ("submitted", Some(job.id.clone()), None),
            Err(e) => ("uncertain", None, Some(brief(&e.to_string()))),
        };
        if let Some(l) = lock(&jobs.ledger).as_mut()
            && let Err(e) = l.append(ledger::Entry {
                at: tinytrader_observe::unix_seconds(),
                request,
                state: state.into(),
                job: job.clone(),
                cost_usd: Some(d.cost_usd),
                note,
            })
        {
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
    let mut warnings = match availability(&jobs, &checked).await {
        Ok(w) => w,
        Err(e) => return refused(StatusCode::BAD_REQUEST, e),
    };
    // What's had already; submitting refuses it, so say so now.
    let (held, prior) = match had(&jobs, &checked).await {
        Ok((h, (live, gone))) => (h, [live, gone].concat()),
        Err(e) => {
            warnings.push(format!(
                "couldn't check what's held already ({e}); submitting will refuse until it can"
            ));
            (Vec::new(), Vec::new())
        }
    };
    if !held.is_empty() {
        warnings.push(format!(
            "the store already holds {} of these days ({}); request only the others (for an option parent, /price-missing buys only the contracts not held)",
            held.len(),
            held.join(", ")
        ));
    }
    if !prior.is_empty() {
        warnings.push(format!("overlaps earlier purchases: {}", prior.join("; ")));
    }
    let (usd, bytes) = match cost(&jobs, &checked).await {
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
    /// Buy again what overlaps an earlier purchase (one never filed, such as
    /// an expired job). Days the store holds are never bought again.
    #[serde(default)]
    again: bool,
}

async fn submit(AxState(jobs): AxState<Shared>, Json(s): Json<Submit>) -> Reply {
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
    // Never buy what's had already, and never when that can't be checked.
    match had(&jobs, &checked).await {
        Err(e) => {
            return refused(
                StatusCode::SERVICE_UNAVAILABLE,
                format!("couldn't check what's held already, so nothing is submitted: {e}"),
            );
        }
        Ok((held, _)) if !held.is_empty() => {
            return refused(
                StatusCode::CONFLICT,
                format!(
                    "the store already holds {} of these days ({}); request only the others (for an option parent, /price-missing buys only the contracts not held)",
                    held.len(),
                    held.join(", ")
                ),
            );
        }
        Ok((_, (live, _))) if !live.is_empty() => {
            return refused(
                StatusCode::CONFLICT,
                format!(
                    "already bought and on Databento's servers: {}",
                    live.join("; ")
                ),
            );
        }
        Ok((_, (_, gone))) if !gone.is_empty() && !s.again => {
            return refused(
                StatusCode::CONFLICT,
                format!(
                    "this overlaps earlier purchases that are no longer on Databento's list: {}. If they were never filed, submit with \"again\": true to buy them again",
                    gone.join("; ")
                ),
            );
        }
        Ok(_) => {}
    }
    let (now, _) = match cost(&jobs, &checked).await {
        Ok(c) => c,
        Err(e) => {
            return refused(
                StatusCode::BAD_GATEWAY,
                format!("could not price it again: {e}"),
            );
        }
    };
    if (now - quote.cost_usd).abs() > 1e-9 {
        return refused(
            StatusCode::CONFLICT,
            format!(
                "the price changed from ${:.2} to ${now:.2}; price the request again",
                quote.cost_usd
            ),
        );
    }
    if let Some(r) = paused(now) {
        return r;
    }
    if now > 0.0 && s.confirm_usd.is_none_or(|c| (c - now).abs() > 1e-9) {
        return refused(
            StatusCode::PRECONDITION_REQUIRED,
            format!("this request costs ${now:.2}; confirm that amount to submit it"),
        );
    }
    // Written down before it is sent; refused if the same request may
    // already have gone through and isn't reconciled yet.
    let request = ledger::fingerprint(
        &checked.dataset,
        checked.schema.as_str(),
        &checked.symbols,
        &checked.start.to_string(),
        &checked.end.to_string(),
    );
    {
        let mut guard = lock(&jobs.ledger);
        let Some(l) = guard.as_mut() else {
            return refused(
                StatusCode::SERVICE_UNAVAILABLE,
                "no submissions ledger is configured (JOBS_LEDGER), so nothing is submitted",
            );
        };
        if let Some(b) = l.blocking(&request) {
            return refused(
                StatusCode::CONFLICT,
                format!(
                    "this exact request was submitted at {} and its outcome isn't known yet; it is being checked against Databento's job list (up to 10 minutes), so it isn't submitted again",
                    tinytrader_schedule::describe((b.at * 1e9) as u64)
                ),
            );
        }
        if let Err(e) = l.append(ledger::Entry {
            at: tinytrader_observe::unix_seconds(),
            request: request.clone(),
            state: "sending".into(),
            job: None,
            cost_usd: Some(now),
            note: None,
        }) {
            return refused(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("could not write the submissions ledger, so nothing was submitted: {e}"),
            );
        }
    }
    let params = SubmitJobParams::builder()
        .dataset(&checked.dataset)
        .schema(checked.schema)
        .symbols(checked.symbols.clone())
        .stype_in(checked.stype_in)
        .date_time_range(checked.start..checked.end)
        .build();
    let submitted = {
        let mut db = jobs.db.lock().await;
        db.batch().submit_job(&params).await
    };
    match submitted {
        Ok(job) => {
            if let Some(l) = lock(&jobs.ledger).as_mut()
                && let Err(e) = l.append(ledger::Entry {
                    at: tinytrader_observe::unix_seconds(),
                    request: request.clone(),
                    state: "submitted".into(),
                    job: Some(job.id.clone()),
                    cost_usd: Some(now),
                    note: None,
                })
            {
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
            if let Some(l) = lock(&jobs.ledger).as_mut() {
                // Unwritten, the "sending" step still blocks a resubmission.
                if let Err(w) = l.append(ledger::Entry {
                    at: tinytrader_observe::unix_seconds(),
                    request: request.clone(),
                    state: "uncertain".into(),
                    job: None,
                    cost_usd: Some(now),
                    note: Some(brief(&e.to_string())),
                }) {
                    eprintln!("databento-jobs: cannot write the ledger: {w}");
                }
            }
            jobs.events
                .push(format!("submission failed: {e}"), e.to_string());
            refused(
                StatusCode::BAD_GATEWAY,
                format!(
                    "submission failed or its outcome is unknown ({e}); check the jobs list before submitting again"
                ),
            )
        }
    }
}

async fn download(AxState(jobs): AxState<Shared>, UrlPath(job): UrlPath<String>) -> Reply {
    let details = {
        let mut db = jobs.db.lock().await;
        db.batch().get_job_details(&job).await
    };
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
    let files = {
        let mut db = jobs.db.lock().await;
        db.batch()
            .list_files(job)
            .await
            .map_err(|e| format!("list files: {e}"))?
    };
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
    let c: Value = jobs
        .http
        .get(format!("{}/catalog", jobs.store))
        .send()
        .await
        .map_err(|e| format!("store: {e}"))?
        .json()
        .await
        .map_err(|e| format!("store: {e}"))?;
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

    #[test]
    fn finds_the_days_the_store_holds_for_the_same_dataset_symbol_and_schema() {
        let catalog = json!({ "roots": [
            { "datasets": [
                { "id": "OPRA.PILLAR", "symbol": "SPY.OPT", "schemas": { "cmbp-1": { "days": { "2025-10-01": 1, "2025-10-03": 1 } } } },
                { "id": "OPRA.PILLAR", "symbol": "QQQ.OPT", "schemas": { "cmbp-1": { "days": { "2025-10-02": 1 } } } },
            ] },
            { "datasets": [
                { "id": "OPRA.PILLAR", "symbol": "SPY.OPT", "schemas": { "definition": { "days": { "2025-10-02": 1 } }, "cmbp-1": { "days": { "2025-10-03": 1, "2025-10-09": 1 } } } },
            ] },
        ] });
        let c = req("SPY.OPT", "2025-10-01", "2025-10-05").check().unwrap();
        assert_eq!(held_days(&catalog, &c), vec!["2025-10-01", "2025-10-03"]);
        let none = req("SPY.OPT", "2025-10-04", "2025-10-05").check().unwrap();
        assert!(held_days(&catalog, &none).is_empty());
    }

    #[test]
    fn finds_overlapping_purchases_in_the_ledger_and_on_databentos_list() {
        let entry = |start: &str, end: &str| ledger::Entry {
            at: 1.0,
            request: ledger::fingerprint("OPRA.PILLAR", "cmbp-1", &["SPY.OPT".into()], start, end),
            state: "submitted".into(),
            job: Some(format!("OPRA-{start}")),
            cost_usd: Some(1.0),
            note: None,
        };
        // End exclusive, as submitted: Oct 1-2 and Oct 10.
        let purchases = vec![
            entry("2025-10-01", "2025-10-03"),
            entry("2025-10-10", "2025-10-11"),
        ];
        let at = |d: &str| {
            time::Date::parse(d, time::macros::format_description!("[year]-[month]-[day]"))
                .unwrap()
                .midnight()
                .assume_utc()
                .unix_timestamp()
        };
        let listed = json!({ "jobs": [
            { "id": "OPRA-X", "state": "done", "dataset": "OPRA.PILLAR", "schema": "cmbp-1", "symbols": "SPY.OPT", "start": at("2025-10-05"), "end": at("2025-10-06") },
            { "id": "OPRA-OLD", "state": "expired", "dataset": "OPRA.PILLAR", "schema": "cmbp-1", "symbols": "SPY.OPT", "start": at("2025-10-04"), "end": at("2025-10-05") },
            { "id": "OPRA-Q", "state": "done", "dataset": "OPRA.PILLAR", "schema": "cmbp-1", "symbols": "QQQ.OPT", "start": at("2025-10-04"), "end": at("2025-10-05") },
        ] });
        let ids = |c: &Checked| {
            let (live, gone) = bought(&purchases, &listed, c);
            [live, gone]
                .concat()
                .iter()
                .map(|l| l.split(' ').next().unwrap().to_owned())
                .collect::<Vec<_>>()
        };
        // On Databento's servers (here done, waiting to download): always
        // refused. In the ledger only (its job gone from the list): may be
        // bought again.
        let (live, gone) = bought(
            &purchases,
            &listed,
            &req("SPY.OPT", "2025-10-02", "2025-10-05").check().unwrap(),
        );
        assert_eq!((live.len(), gone.len()), (1, 1));
        assert!(live[0].starts_with("OPRA-X") && gone[0].starts_with("OPRA-2025-10-01"));
        // The day after a purchase ends doesn't overlap it; nor do expired jobs or other symbols.
        assert!(ids(&req("SPY.OPT", "2025-10-03", "2025-10-04").check().unwrap()).is_empty());
        assert_eq!(
            ids(&req("SPY.OPT", "2025-10-10", "2025-10-10").check().unwrap()),
            vec!["OPRA-2025-10-10"]
        );
    }

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

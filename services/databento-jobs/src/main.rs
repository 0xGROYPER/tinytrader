//! Databento batch jobs: price a request, submit it, follow it, and download
//! the finished files into the store.
//!
//! - `GET /jobs`: Databento's jobs (not expired), each with what the store
//!   already holds of it and any download in progress.
//! - `GET /datasets`, `GET /schemas?dataset=`: for the request form.
//! - `POST /price` with `{ dataset, schema, symbols, stype_in, start, end }`
//!   (dates, `end` inclusive): the cost and billable size, and a quote ID.
//! - `POST /submit` with `{ quote, confirm_usd }`: prices the quoted request
//!   again and submits it only if the price is unchanged and, when it costs
//!   anything, `confirm_usd` is exactly that price. A submission is never
//!   retried; if its outcome is uncertain the jobs list says what happened.
//! - `POST /download/{job}`: streams the job's files into the store, each
//!   checked against Databento's size and SHA-256, then has the store file
//!   the batch. Jobs submitted here download by themselves once done.
//!   Downloading an already purchased job never charges again.
//!
//! The API key comes from `DATABENTO_API_KEY` or `DATABENTO_API_KEY_FILE`.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

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
    quotes: Mutex<HashMap<String, Quote>>,
    transfers: Mutex<HashMap<String, Transfer>>,
    /// Jobs submitted here, downloaded once done.
    follow: Mutex<HashSet<String>>,
    /// Full job details, kept once a job is done (they no longer change).
    details: Mutex<HashMap<String, BatchJob>>,
    /// Bytes to download (compressed, as delivered) of done jobs.
    delivered: Mutex<HashMap<String, u64>>,
    listed: Mutex<Value>,
    one_at_a_time: tokio::sync::Semaphore,
    events: Events,
    next_quote: AtomicU64,
}

type Shared = Arc<Jobs>;
type Reply = (StatusCode, Json<Value>);

fn refused(code: StatusCode, why: impl Into<String>) -> Reply {
    (code, Json(json!({ "error": why.into() })))
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
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
        quotes: Mutex::default(),
        transfers: Mutex::default(),
        follow: Mutex::default(),
        details: Mutex::default(),
        delivered: Mutex::default(),
        listed: Mutex::new(json!({ "jobs": [], "updated_at": null })),
        one_at_a_time: tokio::sync::Semaphore::new(1),
        events: Events::default(),
        next_quote: AtomicU64::new(1),
    });
    let listen = listen_address("JOBS_LISTEN", "127.0.0.1:9121");
    let observer = Observer::new("databento-jobs", env!("CARGO_PKG_VERSION"));
    observer.describe(json!({
        "role": "Databento batch jobs: prices and submits requests, and downloads finished jobs into the store.",
        "upstream": [{ "id": "databento", "label": "databento", "sub": "batch API", "kind": "flow", "via": "https" }],
        "streams": { "jobs": "/jobs", "events": "/events" },
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
        .route("/download/{job}", post(download))
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
    loop {
        match refresh(&jobs).await {
            Ok(n) => observer.set_state(State::Ready, &format!("{n} jobs")),
            Err(e) => observer.set_state(State::Degraded, &e),
        }
        tokio::time::sleep(every).await;
    }
}

/// Read the jobs list and the store's holdings, and start downloads of
/// followed jobs that are done.
async fn refresh(jobs: &Shared) -> Result<usize, String> {
    let short = {
        let mut db = jobs.db.lock().await;
        db.batch()
            .list_jobs(&ListJobsParams::default())
            .await
            .map_err(|e| format!("list jobs: {e}"))?
    };
    let mut out = Vec::new();
    for s in &short {
        let known = lock(&jobs.details).get(&s.id).cloned();
        let job = match known {
            Some(j) => j,
            None => {
                let mut db = jobs.db.lock().await;
                match db.batch().get_job_details(&s.id).await {
                    Ok(j) => j,
                    Err(e) => return Err(format!("job {}: {e}", s.id)),
                }
            }
        };
        if job.state == JobState::Done {
            lock(&jobs.details).insert(job.id.clone(), job.clone());
            if !lock(&jobs.delivered).contains_key(&job.id) {
                let files = jobs.db.lock().await.batch().list_files(&job.id).await;
                if let Ok(files) = files {
                    lock(&jobs.delivered)
                        .insert(job.id.clone(), files.iter().map(|f| f.size).sum());
                }
            }
        }
        out.push(job);
    }
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
    let delivered = lock(&jobs.delivered).clone();
    let rows: Vec<Value> = out
        .iter()
        .map(|j| row(j, &held, delivered.get(&j.id).copied()))
        .collect();
    *lock(&jobs.listed) = json!({ "jobs": rows, "store_reachable": !held.is_null(), "updated_at": tinytrader_observe::unix_seconds() });

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
    Ok(out.len())
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

async fn price(AxState(jobs): AxState<Shared>, Json(request): Json<Request>) -> Reply {
    let checked = match request.check() {
        Ok(c) => c,
        Err(e) => return refused(StatusCode::BAD_REQUEST, e),
    };
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
            json!({ "quote": id, "cost_usd": usd, "billable_bytes": bytes, "valid_seconds": QUOTE_TTL.as_secs() }),
        ),
    )
}

#[derive(Deserialize)]
struct Submit {
    quote: String,
    confirm_usd: Option<f64>,
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
    if now > 0.0 && s.confirm_usd.is_none_or(|c| (c - now).abs() > 1e-9) {
        return refused(
            StatusCode::PRECONDITION_REQUIRED,
            format!("this request costs ${now:.2}; confirm that amount to submit it"),
        );
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
    let room = room(jobs).await?;
    if needed > room {
        return Err(format!(
            "needs {}, but the store has room for {} on any one volume",
            gb(needed),
            gb(room)
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
            .http
            .put(format!(
                "{}/incoming/databento/{job}/manifest.json",
                jobs.store
            ))
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
    let r = jobs
        .http
        .post(format!("{}/incorporate/databento/{job}", jobs.store))
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

/// The most the store can take on one writable volume before it reaches its
/// critical threshold, in bytes.
async fn room(jobs: &Jobs) -> Result<u64, String> {
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
        .max()
        .unwrap_or(0))
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
        .http
        .put(format!(
            "{}/incoming/databento/{job}/{name}?size={size}&sha256={sha}",
            jobs.store
        ))
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

/// A credential from `NAME`, or from the file named by `NAME_FILE`.
fn secret(name: &str) -> Result<String, String> {
    if let Ok(path) = std::env::var(format!("{name}_FILE")) {
        return std::fs::read_to_string(&path)
            .map(|s| s.trim().to_owned())
            .map_err(|e| format!("cannot read {name}_FILE {path}: {e}"));
    }
    std::env::var(name).map_err(|_| format!("set {name} or {name}_FILE"))
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
        assert!(req("SPY.OPT,QQQ.OPT", "2025-10-01", "2025-10-01").check().is_err());
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

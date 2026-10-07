//! The tinytrader store (decision 0005): the one process that touches the
//! data disks. Other services reach data through it over HTTP.
//!
//! - **Volumes**: each `--volume NAME=PATH` is a disk the store manages;
//!   `NAME=PATH:ro` marks one read-only. New data goes to the writable volume
//!   with the most free space below the critical threshold.
//! - **Catalog** (`GET /catalog`): what datasets each volume holds, which
//!   days they cover, and how full the volume is (see the catalog module).
//! - **Ingest**: `PUT /incoming/{provider}/{batch}/{file}?size=&sha256=`
//!   uploads one file of a provider batch, verified on arrival;
//!   `GET /incoming` lists batches waiting; `POST /incorporate/{provider}/{batch}`
//!   files a complete batch into the standard layout, with provenance and a
//!   migration ledger (see the ingest module). `GET /batches/{provider}`
//!   says which batches the volumes hold, and how. `POST /adopt/{provider}`
//!   with `{ "path": … }` files a provider download folder already on a
//!   writable volume where it is, on the same terms.
//!
//! It runs natively (under launchd on a Mac), not in a container, so it sees
//! the real disks, and binds to loopback by default.

mod catalog;
mod ingest;
mod volumes;

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tinytrader_observe::lock;

use axum::body::Body;
use axum::extract::{Path as UrlPath, Query, State as AxState};
use axum::http::StatusCode;
use axum::routing::{get, post, put};
use axum::{Json, Router};
use catalog::Thresholds;
use clap::Parser;
use futures_util::StreamExt as _;
use ingest::Volume;
use serde::Deserialize;
use serde_json::{Value, json};
use tinytrader_observe::{Events, Observer, State, listen_address, unix_seconds};

/// The tinytrader data store.
#[derive(Parser, Debug)]
#[command(version, about)]
struct Args {
    /// A volume as NAME=PATH, or NAME=PATH:ro for read-only. Repeat, or
    /// separate with `;`.
    #[arg(long = "volume", env = "STORE_VOLUMES", value_delimiter = ';')]
    volumes: Vec<String>,

    /// Where drives are mounted (such as /Volumes). Drives there carrying an
    /// approval marker are used as they appear; the others are offered for
    /// approval in the console (decision 0007).
    #[arg(long, env = "STORE_DISCOVER")]
    discover: Option<PathBuf>,

    /// How often to rescan the volumes, in seconds.
    #[arg(long, env = "STORE_RESCAN_SECONDS", default_value_t = 300)]
    rescan_seconds: u64,

    /// Volume usage, as a percentage, at which a volume is in warning.
    #[arg(long, default_value_t = 80.0)]
    warning: f64,

    /// Volume usage at which no new data is placed on a volume.
    #[arg(long, default_value_t = 90.0)]
    critical: f64,

    /// Volume usage at which capture stops cleanly.
    #[arg(long, default_value_t = 97.0)]
    floor: f64,
}

struct Store {
    /// The configured volumes, then approved drives as they are found.
    volumes: std::sync::RwLock<Vec<Volume>>,
    discover: Option<PathBuf>,
    limits: Thresholds,
    catalog: Mutex<Value>,
    events: Events,
    rescan: tokio::sync::Notify,
    bytes_in: AtomicU64,
    batches_in: AtomicU64,
    /// One batch is filed at a time.
    filing: tokio::sync::Mutex<()>,
}

type Shared = Arc<Store>;
type Reply = (StatusCode, Json<Value>);

fn reply(code: StatusCode, v: Value) -> Reply {
    (code, Json(v))
}

fn refused(code: StatusCode, why: impl Into<String>) -> Reply {
    reply(code, json!({ "error": why.into() }))
}

#[tokio::main]
async fn main() {
    let args = Args::parse();
    let volumes: Vec<Volume> = args
        .volumes
        .iter()
        .filter_map(|v| {
            let (name, path) = v.split_once('=')?;
            let (path, writable) = match path.trim().strip_suffix(":ro") {
                Some(p) => (p, false),
                None => (path.trim(), true),
            };
            Some(Volume {
                name: name.trim().to_owned(),
                path: PathBuf::from(path),
                writable,
            })
        })
        .collect();
    if volumes.is_empty() && args.discover.is_none() {
        eprintln!("store: give at least one --volume NAME=PATH, or --discover DIR");
        std::process::exit(2);
    }
    let limits = Thresholds {
        warning: args.warning,
        critical: args.critical,
        floor: args.floor,
    };
    let store = Arc::new(Store {
        volumes: std::sync::RwLock::new(volumes),
        discover: args.discover.clone(),
        limits,
        catalog: Mutex::new(json!({ "generated_at": null, "roots": [] })),
        events: Events::default(),
        rescan: tokio::sync::Notify::new(),
        bytes_in: AtomicU64::new(0),
        batches_in: AtomicU64::new(0),
        filing: tokio::sync::Mutex::new(()),
    });
    let listen = listen_address("STORE_LISTEN", "127.0.0.1:9120");
    let observer = Observer::new("store", env!("CARGO_PKG_VERSION"));
    let names = store
        .volumes()
        .iter()
        .map(|v| format!("{}{}", v.name, if v.writable { "" } else { " (read-only)" }))
        .collect::<Vec<_>>()
        .join(", ");
    observer.describe(json!({
        "role": format!("The data store: ingests provider downloads into the standard layout and catalogs {names}."),
        "streams": { "catalog": "/catalog", "events": "/events" },
        "schedule": [
            { "name": "Store", "kind": "always", "when": "always (launchd keeps it running)" },
            { "name": "Catalog rescan", "kind": "interval", "every_seconds": args.rescan_seconds, "when": format!("every {} min, and after each filing", args.rescan_seconds / 60) },
        ],
    }));
    {
        let s = store.clone();
        observer.set_metrics(move |w| {
            w.counter(
                "tinytrader_store_ingested_bytes_total",
                "Bytes received for ingest.",
                &[],
                s.bytes_in.load(Ordering::Relaxed) as f64,
            );
            w.counter(
                "tinytrader_store_batches_total",
                "Batches incorporated.",
                &[],
                s.batches_in.load(Ordering::Relaxed) as f64,
            );
            let c = lock(&s.catalog);
            for root in c["roots"].as_array().into_iter().flatten() {
                let labels = [("volume", root["name"].as_str().unwrap_or("?"))];
                if let Some(u) = root["volume"]["used_fraction"].as_f64() {
                    w.gauge(
                        "tinytrader_store_volume_used_ratio",
                        "Fraction of the volume in use.",
                        &labels,
                        u,
                    );
                }
                if let Some(b) = root["dataset_bytes"].as_f64() {
                    w.gauge(
                        "tinytrader_store_dataset_bytes",
                        "Bytes in recognised datasets.",
                        &labels,
                        b,
                    );
                }
            }
        });
    }

    let app = Router::new()
        .route(
            "/catalog",
            get(|AxState(s): AxState<Shared>| async move { Json(lock(&s.catalog).clone()) }),
        )
        .route("/incoming", get(list_incoming))
        .route("/incoming/{provider}/{batch}/{file}", put(upload))
        .route("/incorporate/{provider}/{batch}", post(incorporate))
        .route("/batches/{provider}", get(batches))
        .route("/adopt/{provider}", post(adopt))
        .route("/volumes/approve", post(approve))
        // Filing and drive approval can't be triggered by a web page.
        .layer(axum::middleware::from_fn(
            tinytrader_observe::guard::same_origin_writes,
        ))
        .with_state(store.clone())
        .merge(observer.router())
        .merge(store.events.router());
    let listener = match tokio::net::TcpListener::bind(&listen).await {
        Ok(l) => l,
        Err(e) => {
            eprintln!("store: cannot listen on {listen}: {e}");
            std::process::exit(2);
        }
    };
    eprintln!("store: {names}; endpoints on http://{listen}");
    tokio::spawn(async move {
        if let Err(e) = axum::serve(listener, app).await {
            eprintln!("store: http server stopped: {e}");
        }
    });

    let every = Duration::from_secs(args.rescan_seconds.max(10));
    loop {
        scan(&store, &observer).await;
        tokio::select! {
            () = tokio::time::sleep(every) => {}
            () = store.rescan.notified() => {}
        }
    }
}

/// Rescan every volume into the catalog.
async fn scan(store: &Shared, observer: &Observer) {
    // Approved drives join as they are found; one that goes missing stays
    // listed (and reported) until it returns.
    let mut candidates = Vec::new();
    if let Some(dir) = store.discover.clone() {
        let found = tokio::task::spawn_blocking(move || volumes::discover(&dir)).await;
        if let Ok(found) = found {
            let mut list = store
                .volumes
                .write()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            for v in found.marked {
                if !list.iter().any(|x| x.path == v.path || x.name == v.name) {
                    eprintln!(
                        "store: using approved drive {} at {}",
                        v.name,
                        v.path.display()
                    );
                    list.push(v);
                }
            }
            // A drive the configuration already names isn't offered again.
            candidates = found
                .candidates
                .into_iter()
                .filter(|c| !list.iter().any(|v| c["path"].as_str() == v.path.to_str()))
                .collect();
        }
    }
    let (volumes, limits) = (store.volumes(), store.limits);
    let scanned = tokio::task::spawn_blocking(move || {
        volumes
            .iter()
            .map(|v| {
                let mut root = catalog::scan_root(&v.name, &v.path, limits);
                root["writable"] = json!(v.writable);
                root
            })
            .collect::<Vec<_>>()
    })
    .await
    .unwrap_or_default();
    let worst = scanned
        .iter()
        .filter_map(|r| r["volume"]["status"].as_str())
        .max_by_key(|s| catalog::severity(s))
        .unwrap_or("ok")
        .to_owned();
    let missing: Vec<&str> = scanned
        .iter()
        .filter(|r| r["present"] != true)
        .filter_map(|r| r["name"].as_str())
        .collect();
    let detail = if missing.is_empty() {
        format!("{} volumes; fullest {worst}", scanned.len())
    } else {
        format!("missing: {}", missing.join(", "))
    };
    let degraded = !missing.is_empty() || catalog::severity(&worst) >= 2;
    *lock(&store.catalog) = json!({
        "generated_at": unix_seconds(),
        "thresholds": { "warning": store.limits.warning, "critical": store.limits.critical, "floor": store.limits.floor },
        "roots": scanned,
        "candidates": candidates,
        "discover": store.discover,
    });
    observer.set_state(
        if degraded {
            State::Degraded
        } else {
            State::Ready
        },
        &detail,
    );
}

impl Store {
    fn volumes(&self) -> Vec<Volume> {
        self.volumes
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// Every writable volume's share of an incoming batch.
    fn parts(&self, provider: &str, batch: &str) -> Vec<ingest::Part> {
        self.volumes()
            .iter()
            .filter(|v| v.writable)
            .map(|v| v.path.join(provider))
            .map(|root| ingest::Part {
                inc: root.join(ingest::INCOMING).join(batch),
                root,
            })
            .filter(|p| p.inc.is_dir())
            .collect()
    }

    /// Where an incoming file goes (decision 0007: each file whole on the
    /// volume with the most room): the volume already holding it, or a part
    /// of it, if any; otherwise the writable volume with the most free space
    /// that stays below the critical threshold with `size` more bytes.
    fn place(
        &self,
        provider: &str,
        batch: &str,
        file: &str,
        size: Option<u64>,
    ) -> Result<PathBuf, String> {
        if let Some(p) = self
            .parts(provider, batch)
            .into_iter()
            .find(|p| p.inc.join(file).exists() || p.inc.join(format!("{file}.partial")).exists())
        {
            return Ok(p.root);
        }
        let need = size.unwrap_or(0);
        self.volumes()
            .iter()
            .filter(|v| v.writable && v.path.is_dir())
            .filter_map(|v| {
                let total = fs4::total_space(&v.path).ok()?;
                let free = fs4::available_space(&v.path).ok()?.checked_sub(need)?;
                let used = 100.0 * (1.0 - free as f64 / total.max(1) as f64);
                (used < self.limits.critical).then(|| (free, v.path.join(provider)))
            })
            .max_by_key(|(free, _)| *free)
            .map(|(_, root)| root)
            .ok_or_else(|| {
                format!(
                    "no writable volume has room for {file}{} below {}% used",
                    size.map_or(String::new(), |s| format!(" ({s} bytes)")),
                    self.limits.critical
                )
            })
    }
}

#[derive(Deserialize)]
struct Expect {
    size: Option<u64>,
    sha256: Option<String>,
}

async fn upload(
    AxState(store): AxState<Shared>,
    UrlPath((provider, batch, file)): UrlPath<(String, String, String)>,
    Query(expect): Query<Expect>,
    body: Body,
) -> Reply {
    let Some(pdir) = ingest::provider_dir(&provider) else {
        return refused(
            StatusCode::NOT_FOUND,
            format!("unknown provider {provider}"),
        );
    };
    if !ingest::safe(&batch) || !ingest::safe(&file) {
        return refused(StatusCode::BAD_REQUEST, "unsafe batch or file name");
    }
    let root = match store.place(pdir, &batch, &file, expect.size) {
        Ok(r) => r,
        Err(e) => return refused(StatusCode::INSUFFICIENT_STORAGE, e),
    };
    let dir = root.join(ingest::INCOMING).join(&batch);
    let mut up = match tokio::task::block_in_place(|| ingest::Upload::start(&dir, &file)) {
        Ok(u) => u,
        Err(e) => return refused(StatusCode::INTERNAL_SERVER_ERROR, e),
    };
    let mut stream = body.into_data_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = match chunk {
            Ok(c) => c,
            Err(e) => return refused(StatusCode::BAD_REQUEST, format!("upload interrupted: {e}")),
        };
        store
            .bytes_in
            .fetch_add(chunk.len() as u64, Ordering::Relaxed);
        if let Err(e) = tokio::task::block_in_place(|| up.write(&chunk)) {
            return refused(StatusCode::INTERNAL_SERVER_ERROR, e);
        }
    }
    match tokio::task::block_in_place(|| up.finish(expect.size, expect.sha256.as_deref())) {
        Ok((size, sha)) => {
            let text = format!("received {pdir}/{batch}/{file} ({size} bytes)");
            store.events.push(
                text,
                json!({ "file": file, "batch": batch, "bytes": size, "sha256": sha }).to_string(),
            );
            reply(
                StatusCode::OK,
                json!({ "file": file, "bytes": size, "sha256": sha }),
            )
        }
        Err(e) => refused(StatusCode::UNPROCESSABLE_ENTITY, e),
    }
}

/// Batches waiting to be incorporated, with their files.
async fn list_incoming(AxState(store): AxState<Shared>) -> Json<Value> {
    let mut out = Vec::new();
    for v in store.volumes().iter().filter(|v| v.writable) {
        let Ok(providers) = std::fs::read_dir(&v.path) else {
            continue;
        };
        for p in providers.flatten() {
            let inc = p.path().join(ingest::INCOMING);
            let Ok(batches) = std::fs::read_dir(&inc) else {
                continue;
            };
            for b in batches.flatten() {
                let files: Vec<Value> = std::fs::read_dir(b.path())
                    .into_iter()
                    .flatten()
                    .flatten()
                    .filter(|f| !f.file_name().to_string_lossy().ends_with(".sha256"))
                    .map(|f| json!({ "name": f.file_name().to_string_lossy(), "bytes": f.metadata().map(|m| m.len()).unwrap_or(0) }))
                    .collect();
                out.push(json!({ "volume": v.name, "provider": p.file_name().to_string_lossy(), "batch": b.file_name().to_string_lossy(), "files": files }));
            }
        }
    }
    Json(json!({ "batches": out }))
}

async fn incorporate(
    AxState(store): AxState<Shared>,
    UrlPath((provider, batch)): UrlPath<(String, String)>,
) -> Reply {
    let Some(pdir) = ingest::provider_dir(&provider) else {
        return refused(
            StatusCode::NOT_FOUND,
            format!("unknown provider {provider}"),
        );
    };
    if !ingest::safe(&batch) {
        return refused(StatusCode::BAD_REQUEST, "unsafe batch name");
    }
    let parts = store.parts(pdir, &batch);
    if parts.is_empty() {
        return refused(StatusCode::NOT_FOUND, format!("no incoming batch {batch}"));
    }
    // The same provider on every other volume, read-only ones included, is
    // checked for the same days.
    let elsewhere: Vec<PathBuf> = store
        .volumes()
        .iter()
        .map(|v| v.path.join(pdir))
        .filter(|p| !parts.iter().any(|x| x.root == *p))
        .collect();
    let _one_at_a_time = store.filing.lock().await;
    let today = today();
    let b = batch.clone();
    let result = tokio::task::spawn_blocking(move || {
        ingest::incorporate_databento(&parts, &elsewhere, &b, &today)
    })
    .await;
    match result {
        Ok(Ok(summary)) => {
            store.batches_in.fetch_add(1, Ordering::Relaxed);
            let n = summary["files"].as_array().map_or(0, Vec::len);
            store.events.push(
                format!("incorporated {pdir}/{batch}: {n} files"),
                summary.to_string(),
            );
            store.rescan.notify_one();
            reply(StatusCode::OK, summary)
        }
        Ok(Err(e)) => {
            store
                .events
                .push(format!("refused {pdir}/{batch}: {e}"), e.clone());
            refused(StatusCode::CONFLICT, e)
        }
        Err(e) => refused(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

/// Every batch the volumes hold for a provider, by batch ID: `filed` (its
/// receipts are in `provenance/`), `incoming` (uploaded, not yet filed) or
/// `folder` (a provider download folder, as at the top of a volume).
async fn batches(AxState(store): AxState<Shared>, UrlPath(provider): UrlPath<String>) -> Reply {
    let Some(pdir) = ingest::provider_dir(&provider) else {
        return refused(
            StatusCode::NOT_FOUND,
            format!("unknown provider {provider}"),
        );
    };
    let volumes = store.volumes();
    let found = tokio::task::spawn_blocking(move || {
        let mut out = serde_json::Map::new();
        let dirs = |p: &std::path::Path| -> Vec<std::fs::DirEntry> {
            std::fs::read_dir(p)
                .into_iter()
                .flatten()
                .flatten()
                .filter(|e| e.path().is_dir())
                .collect()
        };
        for v in &volumes {
            let root = v.path.join(pdir);
            // Raw download folders first, so a filed copy wins.
            let mut parents = vec![v.path.clone(), root.clone()];
            parents.extend(
                dirs(&v.path)
                    .iter()
                    .map(std::fs::DirEntry::path)
                    .filter(|p| *p != root),
            );
            for parent in parents {
                for e in dirs(&parent) {
                    if e.path().join("manifest.json").is_file()
                        && e.path().join("metadata.json").is_file()
                    {
                        let id = e.file_name().to_string_lossy().into_owned();
                        out.entry(id).or_insert(
                            json!({ "volume": v.name, "how": "folder", "path": e.path() }),
                        );
                    }
                }
            }
            for (dir, how) in [
                (root.join(ingest::INCOMING), "incoming"),
                (root.join("provenance"), "filed"),
            ] {
                for e in dirs(&dir) {
                    let id = e.file_name().to_string_lossy().into_owned();
                    out.insert(
                        id,
                        json!({ "volume": v.name, "how": how, "path": e.path() }),
                    );
                }
            }
        }
        Value::Object(out)
    })
    .await
    .unwrap_or_default();
    reply(
        StatusCode::OK,
        json!({ "provider": pdir, "batches": found }),
    )
}

#[derive(Deserialize)]
struct AdoptRequest {
    path: PathBuf,
}

/// File a download folder already on a volume, where it is. It must sit at
/// the top of a writable volume, or one level down (such as a downloader's
/// own folder), and be named for its batch.
async fn adopt(
    AxState(store): AxState<Shared>,
    UrlPath(provider): UrlPath<String>,
    Json(req): Json<AdoptRequest>,
) -> Reply {
    let Some(pdir) = ingest::provider_dir(&provider) else {
        return refused(
            StatusCode::NOT_FOUND,
            format!("unknown provider {provider}"),
        );
    };
    let Ok(dir) = req.path.canonicalize() else {
        return refused(
            StatusCode::NOT_FOUND,
            format!("no folder {}", req.path.display()),
        );
    };
    let all = store.volumes();
    let Some(volume) = all.iter().find(|v| {
        v.path.canonicalize().is_ok_and(|vp| {
            dir.parent() == Some(vp.as_path())
                || dir.parent().and_then(Path::parent) == Some(vp.as_path())
        })
    }) else {
        return refused(
            StatusCode::BAD_REQUEST,
            "the folder must be at the top of a volume, or one level down",
        );
    };
    if !volume.writable {
        return refused(
            StatusCode::FORBIDDEN,
            format!("{} is read-only", volume.name),
        );
    }
    if !dir.join("manifest.json").is_file() || !dir.join("metadata.json").is_file() {
        return refused(
            StatusCode::BAD_REQUEST,
            "not a provider download folder (no manifest.json and metadata.json)",
        );
    }
    let root = volume.path.join(pdir);
    let elsewhere: Vec<PathBuf> = store
        .volumes()
        .iter()
        .map(|v| v.path.join(pdir))
        .filter(|p| *p != root)
        .collect();
    let _one_at_a_time = store.filing.lock().await;
    let today = today();
    let d = dir.clone();
    match tokio::task::spawn_blocking(move || {
        ingest::adopt_databento(&d, &root, &elsewhere, &today)
    })
    .await
    {
        Ok(Ok(summary)) => {
            store.batches_in.fetch_add(1, Ordering::Relaxed);
            let n = summary["files"].as_array().map_or(0, Vec::len);
            store.events.push(
                format!("filed {} in place: {n} files", dir.display()),
                summary.to_string(),
            );
            store.rescan.notify_one();
            reply(StatusCode::OK, summary)
        }
        Ok(Err(e)) => {
            store
                .events
                .push(format!("refused {}: {e}", dir.display()), e.clone());
            refused(StatusCode::CONFLICT, e)
        }
        Err(e) => refused(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

/// Today's UTC date, `YYYY-MM-DD`.
#[derive(Deserialize)]
struct ApproveRequest {
    path: PathBuf,
    name: String,
}

/// Approve a drive found in the mount directory: write its marker and start
/// using it. The console asks the operator first.
async fn approve(AxState(store): AxState<Shared>, Json(req): Json<ApproveRequest>) -> Reply {
    let Some(dir) = store.discover.clone() else {
        return refused(
            StatusCode::NOT_FOUND,
            "this store doesn't look for new drives (no --discover)",
        );
    };
    let taken: Vec<String> = store.volumes().into_iter().map(|v| v.name).collect();
    let (path, name) = (req.path.clone(), req.name.trim().to_owned());
    let result =
        tokio::task::spawn_blocking(move || volumes::approve(&dir, &path, &name, &taken)).await;
    match result {
        Ok(Ok(v)) => {
            store.events.push(
                format!("approved drive {} at {}", v.name, v.path.display()),
                json!({ "name": v.name, "path": v.path }).to_string(),
            );
            store
                .volumes
                .write()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(v.clone());
            store.rescan.notify_one();
            reply(StatusCode::OK, json!({ "name": v.name, "path": v.path }))
        }
        Ok(Err(e)) => refused(StatusCode::BAD_REQUEST, e),
        Err(e) => refused(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

fn today() -> String {
    let days = (unix_seconds() / 86_400.0) as i64;
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    format!("{:04}-{m:02}-{d:02}", yoe + era * 400 + i64::from(m <= 2))
}

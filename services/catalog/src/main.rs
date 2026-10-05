//! Read-only catalog of data roots: what datasets they hold, which days each
//! covers, how much space they use, and how full each volume is.
//!
//! It never writes to a root. It recognises two layouts:
//!
//! - **Store layout** (Glacier's, which follows Databento's file naming):
//!   `<DATASET>/<schema>/<YYYY-MM-DD>.dbn.zst`, optionally with a symbol level,
//!   `<DATASET>/<symbol>/<schema>/<YYYY-MM-DD>.dbn.zst`, where `DATASET` is
//!   `VENUE.FEED`.
//! - **Databento batch downloads**: folders holding `metadata.json` and
//!   `manifest.json`, whose files are named `<dataset>-<YYYYMMDD>.<schema>.<encoding>`
//!   in any of Databento's encodings (DBN, CSV or JSON, compressed or not).
//!
//! Other top-level entries are listed as unrecognised. Volume usage is
//! classified against fixed thresholds (warning, critical, floor), which the
//! store's writers will act on: warn, pause non-capture writers, stop capture.
//!
//! Serves `/catalog` (see [`scan_root`] for its shape) plus the usual
//! `/health`, `/metrics` and `/describe`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::Router;
use axum::routing::get;
use clap::Parser;
use glacier_observe::{Observer, State, listen_address, unix_seconds};
use serde_json::{Value, json};

/// Read-only catalog of Glacier data roots.
#[derive(Parser, Debug)]
#[command(version, about)]
struct Args {
    /// A data root as NAME=PATH. Repeat for several roots.
    #[arg(
        long = "root",
        env = "CATALOG_ROOTS",
        value_delimiter = ';',
        required = true
    )]
    roots: Vec<String>,

    /// How often to rescan, in seconds.
    #[arg(long, env = "CATALOG_RESCAN_SECONDS", default_value_t = 300)]
    rescan_seconds: u64,

    /// Volume usage, as a percentage, at which a root is in warning.
    #[arg(long, default_value_t = 80.0)]
    warning: f64,

    /// Volume usage at which non-capture writers pause.
    #[arg(long, default_value_t = 90.0)]
    critical: f64,

    /// Volume usage at which capture stops cleanly.
    #[arg(long, default_value_t = 97.0)]
    floor: f64,
}

#[derive(Clone, Copy)]
struct Thresholds {
    warning: f64,
    critical: f64,
    floor: f64,
}

#[tokio::main]
async fn main() {
    let args = Args::parse();
    let roots: Vec<(String, PathBuf)> = args
        .roots
        .iter()
        .filter_map(|r| {
            r.split_once('=')
                .map(|(n, p)| (n.trim().to_owned(), PathBuf::from(p.trim())))
        })
        .collect();
    if roots.is_empty() {
        eprintln!("catalog: give at least one --root NAME=PATH");
        std::process::exit(2);
    }
    let limits = Thresholds {
        warning: args.warning,
        critical: args.critical,
        floor: args.floor,
    };
    let listen = listen_address("CATALOG_LISTEN", "127.0.0.1:9104");
    let observer = Observer::new("catalog", env!("CARGO_PKG_VERSION"));
    let catalog = Arc::new(Mutex::new(json!({ "generated_at": null, "roots": [] })));
    observer.describe(json!({
        "role": format!("Read-only catalog of {}.", roots.iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>().join(", ")),
        "streams": { "catalog": "/catalog" },
    }));
    {
        let c = catalog.clone();
        observer.set_metrics(move |w| {
            let c = lock(&c);
            for root in c["roots"].as_array().into_iter().flatten() {
                let name = root["name"].as_str().unwrap_or("?");
                let labels = [("root", name)];
                w.gauge(
                    "glacier_catalog_root_present",
                    "1 when the root's path exists.",
                    &labels,
                    if root["present"] == true { 1.0 } else { 0.0 },
                );
                if let Some(u) = root["volume"]["used_fraction"].as_f64() {
                    w.gauge(
                        "glacier_catalog_volume_used_ratio",
                        "Fraction of the root's volume in use.",
                        &labels,
                        u,
                    );
                }
                if let Some(b) = root["dataset_bytes"].as_f64() {
                    w.gauge(
                        "glacier_catalog_dataset_bytes",
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
            get({
                let c = catalog.clone();
                move || async move { axum::Json(lock(&c).clone()) }
            }),
        )
        .merge(observer.router());
    let listener = match tokio::net::TcpListener::bind(&listen).await {
        Ok(l) => l,
        Err(e) => {
            eprintln!("catalog: cannot listen on {listen}: {e}");
            std::process::exit(2);
        }
    };
    eprintln!(
        "catalog: {} roots; endpoints on http://{listen}",
        roots.len()
    );
    tokio::spawn(async move {
        if let Err(e) = axum::serve(listener, app).await {
            eprintln!("catalog: http server stopped: {e}");
        }
    });

    let every = Duration::from_secs(args.rescan_seconds.max(10));
    loop {
        observer.set_state(
            if lock(&catalog)["generated_at"].is_null() {
                State::Starting
            } else {
                State::Ready
            },
            "scanning",
        );
        let r = roots.clone();
        let scanned = tokio::task::spawn_blocking(move || {
            r.iter()
                .map(|(n, p)| scan_root(n, p, limits))
                .collect::<Vec<_>>()
        })
        .await
        .unwrap_or_default();
        let worst = scanned
            .iter()
            .filter_map(|r| r["volume"]["status"].as_str())
            .max_by_key(|s| severity(s))
            .unwrap_or("ok")
            .to_owned();
        let missing: Vec<&str> = scanned
            .iter()
            .filter(|r| r["present"] != true)
            .filter_map(|r| r["name"].as_str())
            .collect();
        let detail = if missing.is_empty() {
            format!("{} roots; fullest volume {worst}", scanned.len())
        } else {
            format!("missing: {}", missing.join(", "))
        };
        let degraded = !missing.is_empty() || severity(&worst) >= 2;
        *lock(&catalog) = json!({ "generated_at": unix_seconds(), "thresholds": { "warning": limits.warning, "critical": limits.critical, "floor": limits.floor }, "roots": scanned });
        observer.set_state(
            if degraded {
                State::Degraded
            } else {
                State::Ready
            },
            &detail,
        );
        tokio::time::sleep(every).await;
    }
}

fn severity(status: &str) -> u8 {
    match status {
        "warning" => 1,
        "critical" => 2,
        "floor" => 3,
        _ => 0,
    }
}

/// One root: `{ name, path, present, volume: { total_bytes, available_bytes,
/// used_fraction, status }, datasets: [{ id, sources, schemas: { schema:
/// { files, bytes, symbols, days: { "YYYY-MM-DD": bytes } } } }],
/// dataset_bytes, unrecognised: [names] }`.
fn scan_root(name: &str, path: &Path, t: Thresholds) -> Value {
    if !path.is_dir() {
        return json!({ "name": name, "path": path, "present": false, "datasets": [], "unrecognised": [] });
    }
    let volume = match shared_folder_filesystem(path) {
        // Folders shared into a container by Docker Desktop report the
        // virtual machine's disk, not the drive they live on.
        Some(fs) => json!({
            "status": "unknown",
            "reason": format!("disk space is not visible through a {fs} mount (Docker Desktop); run the catalog natively to see it"),
        }),
        None => volume(path, t),
    };

    let mut datasets: BTreeMap<String, Dataset> = BTreeMap::new();
    let mut unrecognised = Vec::new();
    for entry in entries(path) {
        let found = visit(&entry, 0, &mut datasets);
        if !found {
            unrecognised.push(file_name(&entry));
        }
    }
    let dataset_bytes: u64 = datasets
        .values()
        .flat_map(|d| d.schemas.values())
        .map(|s| s.bytes)
        .sum();
    json!({
        "name": name,
        "path": path,
        "present": true,
        "volume": volume,
        "datasets": datasets.into_iter().map(|(id, d)| d.to_json(&id)).collect::<Vec<_>>(),
        "dataset_bytes": dataset_bytes,
        "unrecognised": unrecognised,
    })
}

/// Space on the volume holding `path`, classified against the thresholds.
fn volume(path: &Path, t: Thresholds) -> Value {
    let total = fs4::total_space(path).unwrap_or(0);
    let available = fs4::available_space(path).unwrap_or(0);
    let used = if total > 0 {
        1.0 - available as f64 / total as f64
    } else {
        0.0
    };
    let pct = used * 100.0;
    let status = if pct >= t.floor {
        "floor"
    } else if pct >= t.critical {
        "critical"
    } else if pct >= t.warning {
        "warning"
    } else {
        "ok"
    };
    json!({ "total_bytes": total, "available_bytes": available, "used_fraction": used, "status": status })
}

/// The filesystem type of `path`'s mount when it is a host folder shared
/// into a container (where space figures describe something else), read
/// from `/proc/mounts`. `None` elsewhere, including on macOS.
fn shared_folder_filesystem(path: &Path) -> Option<String> {
    const SHARED: [&str; 5] = ["fakeowner", "virtiofs", "grpcfuse", "osxfs", "9p"];
    let mounts = std::fs::read_to_string("/proc/mounts").ok()?;
    let path = path.canonicalize().ok()?;
    mounts
        .lines()
        .filter_map(|l| {
            let mut f = l.split_whitespace();
            let (_, point, fs) = (f.next()?, f.next()?, f.next()?);
            path.starts_with(point)
                .then(|| (point.len(), fs.to_owned()))
        })
        .max_by_key(|(len, _)| *len)
        .map(|(_, fs)| fs)
        .filter(|fs| SHARED.contains(&fs.as_str()))
}

#[derive(Default)]
struct Dataset {
    sources: Vec<String>,
    schemas: BTreeMap<String, Schema>,
}

#[derive(Default)]
struct Schema {
    files: u64,
    bytes: u64,
    symbols: Vec<String>,
    encodings: Vec<String>,
    days: BTreeMap<String, u64>,
}

impl Dataset {
    fn to_json(&self, id: &str) -> Value {
        let schemas: serde_json::Map<String, Value> = self
            .schemas
            .iter()
            .map(|(k, s)| (k.clone(), json!({ "files": s.files, "bytes": s.bytes, "symbols": s.symbols, "encodings": s.encodings, "days": s.days })))
            .collect();
        json!({ "id": id, "sources": self.sources, "schemas": schemas })
    }
}

/// Look for datasets under `dir`; returns whether any were found.
fn visit(dir: &Path, depth: usize, datasets: &mut BTreeMap<String, Dataset>) -> bool {
    if !dir.is_dir() || depth > 3 {
        return false;
    }
    let name = file_name(dir);
    if is_dataset_name(&name) {
        return store_dataset(dir, &name, datasets);
    }
    if dir.join("metadata.json").is_file() && dir.join("manifest.json").is_file() {
        return databento_batch(dir, datasets);
    }
    let mut found = false;
    for child in entries(dir) {
        found |= visit(&child, depth + 1, datasets);
    }
    found
}

/// `<DATASET>/<schema>/<date>.dbn.zst`, or with a symbol level in between.
fn store_dataset(dir: &Path, id: &str, datasets: &mut BTreeMap<String, Dataset>) -> bool {
    let mut found = false;
    for child in entries(dir).into_iter().filter(|c| c.is_dir()) {
        if add_days(&child, None, id, &file_name(&child), datasets) {
            found = true;
        } else {
            for grandchild in entries(&child).into_iter().filter(|c| c.is_dir()) {
                found |= add_days(
                    &grandchild,
                    Some(file_name(&child)),
                    id,
                    &file_name(&grandchild),
                    datasets,
                );
            }
        }
    }
    if found {
        let d = datasets.entry(id.to_owned()).or_default();
        let source = format!("store: {}", dir.display());
        if !d.sources.contains(&source) {
            d.sources.push(source);
        }
    }
    found
}

/// Add every `<YYYY-MM-DD>.dbn.zst` in `dir` to a dataset's schema.
fn add_days(
    dir: &Path,
    symbol: Option<String>,
    id: &str,
    schema: &str,
    datasets: &mut BTreeMap<String, Dataset>,
) -> bool {
    let mut any = false;
    for f in entries(dir) {
        let n = file_name(&f);
        let Some(day) = n.strip_suffix(".dbn.zst").filter(|d| is_iso_date(d)) else {
            continue;
        };
        let size = f.metadata().map(|m| m.len()).unwrap_or(0);
        let s = datasets
            .entry(id.to_owned())
            .or_default()
            .schemas
            .entry(schema.to_owned())
            .or_default();
        note_encoding(s, "dbn.zst");
        s.files += 1;
        s.bytes += size;
        *s.days.entry(day.to_owned()).or_default() += size;
        if let Some(sym) = &symbol
            && !s.symbols.contains(sym)
        {
            s.symbols.push(sym.clone());
        }
        any = true;
    }
    any
}

/// File encodings Databento delivers, longest first.
const ENCODINGS: [&str; 6] = ["dbn.zst", "csv.zst", "json.zst", "dbn", "csv", "json"];

fn note_encoding(s: &mut Schema, encoding: &str) {
    if !s.encodings.iter().any(|e| e == encoding) {
        s.encodings.push(encoding.to_owned());
    }
}

/// A Databento batch download: `metadata.json`, `manifest.json` and
/// `<dataset>-<YYYYMMDD>.<schema>.<encoding>` files.
fn databento_batch(dir: &Path, datasets: &mut BTreeMap<String, Dataset>) -> bool {
    let meta: Value = std::fs::read_to_string(dir.join("metadata.json"))
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default();
    let Some(id) = meta["query"]["dataset"].as_str() else {
        return false;
    };
    let symbols: Vec<String> = meta["query"]["symbols"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|s| s.as_str().map(str::to_owned))
        .collect();
    let mut found = false;
    for f in entries(dir) {
        let n = file_name(&f);
        let Some((stem, encoding)) = ENCODINGS
            .iter()
            .find_map(|e| n.strip_suffix(&format!(".{e}")).map(|stem| (stem, *e)))
        else {
            continue;
        };
        // <dataset>-<YYYYMMDD>.<schema>
        let Some((head, schema)) = stem.rsplit_once('.') else {
            continue;
        };
        let Some(compact) = head
            .rsplit('-')
            .next()
            .filter(|d| d.len() == 8 && d.bytes().all(|b| b.is_ascii_digit()))
        else {
            continue;
        };
        let day = format!("{}-{}-{}", &compact[..4], &compact[4..6], &compact[6..]);
        let size = f.metadata().map(|m| m.len()).unwrap_or(0);
        let d = datasets.entry(id.to_owned()).or_default();
        let s = d.schemas.entry(schema.to_owned()).or_default();
        note_encoding(s, encoding);
        s.files += 1;
        s.bytes += size;
        *s.days.entry(day).or_default() += size;
        for sym in &symbols {
            if !s.symbols.contains(sym) {
                s.symbols.push(sym.clone());
            }
        }
        found = true;
    }
    if found {
        let job = meta["job_id"]
            .as_str()
            .map_or_else(|| file_name(dir), str::to_owned);
        datasets
            .entry(id.to_owned())
            .or_default()
            .sources
            .push(format!("databento batch {job}"));
    }
    found
}

/// `VENUE.FEED`, e.g. `OPRA.PILLAR`.
fn is_dataset_name(n: &str) -> bool {
    n.split_once('.').is_some_and(|(a, b)| {
        let ok = |s: &str| {
            !s.is_empty()
                && s.bytes()
                    .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == b'_')
        };
        ok(a) && ok(b)
    })
}

fn is_iso_date(d: &str) -> bool {
    let b = d.as_bytes();
    b.len() == 10
        && b[4] == b'-'
        && b[7] == b'-'
        && b.iter()
            .enumerate()
            .all(|(i, c)| i == 4 || i == 7 || c.is_ascii_digit())
}

/// Visible entries of a directory, sorted.
fn entries(dir: &Path) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| !file_name(p).starts_with('.'))
        .collect();
    out.sort();
    out
}

fn file_name(p: &Path) -> String {
    p.file_name()
        .map_or_else(String::new, |n| n.to_string_lossy().into_owned())
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognises_both_layouts() {
        let root = std::env::temp_dir().join(format!("glacier-catalog-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let day = root.join("OPRA.PILLAR/SPY.OPT/definition");
        std::fs::create_dir_all(&day).unwrap();
        std::fs::write(day.join("2026-09-01.dbn.zst"), b"abc").unwrap();
        std::fs::write(day.join("notes.txt"), b"x").unwrap();
        let batch = root.join("downloads/OPRA-20261003-X");
        std::fs::create_dir_all(&batch).unwrap();
        std::fs::write(batch.join("metadata.json"), r#"{"job_id":"OPRA-20261003-X","query":{"dataset":"OPRA.PILLAR","symbols":["SPY.OPT"]}}"#).unwrap();
        std::fs::write(batch.join("manifest.json"), "{}").unwrap();
        std::fs::write(batch.join("opra-pillar-20261002.cmbp-1.dbn.zst"), b"hello").unwrap();
        let csv = root.join("XNAS-1");
        std::fs::create_dir_all(&csv).unwrap();
        std::fs::write(
            csv.join("metadata.json"),
            r#"{"job_id":"XNAS-1","query":{"dataset":"XNAS.ITCH","symbols":["MSFT"]}}"#,
        )
        .unwrap();
        std::fs::write(csv.join("manifest.json"), "{}").unwrap();
        std::fs::write(csv.join("xnas-itch-20220610.trades.csv"), b"a,b").unwrap();
        std::fs::create_dir_all(root.join("misc")).unwrap();

        let t = Thresholds {
            warning: 80.0,
            critical: 90.0,
            floor: 97.0,
        };
        let r = scan_root("test", &root, t);
        assert_eq!(r["present"], true);
        let ds = &r["datasets"][0];
        assert_eq!(ds["id"], "OPRA.PILLAR");
        assert_eq!(ds["schemas"]["definition"]["days"]["2026-09-01"], 3);
        assert_eq!(ds["schemas"]["definition"]["symbols"][0], "SPY.OPT");
        assert_eq!(ds["schemas"]["cmbp-1"]["days"]["2026-10-02"], 5);
        assert_eq!(ds["sources"].as_array().unwrap().len(), 2);
        assert_eq!(r["unrecognised"], json!(["misc"]));
        assert_eq!(r["datasets"][1]["id"], "XNAS.ITCH");
        assert_eq!(
            r["datasets"][1]["schemas"]["trades"]["encodings"],
            json!(["csv"])
        );
        assert_eq!(r["dataset_bytes"], 11);
        std::fs::remove_dir_all(&root).unwrap();

        assert_eq!(scan_root("gone", &root, t)["present"], false);
    }

    #[test]
    fn names_and_dates() {
        assert!(is_dataset_name("OPRA.PILLAR"));
        assert!(is_dataset_name("GLBX.MDP3"));
        assert!(!is_dataset_name("opra.pillar"));
        assert!(!is_dataset_name("OPRA-20261003-X"));
        assert!(is_iso_date("2026-10-05"));
        assert!(!is_iso_date("20261005"));
    }
}

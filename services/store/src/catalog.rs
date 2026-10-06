//! The catalog: what datasets the store's volumes hold, which days each
//! covers, how much space they use, and how full each volume is. It only
//! reads. It recognises the store layout
//! (`<Provider>/<DATASET>/[<symbol>/]<schema>/<YYYY-MM-DD>.dbn.zst`, which
//! follows Databento's file naming) and Databento batch downloads (folders
//! holding `metadata.json` and `manifest.json`).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

#[derive(Clone, Copy)]
pub struct Thresholds {
    pub warning: f64,
    pub critical: f64,
    pub floor: f64,
}

pub fn severity(status: &str) -> u8 {
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
pub fn scan_root(name: &str, path: &Path, t: Thresholds) -> Value {
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
    let mut other = Vec::new();
    for entry in entries(path) {
        let found = visit(&entry, 0, &mut datasets);
        if !found {
            let name = file_name(&entry);
            let u = usage(&entry);
            other.push(
                json!({ "name": name, "bytes": u.bytes, "files": u.files, "modified": u.modified }),
            );
            unrecognised.push(name);
        }
    }
    let other_bytes: u64 = other.iter().filter_map(|o| o["bytes"].as_u64()).sum();
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
        "other": other,
        "other_bytes": other_bytes,
        "unrecognised": unrecognised,
    })
}

/// Space on the volume holding `path`, classified against the thresholds.
pub fn volume(path: &Path, t: Thresholds) -> Value {
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

/// The key a dataset is listed under: its name, and its symbol when the
/// layout has a symbol level.
fn key(id: &str, symbol: Option<&str>) -> String {
    symbol.map_or_else(|| id.to_owned(), |s| format!("{id} {s}"))
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
    fn to_json(&self, key: &str) -> Value {
        let (id, symbol) = key
            .split_once(' ')
            .map_or((key, None), |(a, b)| (a, Some(b)));
        let schemas: serde_json::Map<String, Value> = self
            .schemas
            .iter()
            .map(|(k, s)| (k.clone(), json!({ "files": s.files, "bytes": s.bytes, "symbols": s.symbols, "encodings": s.encodings, "days": s.days })))
            .collect();
        json!({ "id": id, "symbol": symbol, "sources": self.sources, "schemas": schemas })
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

/// `<DATASET>/<schema>/<day>`, or with a symbol level in between, where a
/// day is `<YYYY-MM-DD>.dbn.zst` or a `<YYYY-MM-DD>/` folder holding that
/// day's files (an older layout).
fn store_dataset(dir: &Path, id: &str, datasets: &mut BTreeMap<String, Dataset>) -> bool {
    let mut found = false;
    let source = format!("store: {}", dir.display());
    let note = |datasets: &mut BTreeMap<String, Dataset>, k: String| {
        let d = datasets.entry(k).or_default();
        if !d.sources.contains(&source) {
            d.sources.push(source.clone());
        }
    };
    for child in entries(dir).into_iter().filter(|c| c.is_dir()) {
        if add_days(&child, &key(id, None), &file_name(&child), datasets) {
            note(datasets, key(id, None));
            found = true;
            continue;
        }
        let sym = file_name(&child);
        for grandchild in entries(&child).into_iter().filter(|c| c.is_dir()) {
            if add_days(
                &grandchild,
                &key(id, Some(&sym)),
                &file_name(&grandchild),
                datasets,
            ) {
                note(datasets, key(id, Some(&sym)));
                found = true;
            }
        }
    }
    found
}

/// Add every day in `dir` (a `<YYYY-MM-DD>.dbn.zst` file or a `<YYYY-MM-DD>/`
/// folder) to the schema of the dataset listed under `k`.
fn add_days(dir: &Path, k: &str, schema: &str, datasets: &mut BTreeMap<String, Dataset>) -> bool {
    let mut any = false;
    for f in entries(dir) {
        let n = file_name(&f);
        let (day, size, encoding) =
            if let Some(day) = n.strip_suffix(".dbn.zst").filter(|d| is_iso_date(d)) {
                (
                    day.to_owned(),
                    f.metadata().map(|m| m.len()).unwrap_or(0),
                    "dbn.zst",
                )
            } else if is_iso_date(&n) && f.is_dir() {
                (n.clone(), usage(&f).bytes, "day folders")
            } else {
                continue;
            };
        let s = datasets
            .entry(k.to_owned())
            .or_default()
            .schemas
            .entry(schema.to_owned())
            .or_default();
        note_encoding(s, encoding);
        s.files += 1;
        s.bytes += size;
        *s.days.entry(day).or_default() += size;
        any = true;
    }
    any
}

/// Space used under a path, its file count and its newest change.
#[derive(Default, Clone, Copy)]
pub struct Usage {
    pub bytes: u64,
    pub files: u64,
    pub modified: f64,
}

/// Walk `path` (not following links), adding up its files.
pub fn usage(path: &Path) -> Usage {
    let mut u = Usage::default();
    let mut stack = vec![path.to_path_buf()];
    while let Some(p) = stack.pop() {
        let Ok(meta) = std::fs::symlink_metadata(&p) else {
            continue;
        };
        if meta.is_dir() {
            // Hidden folders count: older layouts keep data in `.archive`.
            stack.extend(
                std::fs::read_dir(&p)
                    .into_iter()
                    .flatten()
                    .flatten()
                    .map(|e| e.path()),
            );
        } else if meta.is_file() {
            u.bytes += meta.len();
            u.files += 1;
            if let Some(t) = meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            {
                u.modified = u.modified.max(t.as_secs_f64());
            }
        }
    }
    u
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
        let k = match symbols.as_slice() {
            [one] => key(id, Some(one)),
            _ => key(id, None),
        };
        let d = datasets.entry(k).or_default();
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
        let k = match symbols.as_slice() {
            [one] => key(id, Some(one)),
            _ => key(id, None),
        };
        datasets
            .entry(k)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognises_both_layouts() {
        let root = std::env::temp_dir().join(format!("tinytrader-catalog-{}", std::process::id()));
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
        std::fs::create_dir_all(root.join("misc/.archive")).unwrap();
        std::fs::write(root.join("misc/.archive/old.bin"), b"12345678").unwrap();
        // An older layout: each day a folder of that day's files.
        let older = root.join("ALPACA.SIP/QQQ/cmbp-1/2026-09-02");
        std::fs::create_dir_all(&older).unwrap();
        std::fs::write(older.join("cmbp-1.dbn.zst"), b"0123456789").unwrap();
        std::fs::write(older.join("manifest.json"), b"{}").unwrap();

        let t = Thresholds {
            warning: 80.0,
            critical: 90.0,
            floor: 97.0,
        };
        let r = scan_root("test", &root, t);
        assert_eq!(r["present"], true);
        let find = |id: &str| {
            r["datasets"]
                .as_array()
                .unwrap()
                .iter()
                .find(|d| d["id"] == id)
                .unwrap()
                .clone()
        };
        let ds = &find("OPRA.PILLAR");
        assert_eq!(ds["id"], "OPRA.PILLAR");
        assert_eq!(ds["schemas"]["definition"]["days"]["2026-09-01"], 3);
        assert_eq!(ds["symbol"], "SPY.OPT");
        assert_eq!(ds["schemas"]["cmbp-1"]["days"]["2026-10-02"], 5);
        assert_eq!(ds["sources"].as_array().unwrap().len(), 2);
        assert_eq!(r["unrecognised"], json!(["misc"]));
        assert_eq!(r["other"][0]["bytes"], 8, "hidden folders count");
        let older = r["datasets"]
            .as_array()
            .unwrap()
            .iter()
            .find(|d| d["id"] == "ALPACA.SIP")
            .unwrap();
        assert_eq!(older["symbol"], "QQQ");
        assert_eq!(older["schemas"]["cmbp-1"]["days"]["2026-09-02"], 12);
        let xnas = find("XNAS.ITCH");
        assert_eq!(xnas["schemas"]["trades"]["encodings"], json!(["csv"]));
        assert_eq!(r["dataset_bytes"], 23);
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

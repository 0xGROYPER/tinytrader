//! The catalog: what datasets the store's volumes hold, which days each
//! covers, how much space they use, and how full each volume is. It only
//! reads. It recognises the store layout
//! (`<Provider>/<DATASET>/[<symbol>/]<schema>/<YYYY-MM-DD>.dbn.zst`, which
//! follows Databento's file naming) and Databento batch downloads (folders
//! holding `metadata.json` and `manifest.json`). Every other DBN file on the
//! volume is counted from its own header, as held but not filed, so no data
//! on a volume goes uncounted (decision 0019).
//!
//! Each dataset belongs to a provider, named for the volume's top-level folder
//! it sits in (case ignored: `Databento/` and `databento/` are one provider),
//! and is listed by provider, id and symbol, never merged across providers
//! (decision 0019). Data outside every provider's folder (a batch download
//! at the top of a volume, a backup, a recording) has no provider. A folder
//! that can't be read is an error of the volume, not an empty folder.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::SystemTime;

use dbn::decode::DbnMetadata as _;

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
/// used_fraction, status }, datasets: [{ provider, id, symbol, filed, sources,
/// schemas: { schema: { files, bytes, symbols, days: { "YYYY-MM-DD": bytes },
/// partial, unfiled } } }], dataset_bytes, unrecognised: [names], errors }`.
pub fn scan_root(name: &str, path: &Path, t: Thresholds) -> Value {
    if !path.is_dir() {
        return json!({ "name": name, "path": path, "present": false, "datasets": [], "unrecognised": [], "errors": [], "pending_files": [] });
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

    let mut scan = Scan::default();
    let mut unrecognised = Vec::new();
    let mut other = Vec::new();
    for entry in listing(path, true, &mut scan) {
        let found = top(&entry, &mut scan);
        if !found {
            let name = file_name(&entry);
            let u = usage(&entry);
            other.push(
                json!({ "name": name, "bytes": u.bytes, "files": u.files, "modified": u.modified }),
            );
            unrecognised.push(name);
        }
    }
    let Scan {
        datasets,
        mut errors,
        pending,
        ..
    } = scan;
    errors.sort();
    errors.dedup();
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
        "datasets": datasets.iter().map(|(k, d)| d.to_json(k)).collect::<Vec<_>>(),
        "dataset_bytes": dataset_bytes,
        "other": other,
        "other_bytes": other_bytes,
        "unrecognised": unrecognised,
        "errors": errors,
        "pending_files": pending,
    })
}

/// The store's name for the provider whose top-level folder this is, case
/// ignored (`Databento` and `databento` are both `databento`).
#[must_use]
pub fn provider_name(folder: &str) -> Option<String> {
    crate::ingest::provider_dir(folder).map(str::to_ascii_lowercase)
}

/// Where data was found on a volume, with the provider it belongs to: a
/// dataset in the store layout, a provider's batch download (or a download
/// folder without its manifest, by its query), or any other DBN file, by its
/// header.
pub enum Place {
    Store {
        provider: Option<String>,
        dir: PathBuf,
    },
    Batch {
        provider: Option<String>,
        batch: Batch,
    },
    Loose {
        provider: Option<String>,
        header: Header,
        size: u64,
    },
}

/// Every place on the volume at `root` holding datasets, found as the
/// catalog finds them.
///
/// # Errors
/// If a folder can't be read, or a manifest, a batch's metadata or a DBN
/// file's header is unreadable.
pub fn places(root: &Path) -> Result<Vec<Place>, String> {
    let mut scan = Scan::default();
    for entry in listing(root, true, &mut scan) {
        top(&entry, &mut scan);
    }
    match scan.errors.first() {
        Some(e) => Err(e.clone()),
        None => Ok(scan.places),
    }
}

/// Look for datasets in a top-level entry of a volume: the layouts first,
/// then every DBN file they didn't claim. Returns whether any were found.
fn top(entry: &Path, scan: &mut Scan) -> bool {
    let provider = provider_name(&file_name(entry));
    let found = visit(entry, 0, provider.as_deref(), scan);
    loose(entry, provider.as_deref(), scan) || found
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

/// The key a dataset is listed under: its provider, its id, and its symbol
/// when the layout has a symbol level.
type Key = (Option<String>, String, Option<String>);

fn key(provider: Option<&str>, id: &str, symbol: Option<&str>) -> Key {
    (
        provider.map(str::to_owned),
        id.to_owned(),
        symbol.map(str::to_owned),
    )
}

/// What a walk of a volume found so far.
#[derive(Default)]
struct Scan {
    datasets: BTreeMap<Key, Dataset>,
    /// Folders, manifests, metadata or headers that couldn't be read.
    errors: Vec<String>,
    places: Vec<Place>,
    /// Files and folders the layouts have counted.
    claimed: HashSet<PathBuf>,
    /// DBN files still being written, read again on the next scan.
    pending: Vec<String>,
}

/// How recently a file whose header can't be read must have changed to be
/// taken as still being written.
const WRITING: std::time::Duration = std::time::Duration::from_secs(120);

#[derive(Default)]
struct Schema {
    files: u64,
    bytes: u64,
    symbols: Vec<String>,
    encodings: Vec<String>,
    days: BTreeMap<String, u64>,
    /// Days holding only some contracts (decision 0018); the rest are complete.
    partial: Vec<String>,
    /// Days not in the store layout: in a batch download, or a day folder.
    unfiled: Vec<String>,
}

impl Dataset {
    fn to_json(&self, (provider, id, symbol): &Key) -> Value {
        let schemas: serde_json::Map<String, Value> = self
            .schemas
            .iter()
            .map(|(k, s)| (k.clone(), json!({ "files": s.files, "bytes": s.bytes, "symbols": s.symbols, "encodings": s.encodings, "days": s.days, "partial": s.partial, "unfiled": s.unfiled })))
            .collect();
        let filed = self.schemas.values().all(|s| s.unfiled.is_empty());
        json!({ "provider": provider, "id": id, "symbol": symbol, "filed": filed, "sources": self.sources, "schemas": schemas })
    }
}

/// Look for datasets under `dir`, in `provider`'s folder if any; returns
/// whether any were found.
fn visit(dir: &Path, depth: usize, provider: Option<&str>, scan: &mut Scan) -> bool {
    if !dir.is_dir() {
        return false;
    }
    // Deeper, the files are counted by their headers.
    if depth > 3 {
        return false;
    }
    let name = file_name(dir);
    if is_dataset_name(&name) {
        scan.places.push(Place::Store {
            provider: provider.map(str::to_owned),
            dir: dir.to_path_buf(),
        });
        return store_dataset(dir, &name, provider, scan);
    }
    if dir.join("metadata.json").is_file() && dir.join("manifest.json").is_file() {
        return databento_batch(dir, provider, scan);
    }
    let mut found = false;
    for child in listing(dir, true, scan) {
        found |= visit(&child, depth + 1, provider, scan);
    }
    found
}

/// Count every DBN file under `path` that the layouts didn't claim, from its
/// header, as `provider`'s if in its folder (decision 0019), and a download
/// folder without its manifest by its query. Hidden folders (such as
/// `.incoming`) are skipped. A file whose header can't be read yet, changed
/// in the last two minutes, is still being written: it is listed as pending
/// and read again on the next scan. Returns whether any were found.
fn loose(path: &Path, provider: Option<&str>, scan: &mut Scan) -> bool {
    let Ok(meta) = std::fs::symlink_metadata(path) else {
        return false;
    };
    if scan.claimed.contains(path) {
        return false;
    }
    if meta.is_dir() {
        let mut found = path.join("metadata.json").is_file() && unmanifested(path, provider, scan);
        for child in listing(path, true, scan) {
            found |= loose(&child, provider, scan);
        }
        return found;
    }
    let name = file_name(path);
    let Some(encoding) = ["dbn.zst", "dbn"]
        .into_iter()
        .find(|e| name.ends_with(&format!(".{e}")))
    else {
        return false;
    };
    if !meta.is_file() {
        return false;
    }
    let h = match header(path) {
        Ok(h) => h,
        Err(e) => {
            let recent = meta
                .modified()
                .ok()
                .and_then(|t| t.elapsed().ok())
                .is_some_and(|age| age < WRITING);
            if recent {
                scan.pending.push(path.display().to_string());
            } else {
                scan.errors.push(e);
            }
            return false;
        }
    };
    let raw = h.stype_in.as_deref() == Some("raw_symbol");
    let k = match h.symbols.as_slice() {
        [one] if !raw => key(provider, &h.dataset, Some(one)),
        _ => key(provider, &h.dataset, None),
    };
    let d = scan.datasets.entry(k).or_default();
    let source = format!("files in {}", path.parent().unwrap_or(path).display());
    if !d.sources.contains(&source) {
        d.sources.push(source);
    }
    let s = d
        .schemas
        .entry(h.schema.clone().unwrap_or_else(|| "mixed".into()))
        .or_default();
    note_encoding(s, encoding);
    s.files += 1;
    s.bytes += meta.len();
    for (i, day) in h.days().into_iter().enumerate() {
        *s.days.entry(day.clone()).or_default() += if i == 0 { meta.len() } else { 0 };
        if raw && !s.partial.contains(&day) {
            s.partial.push(day.clone());
        }
        if !s.unfiled.contains(&day) {
            s.unfiled.push(day);
        }
    }
    for sym in &h.symbols {
        if !s.symbols.contains(sym) {
            s.symbols.push(sym.clone());
        }
    }
    scan.places.push(Place::Loose {
        provider: provider.map(str::to_owned),
        header: h,
        size: meta.len(),
    });
    true
}

/// A download folder with `metadata.json` but no manifest, such as one
/// extracted by hand: its DBN files are counted by their headers; when it
/// has none, or has data in other encodings, every day of its query counts.
fn unmanifested(dir: &Path, provider: Option<&str>, scan: &mut Scan) -> bool {
    let path = dir.join("metadata.json");
    let meta: Value = match std::fs::read_to_string(&path)
        .map_err(|e| e.to_string())
        .and_then(|t| serde_json::from_str(&t).map_err(|e| e.to_string()))
    {
        Ok(m) => m,
        Err(e) => {
            scan.errors.push(format!("{}: {e}", path.display()));
            return false;
        }
    };
    let q = &meta["query"];
    let Some(dataset) = q["dataset"].as_str() else {
        return false;
    };
    let names: Vec<String> = listing(dir, true, scan)
        .iter()
        .map(|p| file_name(p))
        .collect();
    let dbn = names
        .iter()
        .any(|n| n.ends_with(".dbn") || n.ends_with(".dbn.zst"));
    let other = names.iter().any(|n| {
        ENCODINGS[1..].iter().any(|e| {
            !e.starts_with("dbn")
                && n.strip_suffix(&format!(".{e}"))
                    .is_some_and(|stem| stem.contains('.'))
        })
    });
    if dbn && !other {
        return false;
    }
    let (Some(days), Some(schema)) = (query_days(q), q["schema"].as_str()) else {
        scan.errors.push(format!(
            "{}: a download without its manifest, whose metadata.json gives no query start, end and schema",
            dir.display()
        ));
        return false;
    };
    let batch = Batch {
        dataset: dataset.to_owned(),
        symbols: strings(&q["symbols"]),
        stype_in: q["stype_in"].as_str().map(str::to_owned),
        job: meta["job_id"]
            .as_str()
            .map_or_else(|| file_name(dir), str::to_owned),
        files: days
            .into_iter()
            .map(|d| (d, schema.to_owned(), "query", 0))
            .collect(),
    };
    count_batch(batch, provider, scan)
}

/// What a DBN file's header says it holds.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Header {
    pub dataset: String,
    /// `None` for a file of several schemas.
    pub schema: Option<String>,
    pub stype_in: Option<String>,
    pub symbols: Vec<String>,
    pub start: u64,
    /// Its end (excluded), or its last change when the header has none.
    pub end: u64,
}

impl Header {
    /// The days from its start to its end (excluded).
    #[must_use]
    pub fn days(&self) -> Vec<String> {
        days_between(
            &crate::ingest::day_of(self.start),
            &crate::ingest::day_of(self.end.saturating_sub(1).max(self.start)),
        )
    }
}

/// Headers read, by path, kept while the file's size and last change stay
/// the same, so a rescan doesn't read them again.
type HeaderCache = HashMap<PathBuf, (u64, SystemTime, Header)>;

fn headers() -> &'static Mutex<HeaderCache> {
    static CACHE: OnceLock<Mutex<HeaderCache>> = OnceLock::new();
    CACHE.get_or_init(Mutex::default)
}

/// A DBN file's header, from the cache while the file is unchanged.
///
/// # Errors
/// If the file or its header can't be read.
pub fn header(path: &Path) -> Result<Header, String> {
    let fail = |e: &dyn std::fmt::Display| format!("{}: {e}", path.display());
    let meta = std::fs::metadata(path).map_err(|e| fail(&e))?;
    let changed = meta.modified().map_err(|e| fail(&e))?;
    if let Some((size, at, h)) = tinytrader_observe::lock(headers()).get(path)
        && *size == meta.len()
        && *at == changed
    {
        return Ok(h.clone());
    }
    let d = dbn::decode::DynDecoder::from_file(path, dbn::VersionUpgradePolicy::AsIs)
        .map_err(|e| fail(&format!("not a readable DBN file: {e}")))?;
    let m = d.metadata();
    let last_change = changed
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |t| u64::try_from(t.as_nanos()).unwrap_or(u64::MAX));
    let h = Header {
        dataset: m.dataset.clone(),
        schema: m.schema.map(|s| s.as_str().to_owned()),
        stype_in: m.stype_in.map(|s| s.as_str().to_owned()),
        symbols: m.symbols.clone(),
        start: m.start,
        end: m.end.map_or(last_change, std::num::NonZeroU64::get),
    };
    tinytrader_observe::lock(headers())
        .insert(path.to_path_buf(), (meta.len(), changed, h.clone()));
    Ok(h)
}

/// `<DATASET>/<schema>/<day>`, or with a symbol level in between, where a
/// day is `<YYYY-MM-DD>.dbn.zst` or a `<YYYY-MM-DD>/` folder holding that
/// day's files (an older layout).
fn store_dataset(dir: &Path, id: &str, provider: Option<&str>, scan: &mut Scan) -> bool {
    let mut found = false;
    let source = format!("store: {}", dir.display());
    let note = |scan: &mut Scan, k: Key| {
        let d = scan.datasets.entry(k).or_default();
        if !d.sources.contains(&source) {
            d.sources.push(source.clone());
        }
    };
    for child in listing(dir, true, scan).into_iter().filter(|c| c.is_dir()) {
        let k = key(provider, id, None);
        if add_days(&child, &k, &file_name(&child), scan) {
            note(scan, k);
            found = true;
            continue;
        }
        let sym = file_name(&child);
        for grandchild in listing(&child, true, scan)
            .into_iter()
            .filter(|c| c.is_dir())
        {
            let k = key(provider, id, Some(&sym));
            if add_days(&grandchild, &k, &file_name(&grandchild), scan) {
                note(scan, k);
                found = true;
            }
        }
    }
    found
}

/// Add every day in `dir` (a `<YYYY-MM-DD>.dbn.zst` file or a `<YYYY-MM-DD>/`
/// folder) to the schema of the dataset listed under `k`.
fn add_days(dir: &Path, k: &Key, schema: &str, scan: &mut Scan) -> bool {
    let mut any = false;
    // Partial days are those the folder's manifest lists (its coverage); an
    // unreadable manifest is an error of the volume.
    let partial = crate::partial::read_manifest(dir).unwrap_or_else(|e| {
        scan.errors.push(e);
        BTreeMap::new()
    });
    for f in listing(dir, true, scan) {
        let n = file_name(&f);
        let (day, size, encoding) = if let Some(day) =
            n.strip_suffix(".dbn.zst").filter(|d| is_iso_date(d))
            && f.is_file()
        {
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
        scan.claimed.insert(f.clone());
        let s = scan
            .datasets
            .entry(k.clone())
            .or_default()
            .schemas
            .entry(schema.to_owned())
            .or_default();
        note_encoding(s, encoding);
        s.files += 1;
        s.bytes += size;
        if partial.contains_key(&day) && !s.partial.contains(&day) {
            s.partial.push(day.clone());
        }
        if encoding == "day folders" && !s.unfiled.contains(&day) {
            s.unfiled.push(day.clone());
        }
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

/// What a Databento batch download holds, from its `metadata.json` and the
/// names of its `<dataset>-<YYYYMMDD>.<schema>.<encoding>` files.
#[derive(Clone, Debug)]
pub struct Batch {
    pub dataset: String,
    pub symbols: Vec<String>,
    pub stype_in: Option<String>,
    pub job: String,
    /// Each day of each schema it holds, with the encoding and the bytes
    /// counted to it (a file covering several days counts on its first).
    pub files: Vec<(String, String, &'static str, u64)>,
}

/// The days a batch file's name says it holds: `<dataset>-<YYYYMMDD>`, or
/// `<dataset>-<YYYYMMDD>-<YYYYMMDD>` for a range (both ends included).
fn named_days(head: &str) -> Option<Vec<String>> {
    let date = |d: &str| {
        (d.len() == 8 && d.bytes().all(|b| b.is_ascii_digit()))
            .then(|| format!("{}-{}-{}", &d[..4], &d[4..6], &d[6..]))
            .filter(|d| crate::ingest::day_ns(d).is_some())
    };
    let mut parts = head.rsplit('-');
    let last = date(parts.next()?)?;
    let first = parts.next().and_then(date).unwrap_or_else(|| last.clone());
    let days = days_between(&first, &last);
    (!days.is_empty()).then_some(days)
}

/// Every day from `first` to `last`, both included.
fn days_between(first: &str, last: &str) -> Vec<String> {
    const DAY: u64 = 86_400_000_000_000;
    let (Some(a), Some(b)) = (crate::ingest::day_ns(first), crate::ingest::day_ns(last)) else {
        return Vec::new();
    };
    (a / DAY..=b / DAY)
        .map(|d| crate::ingest::day_of(d * DAY))
        .collect()
}

/// The days of a batch's query, from its `metadata.json`: `start` to `end`
/// as nanoseconds (end excluded) or dates (end counted, to be safe).
fn query_days(q: &Value) -> Option<Vec<String>> {
    let day = |v: &Value, end: bool| -> Option<String> {
        let ns = v.as_u64().or_else(|| {
            v.as_str()
                .filter(|s| s.bytes().all(|b| b.is_ascii_digit()))
                .and_then(|s| s.parse().ok())
        });
        match ns {
            Some(ns) => Some(crate::ingest::day_of(if end {
                ns.saturating_sub(1)
            } else {
                ns
            })),
            None => v
                .as_str()
                .and_then(|s| s.get(..10))
                .filter(|d| crate::ingest::day_ns(d).is_some())
                .map(str::to_owned),
        }
    };
    let days = days_between(&day(&q["start"], false)?, &day(&q["end"], true)?);
    (!days.is_empty()).then_some(days)
}

/// Read the batch download in `dir`; `None` if its metadata names no dataset.
///
/// # Errors
/// If its `metadata.json` or the folder can't be read.
pub fn read_batch(dir: &Path) -> Result<Option<Batch>, String> {
    let path = dir.join("metadata.json");
    let meta: Value = std::fs::read_to_string(&path)
        .map_err(|e| e.to_string())
        .and_then(|t| serde_json::from_str(&t).map_err(|e| e.to_string()))
        .map_err(|e| format!("{}: {e}", path.display()))?;
    let Some(dataset) = meta["query"]["dataset"].as_str() else {
        return Ok(None);
    };
    let q = &meta["query"];
    let mut files = Vec::new();
    // Files split by symbol, or whose names don't say their days.
    let mut unnamed: Option<(&'static str, u64)> = None;
    for f in read_entries(dir)? {
        let n = file_name(&f);
        let Some((stem, encoding)) = ENCODINGS
            .iter()
            .find_map(|e| n.strip_suffix(&format!(".{e}")).map(|stem| (stem, *e)))
        else {
            continue;
        };
        let size = f.metadata().map(|m| m.len()).unwrap_or(0);
        // <dataset>-<YYYYMMDD>[-<YYYYMMDD>].<schema>; receipts such as
        // `metadata.json` have no schema.
        let named = stem
            .rsplit_once('.')
            .and_then(|(head, schema)| Some((named_days(head)?, schema)));
        match named {
            Some((days, schema)) if meta["customizations"]["split_symbols"] != true => {
                for (i, day) in days.into_iter().enumerate() {
                    files.push((
                        day,
                        schema.to_owned(),
                        encoding,
                        if i == 0 { size } else { 0 },
                    ));
                }
            }
            _ if stem.contains('.') || encoding.starts_with("dbn") => {
                let (_, bytes) = unnamed.get_or_insert((encoding, 0));
                *bytes += size;
            }
            _ => {}
        }
    }
    // Their days are the query's, all of them.
    if let Some((encoding, bytes)) = unnamed {
        let (Some(days), Some(schema)) = (query_days(q), q["schema"].as_str()) else {
            return Err(format!(
                "{}: some files' names don't say their days, and metadata.json gives no query start, end and schema",
                dir.display()
            ));
        };
        for (i, day) in days.into_iter().enumerate() {
            if !files.iter().any(|f| f.0 == day && f.1 == schema) {
                files.push((
                    day,
                    schema.to_owned(),
                    encoding,
                    if i == 0 { bytes } else { 0 },
                ));
            }
        }
    }
    Ok(Some(Batch {
        dataset: dataset.to_owned(),
        symbols: strings(&meta["query"]["symbols"]),
        stype_in: meta["query"]["stype_in"].as_str().map(str::to_owned),
        job: meta["job_id"]
            .as_str()
            .map_or_else(|| file_name(dir), str::to_owned),
        files,
    }))
}

/// A Databento batch download, listed under its one symbol, or under none
/// when it has several. Its days are unfiled.
fn databento_batch(dir: &Path, provider: Option<&str>, scan: &mut Scan) -> bool {
    let b = match read_batch(dir) {
        Ok(Some(b)) if !b.files.is_empty() => b,
        Ok(_) => return false,
        Err(e) => {
            scan.errors.push(e);
            return false;
        }
    };
    scan.claimed.insert(dir.to_path_buf());
    count_batch(b, provider, scan)
}

/// Count a batch's days, unfiled, under its one symbol or under none.
fn count_batch(b: Batch, provider: Option<&str>, scan: &mut Scan) -> bool {
    let k = match b.symbols.as_slice() {
        [one] => key(provider, &b.dataset, Some(one)),
        _ => key(provider, &b.dataset, None),
    };
    let d = scan.datasets.entry(k).or_default();
    for (day, schema, encoding, size) in &b.files {
        let s = d.schemas.entry(schema.clone()).or_default();
        note_encoding(s, encoding);
        s.files += 1;
        s.bytes += size;
        *s.days.entry(day.clone()).or_default() += size;
        if !s.unfiled.contains(day) {
            s.unfiled.push(day.clone());
        }
        for sym in &b.symbols {
            if !s.symbols.contains(sym) {
                s.symbols.push(sym.clone());
            }
        }
    }
    d.sources.push(format!("databento batch {}", b.job));
    scan.places.push(Place::Batch {
        provider: provider.map(str::to_owned),
        batch: b,
    });
    true
}

/// The strings of a JSON array.
fn strings(v: &Value) -> Vec<String> {
    v.as_array()
        .into_iter()
        .flatten()
        .filter_map(|s| s.as_str().map(str::to_owned))
        .collect()
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
///
/// # Errors
/// If the directory can't be read.
pub fn read_entries(dir: &Path) -> Result<Vec<PathBuf>, String> {
    let read = std::fs::read_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let mut out = Vec::new();
    for e in read {
        let p = e.map_err(|e| format!("{}: {e}", dir.display()))?.path();
        if !file_name(&p).starts_with('.') {
            out.push(p);
        }
    }
    out.sort();
    Ok(out)
}

/// As [`read_entries`]; when `strict` (a provider's or a dataset's folder),
/// a folder that can't be read is noted as an error.
fn listing(dir: &Path, strict: bool, scan: &mut Scan) -> Vec<PathBuf> {
    read_entries(dir).unwrap_or_else(|e| {
        if strict {
            scan.errors.push(e);
        }
        Vec::new()
    })
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
        // Outside every provider's folder, the store layout and the batch
        // download beside it have no provider, and are one dataset.
        let ds = &find("OPRA.PILLAR");
        assert_eq!(ds["id"], "OPRA.PILLAR");
        assert_eq!(
            (ds["provider"].clone(), ds["filed"].clone()),
            (Value::Null, json!(false))
        );
        assert_eq!(ds["schemas"]["definition"]["days"]["2026-09-01"], 3);
        assert_eq!(ds["symbol"], "SPY.OPT");
        assert_eq!(ds["schemas"]["cmbp-1"]["days"]["2026-10-02"], 5);
        assert_eq!(ds["schemas"]["cmbp-1"]["unfiled"], json!(["2026-10-02"]));
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
        assert_eq!(older["filed"], false, "the older layout's day folders");
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

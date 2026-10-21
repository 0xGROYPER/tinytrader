//! Partial days (decision 0018): a day file that holds a list of contracts
//! for part of the day, not a provider's whole parent symbol.
//!
//! - **Coverage.** A partial day's contracts and time window come from its own
//!   DBN header (`stype_in = raw_symbol`, its symbols, start and end), checked
//!   against every record: each must map to a listed contract and fall inside
//!   the window. It is recorded in the schema folder's `manifest.json`, beside
//!   the file's size and SHA-256. A day with no entry there is complete (every
//!   contract of the parent, as a parent-symbol batch delivers it).
//! - **Import.** DBN that didn't come from a provider batch (a timeseries
//!   request) is uploaded to `.incoming/<id>/` with an `import.json` naming
//!   the dataset, schema, parent symbol, where it came from, and each file's
//!   size and SHA-256. Filing is as for a batch: every check before any move,
//!   moves by link, a ledger in `migrations/`, receipts in `provenance/<id>/`.
//! - **Merge.** A partial day already held, given contracts it doesn't hold,
//!   is replaced by the two merged in timestamp order: written beside it,
//!   checked record for record against its inputs, recorded in the manifest
//!   as pending, swapped in by rename, then made current. An interrupted swap
//!   is settled from the file's hash on the next filing of that day.
//!   Contracts already held are never filed twice.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use dbn::decode::{DbnMetadata as _, DecodeRecordRef as _};
use dbn::encode::{EncodeRecord as _, EncodeRecordRef as _};
use dbn::{Metadata, Record as _, RecordBuf, SType, VersionUpgradePolicy};
use serde_json::{Value, json};

use crate::ingest::{
    self, INCOMING, Ledger, Part, check_header, hash_file, place_new, safe, sync_dir, write_new,
};

/// A schema folder's record of its partial days.
pub const MANIFEST: &str = "manifest.json";
const MANIFEST_SCHEMA: &str = "tinytrader.store-manifest/1";
/// An import's receipt: what was uploaded, and where it came from.
pub const IMPORT: &str = "import.json";

/// What a partial day holds: its contracts and time window (ns, end exclusive).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Coverage {
    pub symbols: BTreeSet<String>,
    pub start: u64,
    pub end: u64,
}

impl Coverage {
    #[must_use]
    pub fn to_json(&self) -> Value {
        json!({ "kind": "contracts", "stype": "raw_symbol", "symbols": self.symbols, "window": [self.start, self.end] })
    }

    #[must_use]
    pub fn from_json(v: &Value) -> Option<Self> {
        if v["kind"] != "contracts" {
            return None;
        }
        Some(Self {
            symbols: v["symbols"]
                .as_array()?
                .iter()
                .filter_map(|s| s.as_str().map(str::to_owned))
                .collect(),
            start: v["window"][0].as_u64()?,
            end: v["window"][1].as_u64()?,
        })
    }

    fn union(&self, other: &Self) -> Self {
        Self {
            symbols: self.symbols.union(&other.symbols).cloned().collect(),
            start: self.start.min(other.start),
            end: self.end.max(other.end),
        }
    }
}

/// Open a day file for reading, records upgraded to the current DBN version
/// so files of different versions read (and merge) alike.
fn open(path: &Path) -> Result<dbn::decode::dbn::Decoder<impl std::io::Read>, String> {
    let mut d = dbn::decode::dbn::Decoder::from_zstd_file(path)
        .map_err(|e| format!("not a readable DBN file: {e}"))?;
    d.set_upgrade_policy(VersionUpgradePolicy::UpgradeToV3)
        .map_err(|e| e.to_string())?;
    Ok(d)
}

/// A partial day's coverage and record count, from its header, checked
/// against every record.
///
/// # Errors
/// If the file isn't a contract list (`stype_in = raw_symbol`), or a record
/// maps to no listed contract or falls outside the header's window.
pub fn inspect(path: &Path) -> Result<(Coverage, u64), String> {
    let mut d = open(path)?;
    let m = d.metadata().clone();
    if m.stype_in != Some(SType::RawSymbol) || m.symbols.is_empty() {
        return Err(
            "its header isn't a list of contracts (stype_in raw_symbol); file whole-symbol batches with /incorporate"
                .into(),
        );
    }
    let symbols: BTreeSet<String> = m.symbols.iter().cloned().collect();
    let end = m.end.ok_or("its header has no end time")?.get();
    let mut ids = HashSet::new();
    for sm in &m.mappings {
        if !symbols.contains(&sm.raw_symbol) {
            return Err(format!(
                "its header maps {} but doesn't list it",
                sm.raw_symbol
            ));
        }
        for iv in &sm.intervals {
            ids.insert(iv.symbol.parse::<u32>().map_err(|_| {
                format!(
                    "{}: mapped to {:?}, not an instrument ID",
                    sm.raw_symbol, iv.symbol
                )
            })?);
        }
    }
    let mut n = 0u64;
    while let Some(r) = d.decode_record_ref().map_err(|e| e.to_string())? {
        let id = r.header().instrument_id;
        if !ids.contains(&id) {
            return Err(format!(
                "record {n} is instrument {id}, which maps to none of its contracts"
            ));
        }
        let t = r.raw_index_ts();
        if t < m.start || t >= end {
            return Err(format!(
                "record {n} at {t} is outside its window {}..{end}",
                m.start
            ));
        }
        n += 1;
    }
    Ok((
        Coverage {
            symbols,
            start: m.start,
            end,
        },
        n,
    ))
}

/// Every record of a day file, in order, and its header.
fn read_all(path: &Path) -> Result<(Metadata, Vec<RecordBuf>), String> {
    let mut d = open(path)?;
    let m = d.metadata().clone();
    let mut out = Vec::new();
    while let Some(r) = d.decode_record_ref().map_err(|e| e.to_string())? {
        out.push(r.to_owned());
    }
    Ok((m, out))
}

/// The records of `a` and `b` interleaved by their index timestamp (ties: `a`
/// first, then each file's own order), and their headers merged.
fn interleave(a: &Path, b: &Path) -> Result<(Metadata, Vec<RecordBuf>), String> {
    let (ma, ra) = read_all(a)?;
    let (mb, rb) = read_all(b)?;
    let m = ma
        .merge([mb])
        .map_err(|e| format!("the two headers can't be merged: {e}"))?;
    let mut all: Vec<(u64, u8, usize, RecordBuf)> = ra
        .into_iter()
        .enumerate()
        .map(|(i, r)| (r.raw_index_ts(), 0, i, r))
        .chain(
            rb.into_iter()
                .enumerate()
                .map(|(i, r)| (r.raw_index_ts(), 1, i, r)),
        )
        .collect();
    all.sort_by_key(|(t, w, i, _)| (*t, *w, *i));
    Ok((m, all.into_iter().map(|(.., r)| r).collect()))
}

/// Merge two partial days into a new file `out`, then read it back and check
/// it holds exactly their records, in order. Returns its record count.
///
/// # Errors
/// If the headers can't be merged, `out` exists, or the file read back
/// differs from what was written.
pub fn merge(a: &Path, b: &Path, out: &Path) -> Result<u64, String> {
    let (m, records) = interleave(a, b)?;
    let file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(out)
        .map_err(|e| format!("{}: {e}", out.display()))?;
    {
        let mut enc = dbn::encode::dbn::Encoder::with_zstd(&file, &m).map_err(|e| e.to_string())?;
        for r in &records {
            enc.encode_record_ref(r.as_rec_ref())
                .map_err(|e| e.to_string())?;
        }
        enc.flush().map_err(|e| e.to_string())?;
    }
    file.sync_all().map_err(|e| e.to_string())?;
    if let Some(dir) = out.parent() {
        sync_dir(dir).map_err(|e| e.to_string())?;
    }
    // Read back: the same records, in the same order, and nothing else.
    let (back_m, back) = read_all(out)?;
    if back.len() != records.len() {
        return Err(format!(
            "the merged file read back {} records, not {}",
            back.len(),
            records.len()
        ));
    }
    if let Some(i) = back.iter().zip(&records).position(|(x, y)| x != y) {
        return Err(format!("the merged file differs at record {i}"));
    }
    if back_m.symbols != m.symbols {
        return Err("the merged file's contracts differ from its inputs'".into());
    }
    Ok(records.len() as u64)
}

/// A schema folder's manifest: day → entry. A missing file is an empty one.
pub fn read_manifest(dir: &Path) -> Result<BTreeMap<String, Value>, String> {
    let path = dir.join(MANIFEST);
    match fs::read_to_string(&path) {
        Ok(text) => {
            let v: Value =
                serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?;
            Ok(v["days"]
                .as_object()
                .map(|o| o.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
                .unwrap_or_default())
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(BTreeMap::new()),
        Err(e) => Err(format!("{}: {e}", path.display())),
    }
}

/// Replace a schema folder's manifest: written beside it, flushed, renamed
/// over it, and the folder flushed, so it is always one whole version.
fn write_manifest(dir: &Path, days: &BTreeMap<String, Value>) -> Result<(), String> {
    fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let text = serde_json::to_string_pretty(&json!({ "schema": MANIFEST_SCHEMA, "days": days }))
        .map_err(|e| e.to_string())?;
    let tmp = dir.join(format!(".{MANIFEST}.new"));
    let _ = fs::remove_file(&tmp);
    write_new(&tmp, text.as_bytes()).map_err(|e| format!("{}: {e}", tmp.display()))?;
    fs::rename(&tmp, dir.join(MANIFEST)).map_err(|e| format!("{}: {e}", dir.display()))?;
    sync_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))
}

/// Set (or with `None`, remove) one day's entry.
fn set_entry(dir: &Path, day: &str, entry: Option<Value>) -> Result<(), String> {
    let mut days = read_manifest(dir)?;
    match entry {
        Some(e) => days.insert(day.to_owned(), e),
        None => days.remove(day),
    };
    write_manifest(dir, &days)
}

/// Settle a day whose merge was interrupted: the file's hash says whether the
/// swap happened (the pending entry becomes current) or not (it's dropped).
/// Returns the day's entry as it now stands.
fn settle(dir: &Path, day: &str) -> Result<Option<Value>, String> {
    let days = read_manifest(dir)?;
    let Some(mut entry) = days.get(day).cloned() else {
        return Ok(None);
    };
    let Some(pending) = entry.get("pending").cloned() else {
        return Ok(Some(entry));
    };
    let file = dir.join(format!("{day}.dbn.zst"));
    let got = hash_file(&file)?;
    entry = if pending["sha256"] == got.as_str() {
        pending
    } else if entry["sha256"] == got.as_str() {
        entry.as_object_mut().map(|o| o.remove("pending"));
        entry
    } else {
        return Err(format!(
            "{}: SHA-256 {got} is neither its recorded file nor its pending merge",
            file.display()
        ));
    };
    set_entry(dir, day, Some(entry.clone()))?;
    Ok(Some(entry))
}

/// `YYYY-MM-DD` of an import's file name, `YYYY-MM-DD.dbn.zst`.
fn import_day(name: &str) -> Option<String> {
    let day = name.strip_suffix(".dbn.zst")?;
    (day.len() == 10 && ingest::day_ns(day).is_some() && day.as_bytes()[4] == b'-')
        .then(|| day.to_owned())
}

/// Do these contracts belong to `parent`? For options, a parent `ROOT.OPT`
/// holds OCC symbols whose root (the first six characters, padded) is `ROOT`.
fn belongs(parent: &str, symbols: &BTreeSet<String>) -> Result<(), String> {
    if !parent.ends_with(".OPT") {
        return Err(format!(
            "only option parents (ROOT.OPT) take contract lists, not {parent}"
        ));
    }
    match symbols.iter().find(|s| !of_parent(parent, s)) {
        Some(s) => Err(format!("{s:?} isn't a contract of {parent}")),
        None => Ok(()),
    }
}

/// What happens to one imported day.
enum Step {
    /// Nothing holds the day: file it.
    Place {
        from: PathBuf,
        to: PathBuf,
        entry: Value,
    },
    /// Byte-identical to the day already held.
    Duplicate { from: PathBuf, existing: PathBuf },
    /// A partial day is held with other contracts: merge the two.
    Merge {
        from: PathBuf,
        existing: PathBuf,
        held: Coverage,
        new: Coverage,
    },
}

/// File an import waiting in `.incoming/<id>` on one or more volumes.
///
/// # Errors
/// If `import.json` is missing or malformed, a file doesn't match it or its
/// own header, or a day can't be filed: held complete, held as a different
/// file, or held partly with some of the same contracts.
#[expect(
    clippy::too_many_lines,
    reason = "one pass of checks, then one of moves, kept together so nothing moves before every check"
)]
pub fn import(
    parts: &[Part],
    elsewhere: &[PathBuf],
    id: &str,
    today: &str,
) -> Result<Value, String> {
    let roots: Vec<&Path> = parts.iter().map(|p| p.root.as_path()).collect();
    let first = parts.first().ok_or("no part of the import was found")?;
    let holder = |name: &str| parts.iter().find(|p| p.inc.join(name).exists());
    let receipt_path = holder(IMPORT)
        .ok_or_else(|| format!("{IMPORT}: missing"))?
        .inc
        .join(IMPORT);
    let receipt: Value = serde_json::from_str(
        &fs::read_to_string(&receipt_path).map_err(|e| format!("{IMPORT}: {e}"))?,
    )
    .map_err(|e| format!("{IMPORT}: {e}"))?;
    let field = |k: &str| {
        receipt[k]
            .as_str()
            .filter(|s| safe(s))
            .ok_or_else(|| format!("{IMPORT} has no usable {k:?}"))
    };
    let (dataset, schema, parent) = (field("dataset")?, field("schema")?, field("symbol")?);
    if receipt["source"]
        .as_object()
        .is_none_or(serde_json::Map::is_empty)
    {
        return Err(format!(
            "{IMPORT} must say where the data came from (\"source\": the request, the tool and its commit, when it was fetched)"
        ));
    }
    let level = PathBuf::from(dataset).join(parent).join(schema);
    let mut everywhere: Vec<PathBuf> = roots.iter().map(|r| r.to_path_buf()).collect();
    everywhere.extend(
        elsewhere
            .iter()
            .filter(|e| !roots.contains(&e.as_path()))
            .cloned(),
    );

    let mut ledger = Ledger::open(
        &roots
            .iter()
            .map(|r| {
                r.join("migrations")
                    .join(format!("{today}-import-{id}.events.jsonl"))
            })
            .collect::<Vec<_>>(),
    )?;
    ledger.record(json!({ "step": "begin", "import": id, "dataset": dataset, "schema": schema, "symbol": parent, "parts": parts.iter().map(|p| &p.inc).collect::<Vec<_>>() }))?;

    let files = receipt["files"]
        .as_array()
        .filter(|f| !f.is_empty())
        .ok_or_else(|| format!("{IMPORT} lists no files"))?;
    let mut steps = Vec::new();
    let mut problems = Vec::new();
    let mut check = |f: &Value| -> Result<(), String> {
        let name = f["filename"].as_str().unwrap_or_default();
        // The day: given (a provider batch's file names say it their own way),
        // or the file's name, YYYY-MM-DD.dbn.zst.
        let day = f["day"]
            .as_str()
            .filter(|d| d.len() == 10 && ingest::day_ns(d).is_some())
            .map(str::to_owned)
            .or_else(|| import_day(name))
            .filter(|_| safe(name))
            .ok_or_else(|| format!("{name:?}: files are named YYYY-MM-DD.dbn.zst"))?;
        let want = f["hash"]
            .as_str()
            .unwrap_or_default()
            .trim_start_matches("sha256:")
            .to_ascii_lowercase();
        let part = holder(name).unwrap_or(first);
        let path = part.inc.join(name);
        let rel = level.join(format!("{day}.dbn.zst"));
        // The day where it's held, settled if a merge was interrupted.
        let mut held = None;
        for r in &everywhere {
            let p = r.join(&rel);
            if p.exists() {
                let entry = settle(&r.join(&level), &day)?;
                held = Some((p, entry));
                break;
            }
        }
        let Ok(meta) = fs::metadata(&path) else {
            // Filed by an earlier, interrupted run?
            if let Some((p, _)) = &held
                && hash_file(p)? == want
            {
                return Ok(());
            }
            return Err(format!("{name}: missing"));
        };
        if f["size"].as_u64().is_some_and(|s| s != meta.len()) {
            return Err(format!(
                "{name}: {} bytes, {IMPORT} says {}",
                meta.len(),
                f["size"]
            ));
        }
        let got = hash_file(&path)?;
        if got != want {
            return Err(format!("{name}: SHA-256 {got}, {IMPORT} says {want}"));
        }
        check_header(&path, dataset, schema, &day).map_err(|e| format!("{name}: {e}"))?;
        let (cov, records) = inspect(&path).map_err(|e| format!("{name}: {e}"))?;
        belongs(parent, &cov.symbols).map_err(|e| format!("{name}: {e}"))?;
        ledger.record(json!({ "step": "verified", "file": path, "bytes": meta.len(), "sha256": got, "records": records, "contracts": cov.symbols.len() }))?;
        let entry = json!({ "file": format!("{day}.dbn.zst"), "size": meta.len(), "sha256": got, "records": records, "coverage": cov.to_json(), "source": format!("import/{id}") });
        match held {
            None => steps.push((
                day,
                Step::Place {
                    from: path,
                    to: part.root.join(&rel),
                    entry,
                },
            )),
            Some((existing, _)) if hash_file(&existing)? == got => {
                steps.push((
                    day,
                    Step::Duplicate {
                        from: path,
                        existing,
                    },
                ));
            }
            Some((existing, None)) => {
                return Err(format!(
                    "{name}: {} holds the whole day already",
                    existing.display()
                ));
            }
            // Merged by an earlier run of this import that stopped before
            // removing its incoming copy.
            Some((existing, Some(e)))
                if e["source"] == format!("import/{id}").as_str()
                    && Coverage::from_json(&e["coverage"])
                        .is_some_and(|h| cov.symbols.is_subset(&h.symbols)) =>
            {
                steps.push((
                    day,
                    Step::Duplicate {
                        from: path,
                        existing,
                    },
                ));
            }
            Some((existing, Some(e))) => {
                let held_cov = Coverage::from_json(&e["coverage"])
                    .ok_or_else(|| format!("{name}: the held day's coverage is unreadable"))?;
                let both: Vec<&String> = held_cov.symbols.intersection(&cov.symbols).collect();
                if let Some(s) = both.first() {
                    return Err(format!(
                        "{name}: {} already holds {} of these contracts ({s}, …); only contracts not held are filed",
                        existing.display(),
                        both.len()
                    ));
                }
                steps.push((
                    day,
                    Step::Merge {
                        from: path,
                        existing,
                        held: held_cov,
                        new: cov,
                    },
                ));
            }
        }
        Ok(())
    };
    for f in files {
        if let Err(e) = check(f) {
            problems.push(e);
        }
    }
    if !problems.is_empty() {
        ledger.record(json!({ "step": "refused", "problems": problems }))?;
        return Err(problems.join("; "));
    }

    // Every check passed: file each day, never over anything held.
    let mut done = Vec::new();
    for (day, step) in steps {
        match step {
            Step::Place { from, to, entry } => {
                let dir = to.parent().ok_or("no folder")?.to_path_buf();
                // The manifest first: a crash before the move leaves an entry
                // for a missing file (filed again on a retry), never a
                // partial file taken for a whole day.
                set_entry(&dir, &day, Some(entry))?;
                if let Err(e) = place_new(&from, &to) {
                    let why = format!("{} -> {}: {e}", from.display(), to.display());
                    ledger.record(json!({ "step": "stopped", "error": why }))?;
                    return Err(why);
                }
                ledger.record(json!({ "step": "moved", "from": from, "to": to }))?;
                done.push(json!({ "day": day, "path": to, "filed": "new" }));
            }
            Step::Duplicate { from, existing } => {
                fs::remove_file(&from).map_err(|e| e.to_string())?;
                ledger.record(
                    json!({ "step": "duplicate-dropped", "file": from, "identical_to": existing }),
                )?;
                done.push(json!({ "day": day, "path": existing, "filed": "already present" }));
            }
            Step::Merge {
                from,
                existing,
                held,
                new,
            } => {
                let dir = existing.parent().ok_or("no folder")?.to_path_buf();
                // Written beside the day's file, on its volume, so the swap is a rename.
                let vol_root = everywhere
                    .iter()
                    .find(|r| existing.starts_with(r))
                    .ok_or("the held day isn't under a provider folder")?;
                let work = vol_root.join(INCOMING).join(id);
                fs::create_dir_all(&work).map_err(|e| e.to_string())?;
                let out = work.join(format!("merged-{day}.dbn.zst"));
                let _ = fs::remove_file(&out);
                let records = merge(&existing, &from, &out)?;
                let sha = hash_file(&out)?;
                let size = fs::metadata(&out).map_err(|e| e.to_string())?.len();
                let mut entry = read_manifest(&dir)?
                    .get(&day)
                    .cloned()
                    .ok_or("the held day's entry vanished")?;
                let union = held.union(&new);
                entry["pending"] = json!({ "file": format!("{day}.dbn.zst"), "size": size, "sha256": sha, "records": records, "coverage": union.to_json(), "source": format!("import/{id}") });
                set_entry(&dir, &day, Some(entry.clone()))?;
                ledger.record(json!({ "step": "merged", "inputs": [existing, from], "output": out, "sha256": sha, "records": records }))?;
                fs::rename(&out, &existing)
                    .map_err(|e| format!("{} -> {}: {e}", out.display(), existing.display()))?;
                sync_dir(&dir).map_err(|e| e.to_string())?;
                set_entry(&dir, &day, Some(entry["pending"].clone()))?;
                fs::remove_file(&from).map_err(|e| e.to_string())?;
                let _ = fs::remove_dir(&work);
                ledger.record(json!({ "step": "swapped", "path": existing, "sha256": sha }))?;
                done.push(json!({ "day": day, "path": existing, "filed": "merged", "contracts": union.symbols.len() }));
            }
        }
    }

    // The receipt goes to provenance on every volume involved.
    let text = fs::read(&receipt_path).map_err(|e| e.to_string())?;
    for root in &roots {
        let to = root.join("provenance").join(id).join(IMPORT);
        if to.exists() {
            if fs::read(&to).map_err(|e| e.to_string())? != text {
                return Err(format!(
                    "{}: provenance already holds a different {IMPORT}",
                    to.display()
                ));
            }
        } else {
            write_new(&to, &text).map_err(|e| format!("{}: {e}", to.display()))?;
        }
        ledger.record(json!({ "step": "receipt", "file": to }))?;
    }
    // Other receipts beside it (a provider batch's manifest and metadata) are
    // kept too, never over a different one.
    for part in parts {
        for e in fs::read_dir(&part.inc).into_iter().flatten().flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if name == IMPORT || !name.ends_with(".json") || !safe(&name) {
                continue;
            }
            let bytes = fs::read(e.path()).map_err(|e| e.to_string())?;
            let mut kept = true;
            for root in &roots {
                let to = root.join("provenance").join(id).join(&name);
                if !to.exists() {
                    write_new(&to, &bytes).map_err(|e| format!("{}: {e}", to.display()))?;
                    ledger.record(json!({ "step": "receipt", "file": to }))?;
                } else if fs::read(&to).map_err(|e| e.to_string())? != bytes {
                    kept = false;
                }
            }
            if kept {
                fs::remove_file(e.path()).map_err(|e| e.to_string())?;
            }
        }
    }
    let mut left = Vec::new();
    for part in parts {
        let _ = fs::remove_file(part.inc.join(IMPORT));
        left.extend(
            fs::read_dir(&part.inc)
                .into_iter()
                .flatten()
                .flatten()
                .map(|e| part.inc.join(e.file_name()).display().to_string()),
        );
        let _ = fs::remove_dir(&part.inc);
        let _ = fs::remove_dir(part.root.join(INCOMING));
    }
    let summary = json!({
        "import": id, "dataset": dataset, "schema": schema, "symbol": parent,
        "days": done, "left_behind": left, "completed_at": tinytrader_observe::unix_seconds(),
    });
    let text = serde_json::to_string_pretty(&summary).map_err(|e| e.to_string())?;
    for root in &roots {
        let mut out = root
            .join("migrations")
            .join(format!("{today}-import-{id}.json"));
        if out.exists() {
            out = root.join("migrations").join(format!(
                "{today}-import-{id}-{}.json",
                tinytrader_observe::unix_seconds() as u64
            ));
        }
        write_new(&out, text.as_bytes()).map_err(|e| format!("{}: {e}", out.display()))?;
    }
    ledger.record(json!({ "step": "complete" }))?;
    Ok(summary)
}

/// File a provider batch bought as a list of contracts of `parent` (such as
/// `SPY.OPT`), waiting in `.incoming/<batch>` with the provider's
/// `manifest.json` and `metadata.json`: as an import of its day files, each a
/// partial day, with those receipts kept beside it.
///
/// # Errors
/// If the batch isn't a daily DBN batch requested by contract
/// (`stype_in = raw_symbol`), or as [`import`].
pub fn import_batch(
    parts: &[Part],
    elsewhere: &[PathBuf],
    batch: &str,
    parent: &str,
    today: &str,
) -> Result<Value, String> {
    let read = |name: &str| -> Result<(Value, &Part), String> {
        let part = parts
            .iter()
            .find(|p| p.inc.join(name).exists())
            .ok_or_else(|| format!("{name}: missing"))?;
        let text = fs::read_to_string(part.inc.join(name)).map_err(|e| format!("{name}: {e}"))?;
        Ok((
            serde_json::from_str(&text).map_err(|e| format!("{name}: {e}"))?,
            part,
        ))
    };
    if parts.iter().any(|p| p.inc.join(IMPORT).exists()) {
        // Written by an earlier, interrupted run: file it as it says.
        return import(parts, elsewhere, batch, today);
    }
    let (manifest, home) = read("manifest.json")?;
    let (metadata, _) = read("metadata.json")?;
    let q = &metadata["query"];
    let (dataset, schema) = (
        q["dataset"]
            .as_str()
            .ok_or("metadata.json has no dataset")?,
        q["schema"].as_str().ok_or("metadata.json has no schema")?,
    );
    if q["encoding"] != "dbn" || q["compression"] != "zstd" {
        return Err("only DBN batches compressed with zstd are filed".into());
    }
    let c = &metadata["customizations"];
    if c["split_symbols"] == true || c["split_duration"] != "day" {
        return Err("only batches split by day, and not by symbol, are filed".into());
    }
    if q["stype_in"] != "raw_symbol" {
        return Err("only batches requested by contract (stype_in raw_symbol) are filed as partial days; file others with /incorporate without a parent".into());
    }
    let files: Vec<Value> = manifest["files"]
        .as_array()
        .ok_or("manifest.json lists no files")?
        .iter()
        .filter_map(|f| {
            let name = f["filename"].as_str()?;
            let day = ingest::databento_day(name, schema)?;
            Some(json!({ "filename": name, "day": day, "size": f["size"], "hash": f["hash"] }))
        })
        .collect();
    let receipt = json!({
        "dataset": dataset, "schema": schema, "symbol": parent,
        "source": { "provider_batch": batch, "query": q },
        "files": files,
    });
    let text = serde_json::to_string_pretty(&receipt).map_err(|e| e.to_string())?;
    write_new(&home.inc.join(IMPORT), text.as_bytes()).map_err(|e| format!("{IMPORT}: {e}"))?;
    import(parts, elsewhere, batch, today)
}

/// What one copy of a day holds, as `/coverage` reports it.
fn row(day: &str, volume: &str, filed: bool, size: u64, entry: Option<&Value>) -> Value {
    match entry {
        None => {
            json!({ "day": day, "volume": volume, "kind": "complete", "filed": filed, "size": size })
        }
        Some(m) => json!({
            "day": day, "volume": volume, "kind": "partial", "filed": filed, "size": size,
            "coverage": m["coverage"], "records": m["records"], "sha256": m["sha256"],
            "pending": m.get("pending").is_some(),
        }),
    }
}

/// Does data of `found` (a provider, or none outside every provider's
/// folder) count for `provider`? Data with no provider counts for Databento,
/// whose layout and names it has: a copy in a backup still blocks buying it
/// again.
fn ours(found: Option<&str>, provider: &str) -> bool {
    found.unwrap_or("databento") == provider
}

/// Symbol types whose symbols say what they hold.
const CLASSIFIED: [&str; 5] = [
    "parent",
    "raw_symbol",
    "continuous",
    "nasdaq_symbol",
    "cms_symbol",
];

/// What a holding of `symbols`, requested as `stype_in`, holds of `symbol`
/// (or, with none, of the dataset): `None` nothing, an empty set all of it,
/// otherwise those of its contracts the holding lists. Symbols that can't be
/// told apart (instrument IDs, or no type) hold all of it, to be safe.
fn holds(
    symbols: &[String],
    stype_in: Option<&str>,
    symbol: Option<&str>,
) -> Option<BTreeSet<String>> {
    if symbols.is_empty() || !stype_in.is_some_and(|t| CLASSIFIED.contains(&t)) {
        return Some(BTreeSet::new());
    }
    let all = symbols
        .iter()
        .any(|s| s.eq_ignore_ascii_case("ALL_SYMBOLS"));
    let whole = match symbol {
        Some(s) => all || symbols.iter().any(|x| x.eq_ignore_ascii_case(s)),
        None => all || symbols.len() != 1,
    };
    if whole {
        return Some(BTreeSet::new());
    }
    // A list of contracts of the parent holds those contracts.
    let contracts: BTreeSet<String> = match symbol {
        Some(s) if stype_in == Some("raw_symbol") => symbols
            .iter()
            .filter(|c| of_parent(s, c))
            .cloned()
            .collect(),
        _ => BTreeSet::new(),
    };
    (!contracts.is_empty()).then_some(contracts)
}

/// An unfiled copy of a day: complete when `contracts` is empty, otherwise
/// partial with those contracts over the part of `window` in the day.
fn held_row(
    day: &str,
    volume: &str,
    size: u64,
    contracts: &BTreeSet<String>,
    window: (u64, u64),
) -> Value {
    let partial = (!contracts.is_empty()).then(|| {
        let from = ingest::day_ns(day).unwrap_or(0);
        let held = Coverage {
            symbols: contracts.clone(),
            start: window.0.max(from),
            end: window.1.min(from + 86_400_000_000_000),
        };
        json!({ "coverage": held.to_json(), "records": null, "sha256": null })
    });
    row(day, volume, false, size, partial.as_ref())
}

/// Is `symbol` a contract of the option parent `parent` (`ROOT.OPT`)?
fn of_parent(parent: &str, symbol: &str) -> bool {
    parent.strip_suffix(".OPT").is_some_and(|root| {
        symbol.len() == 21
            && symbol
                .get(..6)
                .is_some_and(|r| r.trim_end().eq_ignore_ascii_case(root))
    })
}

/// Each held day of `provider`'s `dataset/symbol/schema` from `start` to
/// `end` (inclusive, `YYYY-MM-DD`), on any of `volumes` (name, path) that is
/// present: every day the catalog counts, filed or not. A day held on several
/// volumes, or in several forms, is one row: complete if any copy is,
/// otherwise the union of their contracts and windows, with `conflict` when
/// the copies differ.
///
/// # Errors
/// If a folder of the provider or the dataset can't be read, or a manifest
/// or a batch's metadata is malformed: never an answer with days left out.
#[cfg(test)]
pub fn coverage(
    volumes: &[(String, PathBuf)],
    provider: &str,
    dataset: &str,
    symbol: Option<&str>,
    schema: &str,
    start: &str,
    end: &str,
) -> Result<Vec<Value>, String> {
    let mut found = Vec::new();
    for (volume, root) in volumes.iter().filter(|(_, r)| r.is_dir()) {
        found.push((volume.as_str(), crate::catalog::places(root)?));
    }
    let index: Vec<(&str, &[crate::catalog::Place])> =
        found.iter().map(|(v, p)| (*v, p.as_slice())).collect();
    coverage_in(&index, provider, dataset, symbol, schema, start, end)
}

/// As [`coverage`], from the places a scan found on each volume (`index`),
/// and the current manifests of the matching schema folders, so a merge into
/// a partial day shows at once.
///
/// # Errors
/// If a matching schema folder's manifest is malformed.
pub fn coverage_in(
    index: &[(&str, &[crate::catalog::Place])],
    provider: &str,
    dataset: &str,
    symbol: Option<&str>,
    schema: &str,
    start: &str,
    end: &str,
) -> Result<Vec<Value>, String> {
    let mut copies: BTreeMap<String, Vec<Value>> = BTreeMap::new();
    let wanted = |day: &str| day >= start && day <= end;
    for &(volume, places) in index {
        for place in places {
            match place {
                crate::catalog::Place::Days {
                    provider: p,
                    dataset: d,
                    symbol: s,
                    schema: k,
                    dir,
                    days,
                } if ours(p.as_deref(), provider)
                    && d.eq_ignore_ascii_case(dataset)
                    && s.as_deref() == symbol
                    && k == schema =>
                {
                    let manifest = read_manifest(dir)?;
                    for (day, size, filed) in days.iter().filter(|d| wanted(&d.0)) {
                        let entry = if *filed { manifest.get(day) } else { None };
                        let r = row(day, volume, *filed, *size, entry);
                        copies.entry(day.clone()).or_default().push(r);
                    }
                }
                crate::catalog::Place::Batch {
                    provider: p,
                    batch: b,
                } if ours(p.as_deref(), provider) && b.dataset.eq_ignore_ascii_case(dataset) => {
                    let Some(contracts) = holds(&b.symbols, b.stype_in.as_deref(), symbol) else {
                        continue;
                    };
                    for (day, _, _, size) in b.files.iter().filter(|f| f.1 == schema) {
                        if wanted(day) {
                            let r = held_row(day, volume, *size, &contracts, (0, u64::MAX));
                            copies.entry(day.clone()).or_default().push(r);
                        }
                    }
                }
                // Any other DBN file, by its header; one of several schemas
                // counts for each.
                crate::catalog::Place::Loose {
                    provider: p,
                    header: h,
                    size,
                } if ours(p.as_deref(), provider)
                    && h.dataset.eq_ignore_ascii_case(dataset)
                    && h.schema.as_deref().is_none_or(|s| s == schema) =>
                {
                    let Some(contracts) = holds(&h.symbols, h.stype_in.as_deref(), symbol) else {
                        continue;
                    };
                    for (i, day) in h.days().into_iter().enumerate() {
                        if wanted(&day) {
                            let size = if i == 0 { *size } else { 0 };
                            let r = held_row(&day, volume, size, &contracts, (h.start, h.end));
                            copies.entry(day).or_default().push(r);
                        }
                    }
                }
                _ => {}
            }
        }
    }
    Ok(copies.into_values().map(merge_copies).collect())
}

/// One day's row from its copies: a complete copy if there is one, otherwise
/// the union of the partial copies' contracts and windows.
fn merge_copies(copies: Vec<Value>) -> Value {
    let same = |a: &Value, b: &Value| {
        ["kind", "coverage", "size", "sha256"]
            .iter()
            .all(|k| a.get(*k) == b.get(*k))
    };
    let conflict = copies.windows(2).any(|w| !same(&w[0], &w[1]));
    let mut volumes: Vec<&str> = copies.iter().filter_map(|c| c["volume"].as_str()).collect();
    volumes.sort_unstable();
    volumes.dedup();
    let filed = copies.iter().any(|c| c["filed"] == true);
    let mut out = match copies.iter().find(|c| c["kind"] == "complete") {
        Some(c) => c.clone(),
        None => {
            let mut out = copies[0].clone();
            if conflict {
                let union = copies
                    .iter()
                    .filter_map(|c| Coverage::from_json(&c["coverage"]))
                    .reduce(|a, b| a.union(&b));
                out["coverage"] = union.map_or(Value::Null, |u| u.to_json());
                out["pending"] = json!(copies.iter().any(|c| c["pending"] == true));
                for k in ["size", "sha256", "records"] {
                    out[k] = Value::Null;
                }
            }
            out
        }
    };
    out["filed"] = json!(filed);
    out["volumes"] = json!(volumes);
    out["conflict"] = json!(conflict);
    out
}

#[cfg(test)]
mod tests {
    use std::num::NonZeroU64;

    use dbn::{CbboMsg, MappingInterval, Schema, SymbolMapping};

    use super::*;

    const DAY: &str = "2026-10-02";
    const C1: &str = "SPY   261002C00728000";
    const C2: &str = "SPY   261002P00728000";
    const C3: &str = "SPY   261002C00729000";

    fn scratch(name: &str) -> PathBuf {
        let d =
            std::env::temp_dir().join(format!("tinytrader-partial-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    fn at(minute: u64) -> u64 {
        ingest::day_ns(DAY).unwrap() + (13 * 60 + 30 + minute) * 60_000_000_000
    }

    /// A partial cbbo-1m day: `contracts` (symbol, instrument ID) listed and
    /// mapped, and one record per (instrument, minute) in `records`.
    fn day_bytes(contracts: &[(&str, u32)], records: &[(u32, u64)]) -> Vec<u8> {
        let start = ingest::day_ns(DAY).unwrap();
        let date = time::Date::parse(
            DAY,
            time::macros::format_description!("[year]-[month]-[day]"),
        )
        .unwrap();
        let m = Metadata::builder()
            .dataset("OPRA.PILLAR")
            .schema(Some(Schema::Cbbo1M))
            .start(start + 13 * 3_600_000_000_000)
            .end(NonZeroU64::new(start + 24 * 3_600_000_000_000))
            .stype_in(Some(SType::RawSymbol))
            .stype_out(SType::InstrumentId)
            .symbols(contracts.iter().map(|(s, _)| (*s).to_owned()).collect())
            .mappings(
                contracts
                    .iter()
                    .map(|(s, id)| SymbolMapping {
                        raw_symbol: (*s).to_owned(),
                        intervals: vec![MappingInterval {
                            start_date: date,
                            end_date: date.next_day().unwrap(),
                            symbol: id.to_string(),
                        }],
                    })
                    .collect(),
            )
            .build();
        let mut out = Vec::new();
        {
            let mut enc = dbn::encode::dbn::Encoder::with_zstd(&mut out, &m).unwrap();
            for (id, minute) in records {
                let mut r = CbboMsg::default_for_schema(Schema::Cbbo1M);
                r.hd.instrument_id = *id;
                r.hd.ts_event = at(*minute);
                r.ts_recv = at(*minute);
                r.price = 1_000_000_000 + i64::from(*id);
                enc.encode_record(&r).unwrap();
            }
            enc.flush().unwrap();
        }
        out
    }

    /// An import waiting in `<root>/.incoming/<id>/`: its files and receipt.
    fn stage(root: &Path, id: &str, files: &[(&str, Vec<u8>)]) -> Part {
        let inc = root.join(INCOMING).join(id);
        fs::create_dir_all(&inc).unwrap();
        let mut listed = Vec::new();
        for (name, bytes) in files {
            fs::write(inc.join(name), bytes).unwrap();
            listed.push(json!({ "filename": name, "size": bytes.len(), "hash": format!("sha256:{}", hash_file(&inc.join(name)).unwrap()) }));
        }
        let receipt = json!({
            "dataset": "OPRA.PILLAR", "schema": "cbbo-1m", "symbol": "SPY.OPT",
            "source": { "api": "timeseries.get_range", "tool": "research/cbbo_fetch.py", "commit": "61f984a" },
            "files": listed,
        });
        fs::write(inc.join(IMPORT), receipt.to_string()).unwrap();
        Part {
            inc,
            root: root.to_path_buf(),
        }
    }

    fn file(root: &Path) -> PathBuf {
        root.join("OPRA.PILLAR/SPY.OPT/cbbo-1m")
            .join(format!("{DAY}.dbn.zst"))
    }

    fn name() -> String {
        format!("{DAY}.dbn.zst")
    }

    #[test]
    fn files_a_partial_day_and_reports_its_coverage() {
        let root = scratch("place").join("Databento");
        let bytes = day_bytes(&[(C1, 11), (C2, 12)], &[(11, 0), (12, 0), (11, 1)]);
        let part = stage(&root, "I1", &[(&name(), bytes.clone())]);
        let summary = import(std::slice::from_ref(&part), &[], "I1", "2026-10-19").unwrap();
        assert_eq!(summary["days"][0]["filed"], "new");
        assert_eq!(
            fs::read(file(&root)).unwrap(),
            bytes,
            "filed as uploaded, byte for byte"
        );
        assert!(!part.inc.exists(), "the incoming folder is gone");
        assert!(root.join("provenance/I1").join(IMPORT).exists());
        let days = coverage(
            &[("v".into(), root.parent().unwrap().to_path_buf())],
            "databento",
            "OPRA.PILLAR",
            Some("SPY.OPT"),
            "cbbo-1m",
            DAY,
            DAY,
        )
        .unwrap();
        assert_eq!(days.len(), 1);
        assert_eq!(days[0]["kind"], "partial");
        assert_eq!(days[0]["records"], 3);
        assert_eq!(days[0]["coverage"]["symbols"], json!([C1, C2]));
        // The catalog marks it partial.
        let cat = crate::catalog::scan_root(
            "v",
            root.parent().unwrap(),
            crate::catalog::Thresholds {
                warning: 100.0,
                critical: 100.0,
                floor: 100.0,
            },
        );
        let ds = cat["datasets"]
            .as_array()
            .unwrap()
            .iter()
            .find(|d| d["symbol"] == "SPY.OPT")
            .unwrap();
        assert_eq!(ds["schemas"]["cbbo-1m"]["partial"], json!([DAY]));
        // A day with no manifest entry is complete.
        let other = root.join("OPRA.PILLAR/SPY.OPT/cbbo-1m/2026-10-01.dbn.zst");
        fs::write(&other, b"x").unwrap();
        let days = coverage(
            &[("v".into(), root.parent().unwrap().to_path_buf())],
            "databento",
            "OPRA.PILLAR",
            Some("SPY.OPT"),
            "cbbo-1m",
            "2026-10-01",
            DAY,
        )
        .unwrap();
        assert_eq!(days[0]["kind"], "complete");
        // Filing it again finds it already there.
        let again = stage(&root, "I1b", &[(&name(), bytes)]);
        let s = import(&[again], &[], "I1b", "2026-10-19").unwrap();
        assert_eq!(s["days"][0]["filed"], "already present");
    }

    #[test]
    fn refuses_records_outside_its_contracts_or_window_and_moves_nothing() {
        let root = scratch("refuse").join("Databento");
        // Instrument 99 maps to no listed contract.
        let stray = day_bytes(&[(C1, 11)], &[(11, 0), (99, 1)]);
        let part = stage(&root, "I2", &[(&name(), stray)]);
        let e = import(std::slice::from_ref(&part), &[], "I2", "2026-10-19").unwrap_err();
        assert!(e.contains("maps to none of its contracts"), "{e}");
        assert!(part.inc.join(name()).exists() && !file(&root).exists());
        // A record before the window (13:00 UTC) is refused too.
        let path = root.join("early.dbn.zst");
        fs::write(
            &path,
            day_bytes_at(&[(C1, 11)], &[(11, 12 * 60 * 60_000_000_000)]),
        )
        .unwrap();
        assert!(inspect(&path).unwrap_err().contains("outside its window"));
        // A contract of another root doesn't belong to SPY.OPT.
        assert!(
            belongs(
                "SPY.OPT",
                &BTreeSet::from(["QQQ   261002C00500000".to_owned()])
            )
            .is_err()
        );
        assert!(belongs("SPY", &BTreeSet::from([C1.to_owned()])).is_err());
    }

    /// As `day_bytes`, with one record at an absolute offset from midnight.
    fn day_bytes_at(contracts: &[(&str, u32)], records: &[(u32, u64)]) -> Vec<u8> {
        let start = ingest::day_ns(DAY).unwrap();
        let bytes = day_bytes(contracts, &[]);
        let (m, _) = {
            let p = std::env::temp_dir().join(format!(
                "tinytrader-partial-at-{}.dbn.zst",
                std::process::id()
            ));
            fs::write(&p, &bytes).unwrap();
            let r = read_all(&p).unwrap();
            let _ = fs::remove_file(&p);
            r
        };
        let mut out = Vec::new();
        {
            let mut enc = dbn::encode::dbn::Encoder::with_zstd(&mut out, &m).unwrap();
            for &(id, off) in records {
                let mut r = CbboMsg::default_for_schema(Schema::Cbbo1M);
                r.hd.instrument_id = id;
                r.hd.ts_event = start + off;
                r.ts_recv = start + off;
                enc.encode_record(&r).unwrap();
            }
            enc.flush().unwrap();
        }
        out
    }

    #[test]
    fn merges_other_contracts_into_a_partial_day() {
        let root = scratch("merge").join("Databento");
        let a = day_bytes(&[(C1, 11)], &[(11, 0), (11, 2), (11, 4)]);
        import(
            &[stage(&root, "A", &[(&name(), a)])],
            &[],
            "A",
            "2026-10-19",
        )
        .unwrap();
        let b = day_bytes(&[(C2, 12), (C3, 13)], &[(12, 1), (13, 2), (12, 3)]);
        let s = import(
            &[stage(&root, "B", &[(&name(), b)])],
            &[],
            "B",
            "2026-10-19",
        )
        .unwrap();
        assert_eq!(s["days"][0]["filed"], "merged");
        assert_eq!(s["days"][0]["contracts"], 3);
        // Six records, in time order (ties: the held day's first), each check passing.
        let (cov, n) = inspect(&file(&root)).unwrap();
        assert_eq!(n, 6);
        assert_eq!(
            cov.symbols,
            BTreeSet::from([C1.to_owned(), C2.to_owned(), C3.to_owned()])
        );
        let (_, recs) = read_all(&file(&root)).unwrap();
        let order: Vec<u32> = recs.iter().map(|r| r.header().instrument_id).collect();
        assert_eq!(order, vec![11, 12, 11, 13, 12, 11]);
        let entry = read_manifest(file(&root).parent().unwrap()).unwrap()[DAY].clone();
        assert!(entry.get("pending").is_none());
        assert_eq!(entry["sha256"], hash_file(&file(&root)).unwrap().as_str());
        assert_eq!(entry["records"], 6);
        assert!(!root.join(INCOMING).exists(), "nothing left in incoming");

        // Contracts already held are never filed twice.
        let c = day_bytes(&[(C2, 12)], &[(12, 5)]);
        let e = import(
            &[stage(&root, "C", &[(&name(), c)])],
            &[],
            "C",
            "2026-10-19",
        )
        .unwrap_err();
        assert!(e.contains("already holds 1 of these contracts"), "{e}");
    }

    #[test]
    fn refuses_a_partial_day_where_the_whole_day_is_held() {
        let root = scratch("whole").join("Databento");
        fs::create_dir_all(file(&root).parent().unwrap()).unwrap();
        fs::write(file(&root), b"whole day").unwrap();
        let a = day_bytes(&[(C1, 11)], &[(11, 0)]);
        let e = import(
            &[stage(&root, "W", &[(&name(), a)])],
            &[],
            "W",
            "2026-10-19",
        )
        .unwrap_err();
        assert!(e.contains("holds the whole day"), "{e}");
        assert_eq!(fs::read(file(&root)).unwrap(), b"whole day");
    }

    #[test]
    fn checks_the_held_day_on_another_volume() {
        let base = scratch("volumes");
        let (one, two) = (base.join("one/Databento"), base.join("two/Databento"));
        let a = day_bytes(&[(C1, 11)], &[(11, 0)]);
        import(&[stage(&one, "A", &[(&name(), a)])], &[], "A", "2026-10-19").unwrap();
        // The same contract, arriving on the other volume, is refused.
        let again = day_bytes(&[(C1, 11)], &[(11, 1)]);
        let e = import(
            &[stage(&two, "B", &[(&name(), again)])],
            std::slice::from_ref(&one),
            "B",
            "2026-10-19",
        )
        .unwrap_err();
        assert!(e.contains("already holds"), "{e}");
        // Other contracts merge into the held day, on its own volume.
        let other = day_bytes(&[(C2, 12)], &[(12, 1)]);
        let s = import(
            &[stage(&two, "C", &[(&name(), other)])],
            std::slice::from_ref(&one),
            "C",
            "2026-10-19",
        )
        .unwrap();
        assert_eq!(s["days"][0]["filed"], "merged");
        assert_eq!(inspect(&file(&one)).unwrap().1, 2);
        assert!(!file(&two).exists());
    }

    #[test]
    fn settles_an_interrupted_merge_from_the_files_hash() {
        let root = scratch("settle").join("Databento");
        let a = day_bytes(&[(C1, 11)], &[(11, 0)]);
        import(
            &[stage(&root, "A", &[(&name(), a.clone())])],
            &[],
            "A",
            "2026-10-19",
        )
        .unwrap();
        let dir = file(&root).parent().unwrap().to_path_buf();
        let held = hash_file(&file(&root)).unwrap();
        // Stopped after recording the pending merge, before the swap: dropped.
        let mut e = read_manifest(&dir).unwrap()[DAY].clone();
        e["pending"] = json!({ "sha256": "f00d", "coverage": {} });
        set_entry(&dir, DAY, Some(e)).unwrap();
        let now = settle(&dir, DAY).unwrap().unwrap();
        assert!(now.get("pending").is_none() && now["sha256"] == held.as_str());
        // Stopped after the swap: the pending entry becomes current.
        let merged = day_bytes(&[(C1, 11), (C2, 12)], &[(11, 0), (12, 1)]);
        let mut e = read_manifest(&dir).unwrap()[DAY].clone();
        fs::write(file(&root), &merged).unwrap();
        let sha = hash_file(&file(&root)).unwrap();
        e["pending"] = json!({ "sha256": sha, "records": 2, "coverage": { "kind": "contracts", "symbols": [C1, C2], "window": [0, 1] } });
        set_entry(&dir, DAY, Some(e)).unwrap();
        let now = settle(&dir, DAY).unwrap().unwrap();
        assert_eq!(now["records"], 2);
        assert!(now.get("pending").is_none());
        // Neither: refused, for a person to look at.
        fs::write(file(&root), b"something else").unwrap();
        let mut e = read_manifest(&dir).unwrap()[DAY].clone();
        e["pending"] = json!({ "sha256": "f00d" });
        set_entry(&dir, DAY, Some(e)).unwrap();
        assert!(settle(&dir, DAY).is_err());
    }

    #[test]
    fn files_a_contract_list_batch_as_partial_days() {
        let root = scratch("batch").join("Databento");
        let inc = root.join(INCOMING).join("OPRA-X");
        fs::create_dir_all(&inc).unwrap();
        let name = "opra-pillar-20261002.cbbo-1m.dbn.zst";
        let bytes = day_bytes(&[(C1, 11), (C2, 12)], &[(11, 0), (12, 1)]);
        fs::write(inc.join(name), &bytes).unwrap();
        let sha = hash_file(&inc.join(name)).unwrap();
        fs::write(inc.join("manifest.json"), json!({ "files": [{ "filename": name, "size": bytes.len(), "hash": format!("sha256:{sha}") }] }).to_string()).unwrap();
        let metadata = |stype: &str| {
            json!({ "query": { "dataset": "OPRA.PILLAR", "schema": "cbbo-1m", "stype_in": stype, "symbols": [C1, C2], "encoding": "dbn", "compression": "zstd" }, "customizations": { "split_duration": "day", "split_symbols": false } }).to_string()
        };
        let part = Part {
            inc: inc.clone(),
            root: root.clone(),
        };
        // A whole-symbol batch isn't filed this way.
        fs::write(inc.join("metadata.json"), metadata("parent")).unwrap();
        assert!(
            import_batch(
                std::slice::from_ref(&part),
                &[],
                "OPRA-X",
                "SPY.OPT",
                "2026-10-19"
            )
            .unwrap_err()
            .contains("raw_symbol")
        );
        fs::remove_file(inc.join("metadata.json")).unwrap();
        fs::write(inc.join("metadata.json"), metadata("raw_symbol")).unwrap();
        let s = import_batch(&[part], &[], "OPRA-X", "SPY.OPT", "2026-10-19").unwrap();
        assert_eq!(s["days"][0]["day"], DAY);
        assert_eq!(fs::read(file(&root)).unwrap(), bytes);
        for r in [IMPORT, "manifest.json", "metadata.json"] {
            assert!(root.join("provenance/OPRA-X").join(r).exists(), "{r} kept");
        }
        assert!(!inc.exists());
    }

    #[test]
    fn refuses_an_import_without_its_source() {
        let root = scratch("source").join("Databento");
        let part = stage(&root, "S", &[(&name(), day_bytes(&[(C1, 11)], &[(11, 0)]))]);
        let mut r: Value =
            serde_json::from_str(&fs::read_to_string(part.inc.join(IMPORT)).unwrap()).unwrap();
        r["source"] = json!({});
        fs::write(part.inc.join(IMPORT), r.to_string()).unwrap();
        assert!(
            import(&[part], &[], "S", "2026-10-19")
                .unwrap_err()
                .contains("where the data came from")
        );
    }

    /// `coverage` of OPRA.PILLAR SPY.OPT cbbo-1m over these volumes, for a
    /// provider, from 2026-10-01 to 2026-10-09.
    fn spy(volumes: &[(&str, &Path)], provider: &str) -> Result<Vec<Value>, String> {
        let v: Vec<(String, PathBuf)> = volumes
            .iter()
            .map(|(n, p)| ((*n).to_owned(), p.to_path_buf()))
            .collect();
        coverage(
            &v,
            provider,
            "OPRA.PILLAR",
            Some("SPY.OPT"),
            "cbbo-1m",
            "2026-10-01",
            "2026-10-09",
        )
    }

    fn put(path: &Path, bytes: &[u8]) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, bytes).unwrap();
    }

    const T: crate::catalog::Thresholds = crate::catalog::Thresholds {
        warning: 100.0,
        critical: 100.0,
        floor: 100.0,
    };

    #[test]
    fn keeps_providers_apart() {
        let vol = scratch("providers");
        put(
            &vol.join("Databento/OPRA.PILLAR/SPY.OPT/cbbo-1m/2026-10-01.dbn.zst"),
            b"a",
        );
        put(
            &vol.join("alpaca/OPRA.PILLAR/SPY.OPT/cbbo-1m/2026-10-02.dbn.zst"),
            b"b",
        );
        let cat = crate::catalog::scan_root("v", &vol, T);
        let found: Vec<(Value, Value, Value)> = cat["datasets"]
            .as_array()
            .unwrap()
            .iter()
            .map(|d| {
                (
                    d["provider"].clone(),
                    d["filed"].clone(),
                    d["schemas"]["cbbo-1m"]["days"].clone(),
                )
            })
            .collect();
        assert_eq!(
            found,
            [
                (json!("alpaca"), json!(true), json!({ "2026-10-02": 1 })),
                (json!("databento"), json!(true), json!({ "2026-10-01": 1 })),
            ]
        );
        let days = |p: &str| -> Vec<Value> {
            spy(&[("v", &vol)], p)
                .unwrap()
                .iter()
                .map(|d| d["day"].clone())
                .collect()
        };
        assert_eq!(days("databento"), [json!("2026-10-01")]);
        assert_eq!(days("alpaca"), [json!("2026-10-02")]);
    }

    #[test]
    fn coverage_counts_batch_folders_and_the_older_layout() {
        let vol = scratch("unfiled");
        let batch = |dir: PathBuf, stype: &str, symbols: &[&str], day: &str| {
            put(&dir.join("manifest.json"), b"{}");
            let meta = json!({ "job_id": file_name_of(&dir), "query": { "dataset": "OPRA.PILLAR", "stype_in": stype, "symbols": symbols } });
            put(&dir.join("metadata.json"), meta.to_string().as_bytes());
            put(
                &dir.join(format!("opra-pillar-{day}.cbbo-1m.dbn.zst")),
                b"batch",
            );
        };
        // In Databento's folder, and outside any provider's folder.
        batch(
            vol.join("Databento/downloads/OPRA-A"),
            "parent",
            &["SPY.OPT"],
            "20261003",
        );
        batch(
            vol.join("OPRA-B"),
            "parent",
            &["QQQ.OPT", "SPY.OPT"],
            "20261005",
        );
        batch(
            vol.join("Databento/OPRA-C"),
            "raw_symbol",
            &[C1, "QQQ   261002C00500000"],
            "20261006",
        );
        batch(
            vol.join("alpaca/OPRA-D"),
            "parent",
            &["SPY.OPT"],
            "20261007",
        );
        put(
            &vol.join("Databento/OPRA.PILLAR/SPY.OPT/cbbo-1m/2026-10-04/x.dbn.zst"),
            b"older",
        );
        put(
            &vol.join("Databento/OPRA.PILLAR/SPY.OPT/cbbo-1m/2026-10-08.dbn.zst"),
            b"filed",
        );
        let days = spy(&[("v", &vol)], "databento").unwrap();
        let brief: Vec<(&str, &str, bool)> = days
            .iter()
            .map(|d| {
                (
                    d["day"].as_str().unwrap(),
                    d["kind"].as_str().unwrap(),
                    d["filed"] == true,
                )
            })
            .collect();
        assert_eq!(
            brief,
            [
                ("2026-10-03", "complete", false),
                ("2026-10-04", "complete", false),
                ("2026-10-05", "complete", false),
                ("2026-10-06", "partial", false),
                ("2026-10-08", "complete", true),
            ]
        );
        assert_eq!(
            days[3]["coverage"]["symbols"],
            json!([C1]),
            "only SPY's contracts"
        );
        assert_eq!(days[0]["volumes"], json!(["v"]));
        // The catalog counts the same days, as unfiled.
        let cat = crate::catalog::scan_root("v", &vol, T);
        let ds = cat["datasets"]
            .as_array()
            .unwrap()
            .iter()
            .find(|d| d["provider"] == "databento" && d["symbol"] == "SPY.OPT")
            .unwrap();
        assert_eq!(ds["filed"], false);
        assert_eq!(
            ds["schemas"]["cbbo-1m"]["unfiled"],
            json!(["2026-10-04", "2026-10-03"])
        );
    }

    fn file_name_of(p: &Path) -> String {
        p.file_name().unwrap().to_string_lossy().into_owned()
    }

    #[test]
    fn merges_a_day_held_on_several_volumes() {
        let base = scratch("merge-volumes");
        let vols: Vec<PathBuf> = ["one", "two", "three"]
            .iter()
            .map(|n| base.join(n))
            .collect();
        // Different contracts of the same day, filed on two volumes apart.
        let a = day_bytes(&[(C1, 11)], &[(11, 0)]);
        import(
            &[stage(
                &vols[0].join("Databento"),
                "A",
                &[(&name(), a.clone())],
            )],
            &[],
            "A",
            "2026-10-19",
        )
        .unwrap();
        let b = day_bytes(&[(C2, 12)], &[(12, 1)]);
        import(
            &[stage(&vols[1].join("Databento"), "B", &[(&name(), b)])],
            &[],
            "B",
            "2026-10-19",
        )
        .unwrap();
        let both = spy(&[("one", &vols[0]), ("two", &vols[1])], "databento").unwrap();
        assert_eq!(both.len(), 1);
        assert_eq!(both[0]["kind"], "partial");
        assert_eq!(both[0]["coverage"]["symbols"], json!([C1, C2]));
        assert_eq!(both[0]["volumes"], json!(["one", "two"]));
        assert_eq!(both[0]["conflict"], true);
        assert_eq!(both[0]["sha256"], Value::Null, "no one copy's hash");
        // Identical copies don't conflict.
        import(
            &[stage(&vols[2].join("Databento"), "A2", &[(&name(), a)])],
            &[],
            "A2",
            "2026-10-19",
        )
        .unwrap();
        let same = spy(&[("one", &vols[0]), ("three", &vols[2])], "databento").unwrap();
        assert_eq!(
            (same[0]["conflict"].clone(), same[0]["kind"].clone()),
            (json!(false), json!("partial"))
        );
        // A complete copy wins, whichever volume is read first.
        put(&file(&vols[2].join("Databento")), b"the whole day");
        fs::remove_file(
            vols[2]
                .join("Databento/OPRA.PILLAR/SPY.OPT/cbbo-1m")
                .join(MANIFEST),
        )
        .unwrap();
        let all = spy(
            &[("one", &vols[0]), ("two", &vols[1]), ("three", &vols[2])],
            "databento",
        )
        .unwrap();
        assert_eq!(all[0]["kind"], "complete");
        assert_eq!(all[0]["volumes"], json!(["one", "three", "two"]));
        assert_eq!(all[0]["conflict"], true);
    }

    #[test]
    fn fails_closed_on_a_malformed_manifest_or_an_unreadable_folder() {
        use std::os::unix::fs::PermissionsExt as _;
        let vol = scratch("closed");
        let dir = vol.join("Databento/OPRA.PILLAR/SPY.OPT/cbbo-1m");
        put(&dir.join("2026-10-01.dbn.zst"), b"a");
        put(&dir.join(MANIFEST), b"{ not json");
        assert!(
            spy(&[("v", &vol)], "databento")
                .unwrap_err()
                .contains(MANIFEST)
        );
        let cat = crate::catalog::scan_root("v", &vol, T);
        assert_eq!(cat["errors"].as_array().unwrap().len(), 1, "{cat}");
        fs::remove_file(dir.join(MANIFEST)).unwrap();
        assert_eq!(spy(&[("v", &vol)], "databento").unwrap().len(), 1);
        // A dataset folder that can't be read isn't an empty one.
        let ds = vol.join("Databento/OPRA.PILLAR");
        fs::set_permissions(&ds, fs::Permissions::from_mode(0o000)).unwrap();
        let readable = fs::read_dir(&ds).is_ok();
        let got = spy(&[("v", &vol)], "databento");
        let cat = crate::catalog::scan_root("v", &vol, T);
        fs::set_permissions(&ds, fs::Permissions::from_mode(0o755)).unwrap();
        if !readable {
            assert!(got.is_err(), "{got:?}");
            assert!(!cat["errors"].as_array().unwrap().is_empty(), "{cat}");
        }
    }

    /// A Databento batch download in `dir`: `metadata.json` from `query` and
    /// `split_symbols`, and empty files of these names.
    fn raw_batch(dir: &Path, query: &Value, split_symbols: bool, files: &[&str]) {
        put(&dir.join("manifest.json"), b"{}");
        let meta = json!({ "query": query, "customizations": { "split_symbols": split_symbols } });
        put(&dir.join("metadata.json"), meta.to_string().as_bytes());
        for f in files {
            put(&dir.join(f), b"batch");
        }
    }

    fn days_of(days: &[Value]) -> Vec<&str> {
        days.iter().map(|d| d["day"].as_str().unwrap()).collect()
    }

    #[test]
    fn counts_the_store_layout_outside_a_provider_folder_as_databento() {
        let vol = scratch("no-provider");
        put(
            &vol.join("OPRA.PILLAR/SPY.OPT/cbbo-1m/2026-10-01.dbn.zst"),
            b"a",
        );
        let q = json!({ "dataset": "OPRA.PILLAR", "symbols": ["spy.opt"] });
        raw_batch(
            &vol.join("Databento/OPRA-L"),
            &q,
            false,
            &["opra-pillar-20261002.cbbo-1m.dbn.zst"],
        );
        let days = spy(&[("v", &vol)], "databento").unwrap();
        assert_eq!(
            days_of(&days),
            ["2026-10-01", "2026-10-02"],
            "symbols matched ignoring case"
        );
        assert!(spy(&[("v", &vol)], "alpaca").unwrap().is_empty());
    }

    #[test]
    fn counts_every_day_a_batch_file_or_its_query_covers() {
        let vol = scratch("ranges");
        let q = json!({ "dataset": "OPRA.PILLAR", "schema": "cbbo-1m", "symbols": ["SPY.OPT"] });
        raw_batch(
            &vol.join("Databento/OPRA-R"),
            &q,
            false,
            &["opra-pillar-20261001-20261003.cbbo-1m.dbn.zst"],
        );
        // Split by symbol: the query's days, its end excluded.
        let mut split = q.clone();
        split["start"] = json!(ingest::day_ns("2026-10-05").unwrap());
        split["end"] = json!(ingest::day_ns("2026-10-07").unwrap());
        raw_batch(
            &vol.join("Databento/OPRA-S"),
            &split,
            true,
            &["opra-pillar-20261005.cbbo-1m.SPY.OPT.dbn.zst"],
        );
        // A name that doesn't say its day: the query's, given as dates.
        let mut odd = q.clone();
        odd["start"] = json!("2026-10-08T00:00:00Z");
        // Its end excluded, as Databento means it.
        odd["end"] = json!("2026-10-10");
        raw_batch(
            &vol.join("Databento/OPRA-U"),
            &odd,
            false,
            &["spy.cbbo-1m.dbn.zst"],
        );
        let days = spy(&[("v", &vol)], "databento").unwrap();
        assert_eq!(
            days_of(&days),
            [
                "2026-10-01",
                "2026-10-02",
                "2026-10-03",
                "2026-10-05",
                "2026-10-06",
                "2026-10-08",
                "2026-10-09"
            ]
        );
        let cat = crate::catalog::scan_root("v", &vol, T);
        let ds = &cat["datasets"][0]["schemas"]["cbbo-1m"];
        assert_eq!(ds["days"].as_object().unwrap().len(), 7, "{ds}");
        assert_eq!(cat["errors"], json!([]));
        // Without the query's range, the batch is an error, not skipped.
        raw_batch(
            &vol.join("Databento/OPRA-V"),
            &q,
            true,
            &["opra-pillar-20261010.cbbo-1m.SPY.OPT.dbn.zst"],
        );
        assert!(
            spy(&[("v", &vol)], "databento")
                .unwrap_err()
                .contains("OPRA-V")
        );
        let cat = crate::catalog::scan_root("v", &vol, T);
        assert!(
            cat["errors"][0].as_str().unwrap().contains("OPRA-V"),
            "{cat}"
        );
    }

    /// A cbbo-1m DBN file of no records whose header lists `symbols`
    /// (requested as `stype`) from `first` to `last` (whole days).
    fn header_bytes(symbols: &[&str], stype: SType, first: &str, last: &str) -> Vec<u8> {
        header_of("OPRA.PILLAR", symbols, stype, first, last)
    }

    /// As `header_bytes`, of `dataset`.
    fn header_of(
        dataset: &str,
        symbols: &[&str],
        stype: SType,
        first: &str,
        last: &str,
    ) -> Vec<u8> {
        let m = Metadata::builder()
            .dataset(dataset)
            .schema(Some(Schema::Cbbo1M))
            .start(ingest::day_ns(first).unwrap())
            .end(NonZeroU64::new(
                ingest::day_ns(last).unwrap() + 86_400_000_000_000,
            ))
            .stype_in(Some(stype))
            .stype_out(SType::InstrumentId)
            .symbols(symbols.iter().map(|s| (*s).to_owned()).collect())
            .build();
        let mut out = Vec::new();
        drop(dbn::encode::dbn::Encoder::with_zstd(&mut out, &m).unwrap());
        out
    }

    #[test]
    fn counts_every_dbn_file_the_layouts_do_not_claim() {
        let vol = scratch("loose");
        let parent = |a: &str, b: &str| header_bytes(&["SPY.OPT"], SType::Parent, a, b);
        // A download extracted by hand: metadata.json, no manifest.
        let manual = vol.join("OPRA-M.manual-extracted");
        put(
            &manual.join("metadata.json"),
            json!({ "query": { "dataset": "OPRA.PILLAR", "schema": "cbbo-1m" } })
                .to_string()
                .as_bytes(),
        );
        put(
            &manual.join("opra-pillar-20261003.cbbo-1m.dbn.zst"),
            &parent("2026-10-03", "2026-10-03"),
        );
        // A loose file, named any way, holding two days.
        put(
            &vol.join("Databento/spy-odd-name.dbn.zst"),
            &parent("2026-10-04", "2026-10-05"),
        );
        // In a schema folder, not named for its day.
        put(
            &vol.join("Databento/OPRA.PILLAR/SPY.OPT/cbbo-1m/extra.dbn.zst"),
            &parent("2026-10-06", "2026-10-06"),
        );
        // The store layout, deeper than the catalog reads it.
        put(
            &vol.join("Databento/a/b/c/d/OPRA.PILLAR/SPY.OPT/cbbo-1m/2026-10-07.dbn.zst"),
            &parent("2026-10-07", "2026-10-07"),
        );
        // A list of contracts: partial, for those of SPY's.
        put(
            &vol.join("contracts.dbn"),
            &header_bytes(
                &[C1, "QQQ   261002C00500000"],
                SType::RawSymbol,
                "2026-10-08",
                "2026-10-08",
            ),
        );
        // Without a manifest, and no DBN: its query's days.
        let csv = vol.join("misc/deeper/still/OPRA-C");
        put(&csv.join("metadata.json"), json!({ "query": { "dataset": "OPRA.PILLAR", "schema": "cbbo-1m", "symbols": ["SPY.OPT"], "start": "2026-10-09", "end": "2026-10-10" } }).to_string().as_bytes());
        put(&csv.join("opra-pillar-20261009.cbbo-1m.csv"), b"a,b");
        // Not counted: Alpaca's provider folder.
        put(
            &vol.join("alpaca/x.dbn.zst"),
            &parent("2026-10-01", "2026-10-01"),
        );
        let days = spy_coverage(&vol);
        let brief: Vec<(&str, &str, bool)> = days
            .iter()
            .map(|d| {
                (
                    d["day"].as_str().unwrap(),
                    d["kind"].as_str().unwrap(),
                    d["filed"] == true,
                )
            })
            .collect();
        assert_eq!(
            brief,
            [
                ("2026-10-03", "complete", false),
                ("2026-10-04", "complete", false),
                ("2026-10-05", "complete", false),
                ("2026-10-06", "complete", false),
                ("2026-10-07", "complete", false),
                ("2026-10-08", "partial", false),
                ("2026-10-09", "complete", false),
            ]
        );
        assert_eq!(days[5]["coverage"]["symbols"], json!([C1]));
        let cat = crate::catalog::scan_root("v", &vol, T);
        assert_eq!(cat["errors"], json!([]));
        let ds = cat["datasets"]
            .as_array()
            .unwrap()
            .iter()
            .find(|d| d["provider"] == "databento" && d["symbol"] == "SPY.OPT")
            .unwrap();
        assert_eq!(ds["filed"], false);
        // Outside Databento's folder (the download at the top, the one in
        // misc) they have no provider.
        let elsewhere = cat["datasets"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|d| d["provider"].is_null())
            .map(|d| d["schemas"]["cbbo-1m"]["days"].as_object().unwrap().len())
            .sum::<usize>();
        assert_eq!(elsewhere, 3, "{cat}");
        assert_eq!(
            ds["schemas"]["cbbo-1m"]["days"].as_object().unwrap().len(),
            4,
            "{ds}"
        );
        // A header that can't be read, changed just now, is being written:
        // pending, read again next scan. Older, it is an error.
        let junk = vol.join("Databento/junk.dbn.zst");
        put(&junk, b"not dbn");
        assert_eq!(spy(&[("v", &vol)], "databento").unwrap().len(), 7);
        let cat = crate::catalog::scan_root("v", &vol, T);
        assert_eq!(cat["errors"], json!([]));
        assert_eq!(cat["pending_files"], json!([junk.display().to_string()]));
        let old = std::time::SystemTime::now() - std::time::Duration::from_secs(200);
        fs::File::options()
            .write(true)
            .open(&junk)
            .unwrap()
            .set_modified(old)
            .unwrap();
        assert!(
            spy(&[("v", &vol)], "databento")
                .unwrap_err()
                .contains("junk")
        );
        let cat = crate::catalog::scan_root("v", &vol, T);
        assert!(cat["errors"][0].as_str().unwrap().contains("junk"), "{cat}");
        assert_eq!(cat["pending_files"], json!([]));
    }

    fn spy_coverage(vol: &Path) -> Vec<Value> {
        spy(&[("v", vol)], "databento").unwrap()
    }

    #[test]
    fn reads_each_header_once_while_the_file_is_unchanged() {
        let vol = scratch("header-cache");
        let file = vol.join("Databento/loose.dbn.zst");
        let bytes = header_bytes(&["SPY.OPT"], SType::Parent, "2026-10-03", "2026-10-03");
        put(&file, &bytes);
        let at = fs::metadata(&file).unwrap().modified().unwrap();
        assert_eq!(spy_coverage(&vol).len(), 1);
        // Same size and last change: the header isn't read again.
        fs::write(&file, vec![0u8; bytes.len()]).unwrap();
        fs::File::options()
            .write(true)
            .open(&file)
            .unwrap()
            .set_modified(at)
            .unwrap();
        assert_eq!(spy_coverage(&vol).len(), 1, "from the cache");
        // Changed (and not just now, so not taken as being written).
        let earlier = at - std::time::Duration::from_secs(200);
        fs::File::options()
            .write(true)
            .open(&file)
            .unwrap()
            .set_modified(earlier)
            .unwrap();
        assert!(spy(&[("v", &vol)], "databento").is_err());
    }

    #[test]
    fn data_outside_provider_folders_has_no_provider() {
        let vol = scratch("no-provider-files");
        // Databento's data in a backup, its dataset spelled another way.
        put(
            &vol.join("backups/old/spy.dbn.zst"),
            &header_of(
                "opra.pillar",
                &["SPY.OPT"],
                SType::Parent,
                "2026-10-03",
                "2026-10-03",
            ),
        );
        // A crypto recording.
        put(
            &vol.join("coinbase/btc.dbn.zst"),
            &header_of(
                "COINBASE.L2",
                &["BTC-USD"],
                SType::RawSymbol,
                "2026-10-03",
                "2026-10-03",
            ),
        );
        put(
            &vol.join("Alpaca/spy.dbn.zst"),
            &header_bytes(&["SPY.OPT"], SType::Parent, "2026-10-04", "2026-10-04"),
        );
        let cat = crate::catalog::scan_root("v", &vol, T);
        let providers: Vec<(Value, Value)> = cat["datasets"]
            .as_array()
            .unwrap()
            .iter()
            .map(|d| (d["provider"].clone(), d["id"].clone()))
            .collect();
        assert_eq!(
            providers,
            [
                (Value::Null, json!("COINBASE.L2")),
                (Value::Null, json!("opra.pillar")),
                (json!("alpaca"), json!("OPRA.PILLAR")),
            ]
        );
        // The backup's copy blocks a Databento purchase; Alpaca's doesn't.
        assert_eq!(days_of(&spy_coverage(&vol)), ["2026-10-03"]);
        assert_eq!(
            days_of(&spy(&[("v", &vol)], "alpaca").unwrap()),
            ["2026-10-04"]
        );
    }

    #[test]
    fn instrument_ids_hold_every_symbol_of_the_dataset() {
        let vol = scratch("instrument-ids");
        put(
            &vol.join("Databento/ids.dbn.zst"),
            &header_bytes(&["12345"], SType::InstrumentId, "2026-10-04", "2026-10-04"),
        );
        put(
            &vol.join("Databento/qqq.dbn.zst"),
            &header_bytes(&["QQQ.OPT"], SType::Parent, "2026-10-05", "2026-10-05"),
        );
        let v = [("v".to_owned(), vol.clone())];
        let held = |symbol: &str| -> Vec<String> {
            coverage(
                &v,
                "databento",
                "OPRA.PILLAR",
                Some(symbol),
                "cbbo-1m",
                "2026-10-01",
                "2026-10-09",
            )
            .unwrap()
            .iter()
            .map(|d| {
                format!(
                    "{} {}",
                    d["day"].as_str().unwrap(),
                    d["kind"].as_str().unwrap()
                )
            })
            .collect()
        };
        assert_eq!(held("SPY.OPT"), ["2026-10-04 complete"]);
        assert_eq!(
            held("QQQ.OPT"),
            ["2026-10-04 complete", "2026-10-05 complete"]
        );
    }

    #[test]
    fn the_stores_own_folders_and_receipts_hold_nothing() {
        let vol = scratch("own-folders");
        let q = json!({ "dataset": "OPRA.PILLAR", "schema": "cbbo-1m", "symbols": ["SPY.OPT"], "start": "2026-10-01", "end": "2026-10-04" });
        let receipts = |dir: PathBuf, manifest: bool| {
            put(
                &dir.join("metadata.json"),
                json!({ "query": q }).to_string().as_bytes(),
            );
            put(&dir.join(IMPORT), b"{}");
            if manifest {
                put(&dir.join("manifest.json"), b"{}");
            }
        };
        receipts(vol.join("Databento/provenance/OPRA-P"), true);
        receipts(vol.join("Databento/provenance/OPRA-Q"), false);
        put(
            &vol.join("Databento/migrations/2026-10-19-import-x.events.jsonl"),
            b"{}",
        );
        // A download with its manifest but no data holds nothing either.
        receipts(vol.join("Databento/downloads/OPRA-E"), true);
        assert_eq!(spy_coverage(&vol), Vec::<Value>::new());
        let cat = crate::catalog::scan_root("v", &vol, T);
        assert_eq!(
            (cat["datasets"].clone(), cat["errors"].clone()),
            (json!([]), json!([]))
        );
        // Without its manifest, outside the store's folders, its query
        // counts, the end excluded; an end later in a day includes it.
        receipts(vol.join("Databento/downloads/OPRA-F"), false);
        assert_eq!(
            days_of(&spy_coverage(&vol)),
            ["2026-10-01", "2026-10-02", "2026-10-03"]
        );
        let mut late = q.clone();
        late["end"] = json!("2026-10-04T12:00:00Z");
        put(
            &vol.join("Databento/downloads/OPRA-F/metadata.json"),
            json!({ "query": late }).to_string().as_bytes(),
        );
        assert_eq!(spy_coverage(&vol).len(), 4);
    }

    #[test]
    fn reports_folders_it_cannot_read() {
        use std::os::unix::fs::PermissionsExt as _;
        let vol = scratch("unreadable-misc");
        let misc = vol.join("misc");
        fs::create_dir_all(&misc).unwrap();
        fs::set_permissions(&misc, fs::Permissions::from_mode(0o000)).unwrap();
        let readable = fs::read_dir(&misc).is_ok();
        let cat = crate::catalog::scan_root("v", &vol, T);
        fs::set_permissions(&misc, fs::Permissions::from_mode(0o755)).unwrap();
        if !readable {
            assert!(cat["errors"][0].as_str().unwrap().contains("misc"), "{cat}");
        }
    }

    /// Every file in `TT_REAL_PARTIAL_DIR` passes `inspect`, as an import
    /// would check it. Run with `--ignored`.
    #[test]
    #[ignore = "reads real files named by TT_REAL_PARTIAL_DIR"]
    fn real_partial_days_inspect() {
        let Ok(dir) = std::env::var("TT_REAL_PARTIAL_DIR") else {
            return;
        };
        let mut n = 0;
        for e in fs::read_dir(dir).unwrap().flatten() {
            if e.file_name().to_string_lossy().ends_with(".dbn.zst") {
                if let Err(err) = inspect(&e.path()) {
                    panic!("{}: {err}", e.path().display());
                }
                n += 1;
            }
        }
        eprintln!("{n} files inspected");
    }
}

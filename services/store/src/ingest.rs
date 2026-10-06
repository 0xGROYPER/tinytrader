//! Ingest: provider downloads into the standard layout (decision 0002).
//!
//! A batch is uploaded file by file into `<volume>/<Provider>/.incoming/<batch>/`,
//! each file written to a `.partial`, hashed, flushed and only then renamed
//! into place. Filing the batch hashes every file again, checks it against
//! the provider's manifest, and moves each data file to
//! `<Provider>/<DATASET>/[<symbol>/]<schema>/<YYYY-MM-DD>.dbn.zst`.
//!
//! **Existing data is never replaced.** A day already held, on any volume, is
//! either byte-identical (SHA-256 recomputed on both sides; the incoming copy
//! is dropped) or the whole batch is refused. Moves are hard links, which
//! cannot replace a file even one that appeared after the checks. Receipts go
//! to `<Provider>/provenance/<batch>/` (download URLs removed) on the same
//! terms, and every step is recorded in `<Provider>/migrations/`. Filing can
//! be repeated, so an interrupted run resumes. Nothing is deleted except a
//! proven duplicate and the store's own incoming files.

use std::fs;
use std::io::{Read as _, Write as _};
use std::path::{Path, PathBuf};

use serde_json::{Value, json};
use sha2::{Digest as _, Sha256};

/// A volume the store manages.
#[derive(Clone, Debug)]
pub struct Volume {
    pub name: String,
    pub path: PathBuf,
    pub writable: bool,
}

/// Where incoming files wait to be incorporated.
pub const INCOMING: &str = ".incoming";

/// A provider's directory name, for the providers the store ingests.
#[must_use]
pub fn provider_dir(provider: &str) -> Option<&'static str> {
    match provider.to_ascii_lowercase().as_str() {
        "databento" => Some("Databento"),
        _ => None,
    }
}

/// A single path component safe to use as a batch or file name.
#[must_use]
pub fn safe(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 200
        && !name.starts_with('.')
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// The SHA-256 of a file, as lowercase hex.
pub fn hash_file(path: &Path) -> Result<String, String> {
    let mut f = fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = f
            .read(&mut buf)
            .map_err(|e| format!("{}: {e}", path.display()))?;
        if n == 0 {
            return Ok(hex(&h.finalize()));
        }
        h.update(&buf[..n]);
    }
}

fn sync_dir(dir: &Path) {
    if let Ok(d) = fs::File::open(dir) {
        let _ = d.sync_all();
    }
}

/// Writes one uploaded file: chunks go to a `.partial`, which is checked and
/// renamed into place once complete.
pub struct Upload {
    dir: PathBuf,
    name: String,
    partial: PathBuf,
    file: fs::File,
    hash: Sha256,
    size: u64,
}

impl Upload {
    /// # Errors
    /// If the incoming directory or the partial file can't be created.
    pub fn start(dir: &Path, name: &str) -> Result<Self, String> {
        fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        let partial = dir.join(format!("{name}.partial"));
        let file = fs::File::create(&partial).map_err(|e| format!("{}: {e}", partial.display()))?;
        Ok(Self {
            dir: dir.to_owned(),
            name: name.to_owned(),
            partial,
            file,
            hash: Sha256::new(),
            size: 0,
        })
    }

    /// # Errors
    /// On a write failure.
    pub fn write(&mut self, chunk: &[u8]) -> Result<(), String> {
        self.hash.update(chunk);
        self.size += chunk.len() as u64;
        self.file.write_all(chunk).map_err(|e| e.to_string())
    }

    /// Flush, check against what was expected, and rename into place.
    /// Returns the size and SHA-256.
    ///
    /// # Errors
    /// If the size or hash differs (the partial is removed), or on I/O failure.
    pub fn finish(self, size: Option<u64>, sha256: Option<&str>) -> Result<(u64, String), String> {
        self.file.sync_all().map_err(|e| e.to_string())?;
        let got = hex(&self.hash.finalize());
        let want = sha256.map(|s| s.trim_start_matches("sha256:").to_ascii_lowercase());
        if size.is_some_and(|s| s != self.size) || want.as_ref().is_some_and(|w| *w != got) {
            let _ = fs::remove_file(&self.partial);
            return Err(format!(
                "{}: received {} bytes with SHA-256 {got}, expected {} bytes with {}",
                self.name,
                self.size,
                size.map_or("any".into(), |s| s.to_string()),
                want.unwrap_or_else(|| "any".into())
            ));
        }
        let final_path = self.dir.join(&self.name);
        fs::rename(&self.partial, &final_path).map_err(|e| e.to_string())?;
        sync_dir(&self.dir);
        Ok((self.size, got))
    }
}

/// Appends step records to a migration ledger.
struct Ledger {
    file: fs::File,
}

impl Ledger {
    fn open(path: &Path) -> Result<Self, String> {
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        let file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .map_err(|e| e.to_string())?;
        Ok(Self { file })
    }
    fn record(&mut self, mut event: Value) {
        event["at"] = json!(tinytrader_observe::unix_seconds());
        let _ = writeln!(self.file, "{event}");
        let _ = self.file.sync_all();
    }
}

/// `YYYY-MM-DD` of a Databento file name such as
/// `opra-pillar-20251001.cmbp-1.dbn.zst`, if it is a daily DBN file of `schema`.
fn databento_day(name: &str, schema: &str) -> Option<String> {
    let (stem, rest) = name.split_once('.')?;
    let day = stem.rsplit('-').next()?;
    if day.len() != 8 || !day.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    (rest.strip_prefix(schema)? == ".dbn.zst")
        .then(|| format!("{}-{}-{}", &day[..4], &day[4..6], &day[6..]))
}

/// Move `from` to `to` without ever replacing an existing `to`: a hard link
/// (which fails if `to` exists, atomically) then removing `from`. Both are on
/// the same volume.
fn place_new(from: &Path, to: &Path) -> std::io::Result<()> {
    let dir = to
        .parent()
        .ok_or_else(|| std::io::Error::other("no parent directory"))?;
    fs::create_dir_all(dir)?;
    fs::hard_link(from, to)?;
    sync_dir(dir);
    fs::remove_file(from)?;
    if let Some(d) = from.parent() {
        sync_dir(d);
    }
    Ok(())
}

/// Write `bytes` to a new file at `to`, never replacing one.
fn write_new(to: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(dir) = to.parent() {
        fs::create_dir_all(dir)?;
    }
    let mut f = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(to)?;
    f.write_all(bytes)?;
    f.sync_all()?;
    if let Some(dir) = to.parent() {
        sync_dir(dir);
    }
    Ok(())
}

/// Where a data file goes, and what is already there.
enum Plan {
    /// Nothing holds this day yet: move the file here.
    Place {
        from: PathBuf,
        to: PathBuf,
        sha256: String,
    },
    /// An identical file (same SHA-256, recomputed) is already in place,
    /// here or on another volume: the incoming copy is a provable duplicate.
    Duplicate {
        from: PathBuf,
        existing: PathBuf,
        sha256: String,
    },
}

/// Incorporate a Databento batch waiting in `<root>/.incoming/<batch>`, where
/// `root` is the provider directory. `elsewhere` are the same provider's
/// directories on the store's other volumes (read-only ones included), which
/// are checked for the same days. Returns a summary of what happened.
///
/// Existing data is never replaced. A day already held anywhere is either the
/// same bytes (the incoming copy is dropped as a duplicate) or the whole batch
/// is refused. Every check runs before anything moves, and each move cannot
/// replace a file even if one appeared since the check.
///
/// # Errors
/// If the batch is incomplete, a file doesn't match the manifest, the
/// request shape isn't supported, or a destination holds a different file.
pub fn incorporate_databento(
    root: &Path,
    elsewhere: &[PathBuf],
    batch: &str,
    today: &str,
) -> Result<Value, String> {
    file_databento(
        &root.join(INCOMING).join(batch),
        root,
        elsewhere,
        batch,
        today,
    )
}

/// File a Databento download folder where it already is (`dir`, on the same
/// volume as the provider directory `root`), on exactly the same terms as an
/// incoming batch. Nothing is copied: files move by link on the same volume.
/// If any check fails, the folder is left exactly as it was.
///
/// # Errors
/// As [`incorporate_databento`].
pub fn adopt_databento(
    dir: &Path,
    root: &Path,
    elsewhere: &[PathBuf],
    today: &str,
) -> Result<Value, String> {
    let batch = dir
        .file_name()
        .and_then(|n| n.to_str())
        .filter(|n| safe(n))
        .ok_or("not a batch folder")?;
    file_databento(dir, root, elsewhere, batch, today)
}

fn file_databento(
    inc: &Path,
    root: &Path,
    elsewhere: &[PathBuf],
    batch: &str,
    today: &str,
) -> Result<Value, String> {
    let inc = inc.to_path_buf();
    let read_json = |name: &str| -> Result<Value, String> {
        let p = inc.join(name);
        let text = fs::read_to_string(&p).map_err(|e| format!("{name}: {e}"))?;
        serde_json::from_str(&text).map_err(|e| format!("{name}: {e}"))
    };
    let manifest = read_json("manifest.json")?;
    let metadata = read_json("metadata.json")?;
    let q = &metadata["query"];
    let (dataset, schema) = (
        q["dataset"]
            .as_str()
            .ok_or("metadata.json has no dataset")?,
        q["schema"].as_str().ok_or("metadata.json has no schema")?,
    );
    if !safe(dataset) || !safe(schema) {
        return Err("metadata.json names an unsafe dataset or schema".into());
    }
    if q["encoding"] != "dbn" || q["compression"] != "zstd" {
        return Err(
            "only DBN batches compressed with zstd are filed (the store's format is .dbn.zst)"
                .into(),
        );
    }
    let c = &metadata["customizations"];
    if c["split_symbols"] == true || c["split_duration"] != "day" {
        return Err("only batches split by day, and not by symbol, are filed".into());
    }
    let symbols: Vec<&str> = q["symbols"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect();
    // A file at <DATASET>/<schema>/ holds every symbol; one requested symbol
    // (such as a parent, SPY.OPT) gets its own level. Anything else would
    // file a partial set of symbols where all of them are expected.
    let level: PathBuf = match symbols.as_slice() {
        ["ALL_SYMBOLS"] => PathBuf::from(dataset).join(schema),
        [one] if safe(one) => PathBuf::from(dataset).join(one).join(schema),
        _ => {
            return Err(format!(
                "this batch requests {} symbols; only batches for one symbol (or all symbols) can be filed, so request each symbol separately",
                symbols.len()
            ));
        }
    };
    let prov = root.join("provenance").join(batch);
    let mut ledger = Ledger::open(
        &root
            .join("migrations")
            .join(format!("{today}-incorporate-{batch}.events.jsonl")),
    )?;
    ledger.record(json!({ "step": "begin", "batch": batch, "dataset": dataset, "schema": schema, "symbols": symbols }));

    // The manifest as kept: without its download URLs.
    let mut kept_manifest = manifest.clone();
    for f in kept_manifest["files"].as_array_mut().into_iter().flatten() {
        if let Some(o) = f.as_object_mut() {
            o.remove("urls");
        }
    }
    let kept_manifest = serde_json::to_string_pretty(&kept_manifest).map_err(|e| e.to_string())?;

    // Check everything, and plan every move, before moving anything.
    let files = manifest["files"]
        .as_array()
        .ok_or("manifest.json lists no files")?;
    let mut plans = Vec::new();
    let mut receipts = Vec::new();
    let mut problems = Vec::new();
    let mut check = |f: &Value| -> Result<(), String> {
        let name = f["filename"].as_str().unwrap_or_default();
        if !safe(name) {
            return Err(format!("unsafe file name {name:?}"));
        }
        let want = f["hash"]
            .as_str()
            .unwrap_or_default()
            .trim_start_matches("sha256:")
            .to_ascii_lowercase();
        let path = inc.join(name);
        if name.ends_with(".json") {
            let kept = prov.join(name);
            if !path.exists() {
                // Kept by an earlier, interrupted run?
                return if kept.exists() && hash_file(&kept)? == want {
                    Ok(())
                } else {
                    Err(format!("{name}: missing"))
                };
            }
            if kept.exists() && hash_file(&kept)? != hash_file(&path)? {
                return Err(format!(
                    "{}: provenance already holds a different {name}",
                    prov.display()
                ));
            }
        }
        let rel = if name.ends_with(".json") {
            None
        } else {
            let day = databento_day(name, schema)
                .ok_or_else(|| format!("{name}: not a daily {schema} .dbn.zst file"))?;
            Some(level.join(format!("{day}.dbn.zst")))
        };
        // Every place this day could already be, on any volume.
        let existing: Vec<(PathBuf, String)> = match &rel {
            Some(rel) => std::iter::once(root)
                .chain(elsewhere.iter().map(PathBuf::as_path))
                .map(|r| r.join(rel))
                .filter(|p| p.exists())
                .map(|p| hash_file(&p).map(|h| (p, h)))
                .collect::<Result<_, _>>()?,
            None => Vec::new(),
        };
        if let Some((p, h)) = existing.iter().find(|(_, h)| *h != want) {
            return Err(format!(
                "{name}: {} already holds a different file (SHA-256 {h}, this batch has {want})",
                p.display()
            ));
        }
        let Ok(meta) = fs::metadata(&path) else {
            // Moved by an earlier, interrupted run?
            return if existing.is_empty() {
                Err(format!("{name}: missing"))
            } else {
                Ok(())
            };
        };
        if f["size"].as_u64().is_some_and(|s| s != meta.len()) {
            return Err(format!(
                "{name}: {} bytes, manifest says {}",
                meta.len(),
                f["size"]
            ));
        }
        let got = hash_file(&path)?;
        if got != want {
            return Err(format!("{name}: SHA-256 {got}, manifest says {want}"));
        }
        ledger.record(
            json!({ "step": "verified", "file": name, "bytes": meta.len(), "sha256": got }),
        );
        match (rel, existing.into_iter().next()) {
            (None, _) => receipts.push(name.to_owned()),
            (Some(_), Some((p, _))) => plans.push(Plan::Duplicate {
                from: path,
                existing: p,
                sha256: got,
            }),
            (Some(rel), None) => plans.push(Plan::Place {
                from: path,
                to: root.join(rel),
                sha256: got,
            }),
        }
        Ok(())
    };
    for f in files {
        if let Err(e) = check(f) {
            problems.push(e);
        }
    }
    let kept = prov.join("manifest.json");
    if kept.exists() && fs::read_to_string(&kept).map_err(|e| e.to_string())? != kept_manifest {
        problems.push(format!(
            "{}: provenance already holds a different manifest.json",
            prov.display()
        ));
    }
    if !problems.is_empty() {
        ledger.record(json!({ "step": "refused", "problems": problems }));
        return Err(problems.join("; "));
    }

    // Every check passed: move the data into place, never over anything.
    let mut placed = Vec::new();
    for plan in plans {
        match plan {
            Plan::Place { from, to, sha256 } => {
                if let Err(e) = place_new(&from, &to) {
                    let why = if e.kind() == std::io::ErrorKind::AlreadyExists {
                        format!(
                            "{} appeared while filing; nothing was replaced, file the batch again",
                            to.display()
                        )
                    } else {
                        format!("{} -> {}: {e}", from.display(), to.display())
                    };
                    ledger.record(json!({ "step": "stopped", "error": why }));
                    return Err(why);
                }
                ledger.record(json!({ "step": "moved", "from": from, "to": to, "sha256": sha256 }));
                placed.push(json!({ "path": to, "sha256": sha256 }));
            }
            Plan::Duplicate {
                from,
                existing,
                sha256,
            } => {
                fs::remove_file(&from).map_err(|e| e.to_string())?;
                ledger.record(json!({ "step": "duplicate-dropped", "file": from, "identical_to": existing, "sha256": sha256 }));
                placed.push(json!({ "path": existing, "sha256": sha256, "already_present": true }));
            }
        }
    }

    // Receipts go to provenance, never over a different one (checked above).
    for name in receipts.iter().filter(|n| *n != "manifest.json") {
        let (from, to) = (inc.join(name), prov.join(name));
        if to.exists() {
            fs::remove_file(&from).map_err(|e| e.to_string())?;
        } else {
            place_new(&from, &to).map_err(|e| format!("{}: {e}", to.display()))?;
        }
        ledger.record(json!({ "step": "receipt", "file": to }));
    }
    // Other receipts left in the folder (such as a downloader's own listing)
    // are kept beside the provider's; anything else stays where it is.
    for e in fs::read_dir(&inc).map_err(|e| e.to_string())?.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if name == "manifest.json" || !name.ends_with(".json") || !safe(&name) {
            continue;
        }
        let to = prov.join(&name);
        if to.exists() {
            if hash_file(&to)? == hash_file(&e.path())? {
                fs::remove_file(e.path()).map_err(|e| e.to_string())?;
            }
            continue;
        }
        place_new(&e.path(), &to).map_err(|e| format!("{}: {e}", to.display()))?;
        ledger.record(json!({ "step": "receipt", "file": to }));
    }
    // The manifest doesn't list itself; it is always kept.
    if !kept.exists() {
        write_new(&kept, kept_manifest.as_bytes())
            .map_err(|e| format!("{}: {e}", kept.display()))?;
    }
    let from = inc.join("manifest.json");
    let _ = fs::remove_file(&from);
    ledger.record(json!({ "step": "receipt", "file": kept }));
    // The folder is empty now; remove it (never recursively). Anything the
    // store didn't recognise keeps it there, and is reported.
    let left: Vec<String> = fs::read_dir(&inc)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    let _ = fs::remove_dir(&inc);
    let _ = fs::remove_dir(root.join(INCOMING));
    let summary = json!({
        "batch": batch, "dataset": dataset, "schema": schema, "symbols": symbols,
        "files": placed, "receipts": receipts, "provenance": prov, "from": inc, "left_behind": left, "completed_at": tinytrader_observe::unix_seconds(),
    });
    let text = serde_json::to_string_pretty(&summary).map_err(|e| e.to_string())?;
    let mut out = root
        .join("migrations")
        .join(format!("{today}-incorporate-{batch}.json"));
    if out.exists() {
        out = root.join("migrations").join(format!(
            "{today}-incorporate-{batch}-{}.json",
            tinytrader_observe::unix_seconds() as u64
        ));
    }
    write_new(&out, text.as_bytes()).map_err(|e| format!("{}: {e}", out.display()))?;
    ledger.record(json!({ "step": "complete" }));
    Ok(summary)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!(
            "tinytrader-store-test-{name}-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    fn upload(dir: &Path, name: &str, bytes: &[u8]) -> String {
        let mut u = Upload::start(dir, name).unwrap();
        u.write(bytes).unwrap();
        u.finish(Some(bytes.len() as u64), None).unwrap().1
    }

    /// Stage a Databento batch of one day for `symbols` in `root`.
    fn stage(root: &Path, batch: &str, symbols: &[&str], data: &[u8]) {
        let inc = root.join(INCOMING).join(batch);
        let data_sha = upload(&inc, "opra-pillar-20251001.cmbp-1.dbn.zst", data);
        let metadata = json!({
            "job_id": batch,
            "query": { "dataset": "OPRA.PILLAR", "schema": "cmbp-1", "symbols": symbols, "encoding": "dbn", "compression": "zstd" },
            "customizations": { "split_symbols": false, "split_duration": "day" },
        })
        .to_string();
        let meta_sha = upload(&inc, "metadata.json", metadata.as_bytes());
        let manifest = json!({ "job_id": batch, "files": [
            { "filename": "opra-pillar-20251001.cmbp-1.dbn.zst", "size": data.len(), "hash": format!("sha256:{data_sha}"), "urls": { "https": "secret" } },
            { "filename": "metadata.json", "size": metadata.len(), "hash": format!("sha256:{meta_sha}"), "urls": { "https": "secret" } },
        ] })
        .to_string();
        upload(&inc, "manifest.json", manifest.as_bytes());
    }

    const DAY: &str = "OPRA.PILLAR/SPX.OPT/cmbp-1/2025-10-01.dbn.zst";

    #[test]
    fn refuses_a_file_that_does_not_match() {
        let d = scratch("mismatch");
        let mut u = Upload::start(&d, "x.dbn.zst").unwrap();
        u.write(b"abc").unwrap();
        assert!(u.finish(Some(3), Some("sha256:00")).is_err());
        assert!(!d.join("x.dbn.zst").exists() && !d.join("x.dbn.zst.partial").exists());
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn files_a_batch_in_the_standard_layout() {
        let root = scratch("batch").join("Databento");
        stage(&root, "OPRA-1", &["SPX.OPT"], b"day one");
        let s = incorporate_databento(&root, &[], "OPRA-1", "2026-10-06").unwrap();
        assert_eq!(fs::read(root.join(DAY)).unwrap(), b"day one");
        assert_eq!(s["files"].as_array().unwrap().len(), 1);
        let kept = fs::read_to_string(root.join("provenance/OPRA-1/manifest.json")).unwrap();
        assert!(!kept.contains("secret"), "download URLs removed");
        assert!(root.join("provenance/OPRA-1/metadata.json").exists());
        assert!(!root.join(INCOMING).exists(), "incoming removed once empty");
        assert!(
            root.join("migrations/2026-10-06-incorporate-OPRA-1.json")
                .exists()
        );
        let _ = fs::remove_dir_all(root.parent().unwrap());
    }

    #[test]
    fn never_replaces_a_different_day() {
        let root = scratch("conflict").join("Databento");
        fs::create_dir_all(root.join(DAY).parent().unwrap()).unwrap();
        fs::write(root.join(DAY), b"what we had").unwrap();
        stage(&root, "OPRA-2", &["SPX.OPT"], b"something else");
        let err = incorporate_databento(&root, &[], "OPRA-2", "2026-10-06").unwrap_err();
        assert!(err.contains("already holds a different file"), "{err}");
        assert_eq!(fs::read(root.join(DAY)).unwrap(), b"what we had");
        assert!(
            root.join(INCOMING)
                .join("OPRA-2/opra-pillar-20251001.cmbp-1.dbn.zst")
                .exists(),
            "nothing moved"
        );
        assert!(
            !root.join("provenance/OPRA-2").exists(),
            "no receipts filed"
        );
        let _ = fs::remove_dir_all(root.parent().unwrap());
    }

    #[test]
    fn checks_the_other_volumes_too() {
        let base = scratch("volumes");
        let (here, there) = (base.join("new/Databento"), base.join("old/Databento"));
        fs::create_dir_all(there.join(DAY).parent().unwrap()).unwrap();
        fs::write(there.join(DAY), b"held elsewhere").unwrap();

        // A different day elsewhere: refused.
        stage(&here, "OPRA-3", &["SPX.OPT"], b"not the same");
        let err =
            incorporate_databento(&here, std::slice::from_ref(&there), "OPRA-3", "2026-10-06")
                .unwrap_err();
        assert!(err.contains("already holds a different file"), "{err}");
        assert!(!here.join(DAY).exists());

        // The same bytes elsewhere: a proven duplicate, not a second copy.
        let other = base.join("other/Databento");
        stage(&other, "OPRA-4", &["SPX.OPT"], b"held elsewhere");
        let s = incorporate_databento(&other, std::slice::from_ref(&there), "OPRA-4", "2026-10-06")
            .unwrap();
        assert!(!other.join(DAY).exists(), "no second copy");
        assert_eq!(s["files"][0]["already_present"], true);
        assert_eq!(fs::read(there.join(DAY)).unwrap(), b"held elsewhere");
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn refuses_batches_of_several_symbols() {
        let root = scratch("symbols").join("Databento");
        stage(&root, "OPRA-5", &["SPY.OPT", "QQQ.OPT"], b"two parents");
        let err = incorporate_databento(&root, &[], "OPRA-5", "2026-10-06").unwrap_err();
        assert!(err.contains("one symbol"), "{err}");
        assert!(!root.join("OPRA.PILLAR").exists());
        let _ = fs::remove_dir_all(root.parent().unwrap());
    }

    #[test]
    fn filing_the_same_batch_again_changes_nothing() {
        let root = scratch("again").join("Databento");
        stage(&root, "OPRA-6", &["SPX.OPT"], b"day one");
        incorporate_databento(&root, &[], "OPRA-6", "2026-10-06").unwrap();
        let receipt = fs::read(root.join("provenance/OPRA-6/manifest.json")).unwrap();
        stage(&root, "OPRA-6", &["SPX.OPT"], b"day one");
        let s = incorporate_databento(&root, &[], "OPRA-6", "2026-10-06").unwrap();
        assert_eq!(s["files"][0]["already_present"], true);
        assert_eq!(fs::read(root.join(DAY)).unwrap(), b"day one");
        assert_eq!(
            fs::read(root.join("provenance/OPRA-6/manifest.json")).unwrap(),
            receipt
        );
        let summaries = fs::read_dir(root.join("migrations"))
            .unwrap()
            .filter(|e| {
                e.as_ref()
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .ends_with(".json")
            })
            .count();
        assert_eq!(summaries, 2, "the first summary is kept, not rewritten");
        let _ = fs::remove_dir_all(root.parent().unwrap());
    }

    #[test]
    fn refuses_a_different_receipt_for_the_same_batch() {
        let root = scratch("receipt").join("Databento");
        fs::create_dir_all(root.join("provenance/OPRA-7")).unwrap();
        fs::write(root.join("provenance/OPRA-7/manifest.json"), "{}").unwrap();
        stage(&root, "OPRA-7", &["SPX.OPT"], b"day one");
        let err = incorporate_databento(&root, &[], "OPRA-7", "2026-10-06").unwrap_err();
        assert!(err.contains("different manifest.json"), "{err}");
        assert!(!root.join(DAY).exists());
        assert_eq!(
            fs::read_to_string(root.join("provenance/OPRA-7/manifest.json")).unwrap(),
            "{}"
        );
        let _ = fs::remove_dir_all(root.parent().unwrap());
    }

    #[test]
    fn resumes_after_an_interruption() {
        let root = scratch("resume").join("Databento");
        stage(&root, "OPRA-8", &["SPX.OPT"], b"day one");
        // As if a run stopped right after moving the data file.
        fs::create_dir_all(root.join(DAY).parent().unwrap()).unwrap();
        fs::rename(
            root.join(INCOMING)
                .join("OPRA-8/opra-pillar-20251001.cmbp-1.dbn.zst"),
            root.join(DAY),
        )
        .unwrap();
        incorporate_databento(&root, &[], "OPRA-8", "2026-10-06").unwrap();
        assert_eq!(fs::read(root.join(DAY)).unwrap(), b"day one");
        assert!(root.join("provenance/OPRA-8/manifest.json").exists());
        assert!(!root.join(INCOMING).exists());
        let _ = fs::remove_dir_all(root.parent().unwrap());
    }

    #[test]
    fn adopts_a_download_folder_in_place() {
        let base = scratch("adopt");
        let root = base.join("Databento");
        // A download folder at the top of the volume, with an extra receipt.
        stage(&base, "OPRA-9", &["SPX.OPT"], b"day one");
        let dir = base.join("OPRA-9");
        fs::rename(base.join(INCOMING).join("OPRA-9"), &dir).unwrap();
        fs::write(dir.join("_batch_listing.json"), "[]").unwrap();
        let s = adopt_databento(&dir, &root, &[], "2026-10-06").unwrap();
        assert_eq!(fs::read(root.join(DAY)).unwrap(), b"day one");
        assert!(root.join("provenance/OPRA-9/_batch_listing.json").exists());
        assert!(!dir.exists(), "the folder is gone once filed");
        assert_eq!(s["left_behind"], json!([]));

        // A folder that fails a check is left exactly as it was.
        stage(&base, "OPRA-10", &["SPX.OPT"], b"different day one");
        let bad = base.join("OPRA-10");
        fs::rename(base.join(INCOMING).join("OPRA-10"), &bad).unwrap();
        assert!(adopt_databento(&bad, &root, &[], "2026-10-06").is_err());
        assert_eq!(fs::read_dir(&bad).unwrap().count(), 3);
        assert_eq!(fs::read(root.join(DAY)).unwrap(), b"day one");
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn a_move_cannot_replace_a_file() {
        let d = scratch("link");
        fs::write(d.join("a"), b"incoming").unwrap();
        fs::write(d.join("b"), b"already here").unwrap();
        assert_eq!(
            place_new(&d.join("a"), &d.join("b")).unwrap_err().kind(),
            std::io::ErrorKind::AlreadyExists
        );
        assert_eq!(fs::read(d.join("b")).unwrap(), b"already here");
        assert!(d.join("a").exists());
        let _ = fs::remove_dir_all(&d);
    }
}

//! Ingest: provider downloads into the standard layout (decision 0002).
//!
//! A batch is uploaded file by file into `<volume>/<Provider>/.incoming/<batch>/`,
//! each file written to a `.partial`, hashed, flushed and only then renamed
//! into place, with its SHA-256 kept beside it. Incorporating the batch
//! checks every file against the provider's manifest, then renames each data
//! file to `<Provider>/<DATASET>/[<symbol>/]<schema>/<YYYY-MM-DD>.<encoding>`
//! (never over a different file), keeps the receipts in
//! `<Provider>/provenance/<batch>/` (download URLs removed), and records every
//! step in `<Provider>/migrations/`. Each step can be repeated, so an
//! interrupted incorporation resumes. Nothing is ever deleted except a
//! verified duplicate of a file already in place, and helper files the store
//! itself wrote.

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
        let mut side = fs::File::create(self.dir.join(format!("{}.sha256", self.name)))
            .map_err(|e| e.to_string())?;
        side.write_all(got.as_bytes())
            .and_then(|()| side.sync_all())
            .map_err(|e| e.to_string())?;
        sync_dir(&self.dir);
        Ok((self.size, got))
    }
}

/// A file's SHA-256: from the sidecar the upload wrote, or computed.
fn known_hash(path: &Path) -> Result<String, String> {
    let side = PathBuf::from(format!("{}.sha256", path.display()));
    match fs::read_to_string(&side) {
        Ok(h) if h.trim().len() == 64 => Ok(h.trim().to_owned()),
        _ => hash_file(path),
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
/// `opra-pillar-20251001.cmbp-1.dbn.zst`, and its encoding (`dbn.zst`).
fn databento_file(name: &str, schema: &str) -> Option<(String, String)> {
    let (stem, rest) = name.split_once('.')?;
    let day = stem.rsplit('-').next()?;
    if day.len() != 8 || !day.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let encoding = rest.strip_prefix(schema)?.strip_prefix('.')?;
    Some((
        format!("{}-{}-{}", &day[..4], &day[4..6], &day[6..]),
        encoding.to_owned(),
    ))
}

/// Incorporate a Databento batch waiting in `<root>/.incoming/<batch>`, where
/// `root` is the provider directory. Returns a summary of what happened.
///
/// # Errors
/// If the batch is incomplete, a file doesn't match the manifest, the
/// request shape isn't supported, or a destination holds a different file.
/// Nothing is moved unless every check passes.
pub fn incorporate_databento(root: &Path, batch: &str, today: &str) -> Result<Value, String> {
    let inc = root.join(INCOMING).join(batch);
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
    let c = &metadata["customizations"];
    if c["split_symbols"] == true || c["split_duration"].as_str().is_some_and(|d| d != "day") {
        return Err("only batches split by day, not by symbol, are supported".into());
    }
    let symbols: Vec<&str> = q["symbols"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect();
    // One requested symbol (such as a parent, SPY.OPT) gets a symbol level.
    let base = match symbols.as_slice() {
        [one] => root.join(dataset).join(one).join(schema),
        _ => root.join(dataset).join(schema),
    };
    let mut ledger = Ledger::open(
        &root
            .join("migrations")
            .join(format!("{today}-incorporate-{batch}.events.jsonl")),
    )?;
    ledger.record(json!({ "step": "begin", "batch": batch, "dataset": dataset, "schema": schema, "symbols": symbols }));

    // Check every file against the manifest, and plan every move.
    let files = manifest["files"]
        .as_array()
        .ok_or("manifest.json lists no files")?;
    let mut moves = Vec::new();
    let mut receipts = Vec::new();
    let mut problems = Vec::new();
    for f in files {
        let name = f["filename"].as_str().unwrap_or_default();
        if !safe(name) {
            problems.push(format!("unsafe file name {name:?}"));
            continue;
        }
        let path = inc.join(name);
        let size = f["size"].as_u64();
        let want = f["hash"]
            .as_str()
            .unwrap_or_default()
            .trim_start_matches("sha256:")
            .to_ascii_lowercase();
        let meta = fs::metadata(&path);
        let Ok(meta) = meta else {
            // Already moved by an earlier, interrupted run?
            if !name.ends_with(".json")
                && let Some((day, enc)) = databento_file(name, schema)
            {
                let dest = base.join(format!("{day}.{enc}"));
                if dest.exists() && known_hash(&dest).ok().as_deref() == Some(want.as_str()) {
                    continue;
                }
            }
            if name.ends_with(".json") && root.join("provenance").join(batch).join(name).exists() {
                continue;
            }
            problems.push(format!("{name}: missing"));
            continue;
        };
        if size.is_some_and(|s| s != meta.len()) {
            problems.push(format!(
                "{name}: {} bytes, manifest says {}",
                meta.len(),
                size.unwrap_or(0)
            ));
            continue;
        }
        let got = known_hash(&path)?;
        if got != want {
            problems.push(format!("{name}: SHA-256 {got}, manifest says {want}"));
            continue;
        }
        ledger.record(
            json!({ "step": "verified", "file": name, "bytes": meta.len(), "sha256": got }),
        );
        if name.ends_with(".json") {
            receipts.push(name.to_owned());
            continue;
        }
        let Some((day, enc)) = databento_file(name, schema) else {
            problems.push(format!("{name}: not a daily {schema} file"));
            continue;
        };
        let dest = base.join(format!("{day}.{enc}"));
        if dest.exists() {
            if known_hash(&dest)? == got {
                moves.push((path, dest, got, true));
            } else {
                problems.push(format!(
                    "{name}: {} already holds a different file",
                    dest.display()
                ));
            }
        } else {
            moves.push((path, dest, got, false));
        }
    }
    if !problems.is_empty() {
        ledger.record(json!({ "step": "refused", "problems": problems }));
        return Err(problems.join("; "));
    }

    // Every check passed: move the data into place.
    let mut placed = Vec::new();
    for (from, to, sha, duplicate) in moves {
        if duplicate {
            fs::remove_file(&from).map_err(|e| e.to_string())?;
            ledger.record(json!({ "step": "already-present", "file": to, "sha256": sha }));
        } else {
            let dir = to.parent().ok_or("bad destination")?;
            fs::create_dir_all(dir).map_err(|e| e.to_string())?;
            fs::rename(&from, &to)
                .map_err(|e| format!("{} -> {}: {e}", from.display(), to.display()))?;
            sync_dir(dir);
            ledger.record(json!({ "step": "moved", "from": from, "to": to, "sha256": sha }));
        }
        let _ = fs::remove_file(format!("{}.sha256", from.display()));
        placed.push(json!({ "path": to, "sha256": sha }));
    }

    // Receipts go to provenance; the manifest without its download URLs.
    let prov = root.join("provenance").join(batch);
    fs::create_dir_all(&prov).map_err(|e| e.to_string())?;
    for name in receipts.iter().filter(|n| *n != "manifest.json") {
        let from = inc.join(name);
        fs::rename(&from, prov.join(name)).map_err(|e| e.to_string())?;
        let _ = fs::remove_file(format!("{}.sha256", from.display()));
        ledger.record(json!({ "step": "receipt", "file": prov.join(name) }));
    }
    // The manifest doesn't list itself; it is always kept, without its
    // download URLs.
    let mut m = manifest.clone();
    for f in m["files"].as_array_mut().into_iter().flatten() {
        if let Some(o) = f.as_object_mut() {
            o.remove("urls");
        }
    }
    let text = serde_json::to_string_pretty(&m).map_err(|e| e.to_string())?;
    let mut out = fs::File::create(prov.join("manifest.json")).map_err(|e| e.to_string())?;
    out.write_all(text.as_bytes())
        .and_then(|()| out.sync_all())
        .map_err(|e| e.to_string())?;
    let from = inc.join("manifest.json");
    fs::remove_file(&from).map_err(|e| e.to_string())?;
    let _ = fs::remove_file(format!("{}.sha256", from.display()));
    ledger.record(json!({ "step": "receipt", "file": prov.join("manifest.json") }));
    sync_dir(&prov);
    // The incoming directory is empty now; remove it (never recursively).
    let _ = fs::remove_dir(&inc);
    let _ = fs::remove_dir(root.join(INCOMING));
    let summary = json!({
        "batch": batch, "dataset": dataset, "schema": schema, "symbols": symbols,
        "files": placed, "receipts": receipts, "provenance": prov, "completed_at": tinytrader_observe::unix_seconds(),
    });
    let text = serde_json::to_string_pretty(&summary).map_err(|e| e.to_string())?;
    fs::write(
        root.join("migrations")
            .join(format!("{today}-incorporate-{batch}.json")),
        text,
    )
    .map_err(|e| e.to_string())?;
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
    fn incorporates_a_batch_into_the_standard_layout() {
        let root = scratch("batch").join("Databento");
        let inc = root.join(INCOMING).join("OPRA-1");
        let data = b"not really dbn";
        let data_sha = upload(&inc, "opra-pillar-20251001.cmbp-1.dbn.zst", data);
        let metadata = json!({ "query": { "dataset": "OPRA.PILLAR", "schema": "cmbp-1", "symbols": ["SPX.OPT"] },
                               "customizations": { "split_symbols": false, "split_duration": "day" } }).to_string();
        let meta_sha = upload(&inc, "metadata.json", metadata.as_bytes());
        let manifest = json!({ "job_id": "OPRA-1", "files": [
            { "filename": "opra-pillar-20251001.cmbp-1.dbn.zst", "size": data.len(), "hash": format!("sha256:{data_sha}"), "urls": { "https": "secret" } },
            { "filename": "metadata.json", "size": metadata.len(), "hash": format!("sha256:{meta_sha}"), "urls": { "https": "secret" } },
        ] }).to_string();
        upload(&inc, "manifest.json", manifest.as_bytes());

        let s = incorporate_databento(&root, "OPRA-1", "2026-10-06").unwrap();
        let placed = root.join("OPRA.PILLAR/SPX.OPT/cmbp-1/2025-10-01.dbn.zst");
        assert_eq!(fs::read(&placed).unwrap(), data);
        assert_eq!(s["files"][0]["sha256"], json!(data_sha));
        let kept = fs::read_to_string(root.join("provenance/OPRA-1/manifest.json")).unwrap();
        assert!(!kept.contains("secret") && kept.contains(&data_sha));
        assert!(root.join("provenance/OPRA-1/metadata.json").exists());
        assert!(!inc.exists(), "incoming batch removed once empty");
        assert!(
            root.join("migrations/2026-10-06-incorporate-OPRA-1.json")
                .exists()
        );
        let _ = fs::remove_dir_all(root.parent().unwrap());
    }

    #[test]
    fn never_overwrites_a_different_file() {
        let root = scratch("conflict").join("Databento");
        let inc = root.join(INCOMING).join("B");
        let data_sha = upload(&inc, "xnas-itch-20220610.trades.dbn.zst", b"new");
        let metadata = json!({ "query": { "dataset": "XNAS.ITCH", "schema": "trades", "symbols": ["MSFT", "AAPL"] }, "customizations": {} }).to_string();
        let meta_sha = upload(&inc, "metadata.json", metadata.as_bytes());
        let manifest = json!({ "files": [
            { "filename": "xnas-itch-20220610.trades.dbn.zst", "size": 3, "hash": format!("sha256:{data_sha}") },
            { "filename": "metadata.json", "size": metadata.len(), "hash": format!("sha256:{meta_sha}") },
        ] }).to_string();
        upload(&inc, "manifest.json", manifest.as_bytes());
        let existing = root.join("XNAS.ITCH/trades/2022-06-10.dbn.zst");
        fs::create_dir_all(existing.parent().unwrap()).unwrap();
        fs::write(&existing, b"old").unwrap();

        let err = incorporate_databento(&root, "B", "2026-10-06").unwrap_err();
        assert!(err.contains("already holds a different file"), "{err}");
        assert_eq!(fs::read(&existing).unwrap(), b"old");
        assert!(
            inc.join("xnas-itch-20220610.trades.dbn.zst").exists(),
            "nothing moved"
        );
        let _ = fs::remove_dir_all(root.parent().unwrap());
    }
}

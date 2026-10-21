//! What the store remembers between runs (decision 0019): every volume it
//! has used, so a drive that is missing when the store starts shows as
//! missing rather than as never having been there, and what each held when
//! last seen. A volume away for good is retired explicitly, which keeps a
//! record of what it held.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::ingest::sync_dir;

const SCHEMA: &str = "tinytrader.store-state/1";

/// The state file: `{ schema, volumes: { name: { path, writable,
/// require_mount, held } }, retired: [{ name, path, retired_at, held }],
/// accepted_empty: [...] }`.
#[derive(Serialize, Deserialize, Clone, Default, PartialEq, Debug)]
pub struct Remembered {
    #[serde(default)]
    pub schema: String,
    #[serde(default)]
    pub volumes: BTreeMap<String, Known>,
    #[serde(default)]
    pub retired: Vec<Value>,
    /// Volumes accepted as empty: `{ name, path, accepted_at, by,
    /// held_before }`.
    #[serde(default)]
    pub accepted_empty: Vec<Value>,
}

/// A volume the store has used.
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
pub struct Known {
    pub path: PathBuf,
    pub writable: bool,
    /// Its path must be a mount point; true unless recorded otherwise.
    #[serde(default = "yes")]
    pub require_mount: bool,
    /// Its datasets when last scanned: `{ "provider/id/symbol": { schema:
    /// { days, first, last, bytes } } }`.
    #[serde(default)]
    pub held: Value,
}

fn yes() -> bool {
    true
}

/// Whether the volume held any datasets when last read in full.
#[must_use]
pub fn held_any(k: &Known) -> bool {
    k.held.as_object().is_some_and(|o| !o.is_empty())
}

/// Where the state file is by default.
#[must_use]
pub fn default_path() -> PathBuf {
    let home = std::env::var_os("HOME").map_or_else(|| PathBuf::from("."), PathBuf::from);
    home.join("Library/Application Support/tinytrader/store-state.json")
}

/// Read the state file; a missing one is empty.
///
/// # Errors
/// If it exists but can't be read or isn't a state file.
pub fn load(path: &Path) -> Result<Remembered, String> {
    match fs::read_to_string(path) {
        Ok(text) => serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Remembered::default()),
        Err(e) => Err(format!("{}: {e}", path.display())),
    }
}

/// Replace the state file: written beside it, flushed, renamed over it, and
/// its folder flushed, so it is always one whole version.
///
/// # Errors
/// If it can't be written.
pub fn save(path: &Path, state: &Remembered) -> Result<(), String> {
    let dir = path.parent().unwrap_or(Path::new("."));
    fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let mut doc = state.clone();
    doc.schema = SCHEMA.to_owned();
    let text = serde_json::to_string_pretty(&doc).map_err(|e| e.to_string())?;
    let name = path
        .file_name()
        .map_or_else(|| "state".into(), |n| n.to_string_lossy().into_owned());
    let tmp = dir.join(format!(".{name}.new"));
    let write = || -> std::io::Result<()> {
        let mut f = fs::File::create(&tmp)?;
        std::io::Write::write_all(&mut f, text.as_bytes())?;
        f.sync_all()?;
        fs::rename(&tmp, path)?;
        sync_dir(dir)
    };
    write().map_err(|e| format!("{}: {e}", path.display()))
}

/// What a scanned root holds, in brief: for each dataset and schema, how
/// many days, the first and last, and the bytes.
#[must_use]
pub fn held(root: &Value) -> Value {
    let mut out = serde_json::Map::new();
    for d in root["datasets"].as_array().into_iter().flatten() {
        let name = [&d["provider"], &d["id"], &d["symbol"]]
            .iter()
            .map(|v| v.as_str().unwrap_or("-"))
            .collect::<Vec<_>>()
            .join("/");
        let entry = out.entry(name).or_insert_with(|| json!({}));
        for (schema, s) in d["schemas"].as_object().into_iter().flatten() {
            let days = s["days"].as_object();
            entry[schema] = json!({
                "days": days.map_or(0, serde_json::Map::len),
                "first": days.and_then(|d| d.keys().next()),
                "last": days.and_then(|d| d.keys().next_back()),
                "bytes": s["bytes"],
            });
        }
    }
    Value::Object(out)
}

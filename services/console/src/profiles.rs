//! Profile recordings: what a page in profile mode measured (widget updates,
//! fetches, stalls, DOM size; see `web/lib/profile.js`), uploaded when its
//! recording stops so they can be read on this host afterwards
//! (`scripts/profile-report`).
//!
//! `POST /api/profiles` stores one JSON recording (at most [`MAX_BYTES`]) as
//! `<id>.json` in the profiles directory and keeps the newest [`KEEP`];
//! `GET /api/profiles` lists them newest first (`id`, `at`, `user_agent`,
//! `seconds`, `bytes`); `GET /api/profiles/{id}` returns one. Ids are made
//! here (the time and a counter), so a path can't be named from outside.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use serde_json::{Value, json};

/// The largest recording accepted (the page caps its own at 5 MB).
pub const MAX_BYTES: usize = 8 << 20;
/// Recordings kept; older ones are removed when a new one is stored.
pub const KEEP: usize = 30;

pub struct Profiles {
    dir: PathBuf,
    seq: AtomicU64,
}

/// An id this module made: `p` and digits, dashes and underscores only.
fn valid(id: &str) -> bool {
    id.len() <= 64
        && id.starts_with('p')
        && id[1..]
            .chars()
            .all(|c| c.is_ascii_digit() || c == '-' || c == '_')
}

impl Profiles {
    pub fn new(dir: PathBuf) -> Self {
        Profiles {
            dir,
            seq: AtomicU64::new(0),
        }
    }

    /// Stores a recording; returns its id.
    pub fn store(&self, body: &[u8]) -> Result<String, String> {
        if body.len() > MAX_BYTES {
            return Err(format!("a recording is at most {MAX_BYTES} bytes"));
        }
        let v: Value = serde_json::from_slice(body).map_err(|e| format!("not JSON: {e}"))?;
        if !v.is_object() {
            return Err("a recording is a JSON object".into());
        }
        std::fs::create_dir_all(&self.dir).map_err(|e| e.to_string())?;
        let ms = (tinytrader_observe::unix_seconds() * 1000.0) as u64;
        let id = format!("p{ms}-{}", self.seq.fetch_add(1, Ordering::Relaxed));
        let path = self.dir.join(format!("{id}.json"));
        let tmp = self.dir.join(format!(".{id}.tmp"));
        std::fs::write(&tmp, body)
            .and_then(|_| std::fs::rename(&tmp, &path))
            .map_err(|e| e.to_string())?;
        self.prune();
        Ok(id)
    }

    /// The recordings' files, newest first.
    fn files(&self) -> Vec<(String, PathBuf, u64)> {
        let mut out: Vec<_> = std::fs::read_dir(&self.dir)
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|e| {
                let p = e.path();
                let id = p.file_name()?.to_str()?.strip_suffix(".json")?.to_owned();
                valid(&id).then(|| (id, p.clone(), e.metadata().map_or(0, |m| m.len())))
            })
            .collect();
        // p<ms>-<n>: newest first by time, then counter.
        let key = |id: &str| {
            let (ms, n) = id[1..].split_once('-').unwrap_or((&id[1..], "0"));
            (
                ms.parse::<u64>().unwrap_or(0),
                n.parse::<u64>().unwrap_or(0),
            )
        };
        out.sort_by_key(|r| std::cmp::Reverse(key(&r.0)));
        out
    }

    fn prune(&self) {
        for (_, p, _) in self.files().into_iter().skip(KEEP) {
            let _ = std::fs::remove_file(p);
        }
    }

    pub fn list(&self) -> Value {
        let rows: Vec<Value> = self
            .files()
            .into_iter()
            .map(|(id, p, bytes)| {
                let head = read_meta(&p);
                json!({
                    "id": id, "bytes": bytes,
                    "at": head.get("started_at").cloned().unwrap_or(Value::Null),
                    "user_agent": head.get("user_agent").cloned().unwrap_or(Value::Null),
                    "seconds": head.get("seconds").cloned().unwrap_or(Value::Null),
                })
            })
            .collect();
        json!({ "profiles": rows })
    }

    pub fn get(&self, id: &str) -> Option<Vec<u8>> {
        if !valid(id) {
            return None;
        }
        std::fs::read(self.dir.join(format!("{id}.json"))).ok()
    }
}

/// A recording's `meta` object (the page writes it first).
fn read_meta(p: &Path) -> serde_json::Map<String, Value> {
    std::fs::read(p)
        .ok()
        .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
        .and_then(|v| v.get("meta").and_then(|m| m.as_object().cloned()))
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("tt-profiles-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn stores_lists_and_reads() {
        let p = Profiles::new(dir("store"));
        let body = br#"{"meta":{"started_at":"2026-10-12T03:00:00Z","user_agent":"Safari","seconds":60},"events":[]}"#;
        let id = p.store(body).unwrap();
        let l = p.list();
        assert_eq!(l["profiles"][0]["id"], id);
        assert_eq!(l["profiles"][0]["user_agent"], "Safari");
        assert_eq!(l["profiles"][0]["seconds"], 60);
        assert_eq!(p.get(&id).unwrap(), body.to_vec());
    }

    #[test]
    fn keeps_the_newest() {
        let p = Profiles::new(dir("prune"));
        let ids: Vec<String> = (0..KEEP + 3).map(|_| p.store(b"{}").unwrap()).collect();
        let l = p.list();
        let listed: Vec<&str> = l["profiles"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| r["id"].as_str().unwrap())
            .collect();
        assert_eq!(listed.len(), KEEP);
        assert_eq!(listed[0], ids.last().unwrap());
        assert!(p.get(&ids[0]).is_none(), "the oldest was removed");
    }

    #[test]
    fn refuses_too_large_or_not_an_object() {
        let p = Profiles::new(dir("limit"));
        let big = format!("{{\"x\":\"{}\"}}", "a".repeat(MAX_BYTES));
        assert!(p.store(big.as_bytes()).is_err());
        assert!(p.store(b"[1,2]").is_err());
        assert!(p.store(b"not json").is_err());
    }

    #[test]
    fn ids_cannot_name_other_paths() {
        let p = Profiles::new(dir("ids"));
        for bad in ["../reminders", "p1/../../etc", "p../x", "x1", "", "p1.json"] {
            assert!(p.get(bad).is_none(), "{bad}");
        }
    }
}

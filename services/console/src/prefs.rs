//! The console's settings that follow you between devices: watchlists, views
//! and folders, alerts, the Cross-asset board, the palette and theme. The page
//! keeps them in its browser storage as before and mirrors each change here
//! (`PUT /api/prefs/{key}`); a page opening reads them all first
//! (`GET /api/prefs`), so every device starts from the same ones.
//!
//! A save names the version it started from (`If-Match`: the SHA-256 of the
//! value it last read, or `absent`), and is refused with the current value
//! when the key has changed since, so a page left open with old settings
//! can't overwrite newer ones; the page merges and saves again.
//!
//! One JSON object of key → value, written whole to a temporary file and
//! renamed over the old one. Keys are the page's own storage names
//! (`tinytrader-watchlists`); values are any JSON. Bounded: 1 MiB a value,
//! 8 MiB in all.

use std::path::PathBuf;
use std::sync::Mutex;

use serde_json::{Map, Value};
use sha2::{Digest, Sha256};

const MAX_VALUE: usize = 1 << 20;
const MAX_TOTAL: usize = 8 << 20;

pub struct Prefs {
    path: PathBuf,
    doc: Mutex<Map<String, Value>>,
}

/// Why a save was refused.
pub enum Refused {
    /// The key changed since the version the save started from: here it is now.
    Stale(Value),
    Bad(String),
}

/// A value's version: the SHA-256 of its text (a string's, as the page holds
/// it), or `absent`.
#[must_use]
pub fn version(v: Option<&Value>) -> String {
    match v {
        None | Some(Value::Null) => "absent".into(),
        Some(Value::String(s)) => format!("{:x}", Sha256::digest(s.as_bytes())),
        Some(other) => format!("{:x}", Sha256::digest(other.to_string().as_bytes())),
    }
}

/// A storage name the page uses: lower case letters, digits and dashes.
fn key_ok(k: &str) -> bool {
    !k.is_empty()
        && k.len() <= 64
        && k.bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

impl Prefs {
    /// Open the settings at `path` (an empty set if it doesn't exist yet).
    pub fn open(path: PathBuf) -> Self {
        let doc = std::fs::read(&path)
            .ok()
            .and_then(|b| serde_json::from_slice::<Map<String, Value>>(&b).ok())
            .unwrap_or_default();
        Self {
            path,
            doc: Mutex::new(doc),
        }
    }

    pub fn get(&self) -> Value {
        Value::Object(
            self.doc
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone(),
        )
    }

    /// Set one key (`null` removes it) and keep the whole set. With
    /// `based_on` (a version), only if the key is still at that version.
    ///
    /// # Errors
    /// The key changed since `based_on`; a bad key, a value or set too
    /// large, or the file can't be written.
    pub fn put(&self, key: &str, value: Value, based_on: Option<&str>) -> Result<(), Refused> {
        if !key_ok(key) {
            return Err(Refused::Bad("a key is lower case letters, digits and dashes".into()));
        }
        let size = serde_json::to_vec(&value).map_or(usize::MAX, |b| b.len());
        if size > MAX_VALUE {
            return Err(Refused::Bad(format!("{key}: more than 1 MiB")));
        }
        let mut doc = self
            .doc
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(base) = based_on
            && base != version(doc.get(key))
        {
            return Err(Refused::Stale(doc.get(key).cloned().unwrap_or(Value::Null)));
        }
        let mut next = doc.clone();
        if value.is_null() {
            next.remove(key);
        } else {
            next.insert(key.to_owned(), value);
        }
        let bad = |e: String| Refused::Bad(e);
        let bytes = serde_json::to_vec_pretty(&next).map_err(|e| bad(e.to_string()))?;
        if bytes.len() > MAX_TOTAL {
            return Err(bad("the settings would pass 8 MiB".into()));
        }
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| bad(e.to_string()))?;
        }
        let tmp = self.path.with_extension("json.tmp");
        std::fs::write(&tmp, &bytes).map_err(|e| bad(e.to_string()))?;
        std::fs::File::open(&tmp)
            .and_then(|f| f.sync_all())
            .map_err(|e| bad(e.to_string()))?;
        std::fs::rename(&tmp, &self.path).map_err(|e| bad(e.to_string()))?;
        *doc = next;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_and_reloads_settings() {
        let dir = std::env::temp_dir().join(format!("tt-prefs-{}", std::process::id()));
        let path = dir.join("prefs.json");
        let p = Prefs::open(path.clone());
        let ok = |r: Result<(), Refused>| assert!(r.is_ok());
        ok(p.put("tinytrader-watchlists", serde_json::json!([{ "name": "China AI" }]), None));
        ok(p.put("tinytrader-theme", Value::String("dark".into()), Some("absent")));
        // Saved from an old version: refused, with the current value.
        let v1 = version(Some(&Value::String("dark".into())));
        ok(p.put("tinytrader-theme", Value::String("light".into()), Some(&v1)));
        assert!(matches!(
            p.put("tinytrader-theme", Value::String("amber".into()), Some(&v1)),
            Err(Refused::Stale(Value::String(s))) if s == "light"
        ));
        ok(p.put("tinytrader-theme", Value::Null, None));
        assert!(p.put("../etc", Value::Bool(true), None).is_err());
        assert!(p.put("Bad Key", Value::Bool(true), None).is_err());
        let again = Prefs::open(path);
        assert_eq!(again.get()["tinytrader-watchlists"][0]["name"], "China AI");
        assert!(again.get().get("tinytrader-theme").is_none());
        let _ = std::fs::remove_dir_all(dir);
    }
}

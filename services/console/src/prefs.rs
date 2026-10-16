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
//!
//! **Defaults.** A second, read-only file of the same shape (such as curated
//! views kept in a deployment's repository) supplies any key the settings
//! don't have: the settings win where both do, and removing a key brings its
//! default back. A default that isn't a string is served as its JSON text, as
//! the page keeps it, so the file can hold views as readable JSON rather than
//! one long string. It is read again when it changes. `scripts/prefs-promote` writes such a file from the
//! console's current settings.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde_json::{Map, Value};
use sha2::{Digest, Sha256};

const MAX_VALUE: usize = 1 << 20;
const MAX_TOTAL: usize = 8 << 20;

pub struct Prefs {
    path: PathBuf,
    doc: Mutex<Map<String, Value>>,
    defaults: Option<Defaults>,
}

/// The defaults file, read again when it changes (a pull, a promotion).
struct Defaults {
    path: PathBuf,
    /// Its modification time and size when read, and what it held.
    held: Mutex<(Stamp, Map<String, Value>)>,
}

type Stamp = Option<(std::time::SystemTime, u64)>;

fn stamp(path: &Path) -> Stamp {
    let m = std::fs::metadata(path).ok()?;
    Some((m.modified().ok()?, m.len()))
}

impl Defaults {
    fn get(&self) -> Map<String, Value> {
        let mut held = self
            .held
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let now = stamp(&self.path);
        if now != held.0 {
            *held = (now, load_defaults(&self.path));
        }
        held.1.clone()
    }
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
    /// Open the settings at `path` (an empty set if it doesn't exist yet),
    /// over the defaults at `defaults`, if any.
    pub fn open(path: PathBuf, defaults: Option<&Path>) -> Self {
        let doc = std::fs::read(&path)
            .ok()
            .and_then(|b| serde_json::from_slice::<Map<String, Value>>(&b).ok())
            .unwrap_or_default();
        let defaults = defaults.map(|p| Defaults {
            path: p.to_owned(),
            held: Mutex::new((stamp(p), load_defaults(p))),
        });
        Self {
            path,
            doc: Mutex::new(doc),
            defaults,
        }
    }

    /// Every setting: the saved ones over the defaults.
    fn defaults(&self) -> Map<String, Value> {
        self.defaults
            .as_ref()
            .map(Defaults::get)
            .unwrap_or_default()
    }

    pub fn get(&self) -> Value {
        let mut all = self.defaults();
        all.extend(
            self.doc
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone(),
        );
        Value::Object(all)
    }

    /// Set one key (`null` removes it) and keep the whole set. With
    /// `based_on` (a version), only if the key is still at that version.
    ///
    /// # Errors
    /// The key changed since `based_on`; a bad key, a value or set too
    /// large, or the file can't be written.
    pub fn put(&self, key: &str, value: Value, based_on: Option<&str>) -> Result<(), Refused> {
        if !key_ok(key) {
            return Err(Refused::Bad(
                "a key is lower case letters, digits and dashes".into(),
            ));
        }
        let size = serde_json::to_vec(&value).map_or(usize::MAX, |b| b.len());
        if size > MAX_VALUE {
            return Err(Refused::Bad(format!("{key}: more than 1 MiB")));
        }
        let mut doc = self
            .doc
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        // A key not saved yet is at its default's version.
        let defaults = self.defaults();
        let current = doc.get(key).or_else(|| defaults.get(key));
        if let Some(base) = based_on
            && base != version(current)
        {
            return Err(Refused::Stale(current.cloned().unwrap_or(Value::Null)));
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

/// The defaults file, each value as the page holds it: a string as it is,
/// anything else as its JSON text. Unreadable: no defaults, said once.
fn load_defaults(path: &Path) -> Map<String, Value> {
    let parsed = std::fs::read(path)
        .map_err(|e| e.to_string())
        .and_then(|b| serde_json::from_slice::<Map<String, Value>>(&b).map_err(|e| e.to_string()));
    match parsed {
        Ok(m) => m
            .into_iter()
            .filter(|(k, v)| key_ok(k) && !v.is_null())
            .map(|(k, v)| match v {
                Value::String(_) => (k, v),
                other => (k, Value::String(other.to_string())),
            })
            .collect(),
        Err(e) => {
            eprintln!("console: no default settings from {}: {e}", path.display());
            Map::new()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_and_reloads_settings() {
        let dir = std::env::temp_dir().join(format!("tt-prefs-{}", std::process::id()));
        let path = dir.join("prefs.json");
        let p = Prefs::open(path.clone(), None);
        let ok = |r: Result<(), Refused>| assert!(r.is_ok());
        ok(p.put(
            "tinytrader-watchlists",
            serde_json::json!([{ "name": "China AI" }]),
            None,
        ));
        ok(p.put(
            "tinytrader-theme",
            Value::String("dark".into()),
            Some("absent"),
        ));
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
        let again = Prefs::open(path, None);
        assert_eq!(again.get()["tinytrader-watchlists"][0]["name"], "China AI");
        assert!(again.get().get("tinytrader-theme").is_none());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn defaults_fill_in_until_saved_over() {
        let dir = std::env::temp_dir().join(format!("tt-prefs-defaults-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let defaults = dir.join("layouts.json");
        std::fs::write(
            &defaults,
            r#"{ "tinytrader-workspaces": [{ "name": "Home", "tree": { "view": "search" } }],
                 "tinytrader-tree": 1, "tinytrader-theme": "dark", "Bad Key": "x", "tinytrader-gone": null }"#,
        )
        .unwrap();
        let p = Prefs::open(dir.join("prefs.json"), Some(&defaults));
        // Served as the page keeps them: JSON text.
        let all = p.get();
        let views = all["tinytrader-workspaces"].as_str().unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(views).unwrap()[0]["name"],
            "Home"
        );
        assert_eq!(all["tinytrader-tree"], "1");
        assert_eq!(all["tinytrader-theme"], "dark");
        assert!(all.get("Bad Key").is_none() && all.get("tinytrader-gone").is_none());
        // A save starts from the default's version; one from `absent` is stale.
        assert!(matches!(
            p.put("tinytrader-theme", Value::String("light".into()), Some("absent")),
            Err(Refused::Stale(Value::String(s))) if s == "dark"
        ));
        let dark = version(Some(&Value::String("dark".into())));
        assert!(
            p.put(
                "tinytrader-theme",
                Value::String("light".into()),
                Some(&dark)
            )
            .is_ok()
        );
        assert_eq!(p.get()["tinytrader-theme"], "light");
        // Only the saved setting is written, and removing it brings the default back.
        let saved: Value =
            serde_json::from_slice(&std::fs::read(dir.join("prefs.json")).unwrap()).unwrap();
        assert!(saved.get("tinytrader-workspaces").is_none());
        assert!(p.put("tinytrader-theme", Value::Null, None).is_ok());
        assert_eq!(p.get()["tinytrader-theme"], "dark");
        // Changed on disk (a pull, a promotion): read again.
        std::fs::write(&defaults, r#"{ "tinytrader-theme": "amber" }"#).unwrap();
        assert_eq!(p.get()["tinytrader-theme"], "amber");
        assert!(p.get().get("tinytrader-workspaces").is_none());
        // An unreadable defaults file: no defaults.
        std::fs::write(&defaults, "<<<<<<< HEAD").unwrap();
        let none = Prefs::open(dir.join("other.json"), Some(&defaults));
        assert_eq!(none.get(), serde_json::json!({}));
        let _ = std::fs::remove_dir_all(dir);
    }
}

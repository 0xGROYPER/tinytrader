//! Changes-only snapshots for `GET /api/state?since=G&epoch=E`.
//!
//! The snapshot is about a megabyte, mostly fields that change a few times
//! a day (an earnings calendar, news, a store's catalog), and a page polls
//! it every two seconds. Each service field's content is hashed per answer;
//! a field whose hash differs from the last one seen gets a new generation.
//! A page that passes the generation and epoch of its last answer gets every
//! top-level key as usual, but under `services` only the fields changed since
//! then, plus `_fields` (all the service's field names) so it can keep the
//! others and drop removed ones. A stale or unknown epoch (the console
//! restarted) or no `since` gets everything; `full` says which.

use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::sync::Mutex;

use serde_json::{Map, Value, json};

pub struct Delta {
    /// Distinguishes this process's generations from a previous one's.
    epoch: u64,
    inner: Mutex<Inner>,
}

#[derive(Default)]
struct Inner {
    generation: u64,
    /// (service, field) → (content hash, generation it last changed in).
    seen: HashMap<(String, String), (u64, u64)>,
}

impl Delta {
    pub fn new(epoch: u64) -> Self {
        Delta {
            epoch,
            inner: Mutex::default(),
        }
    }

    /// The answer for a page that last saw `since` in `epoch`, or the whole
    /// snapshot unchanged when it passes neither.
    pub fn answer(&self, snapshot: Value, since: Option<u64>, epoch: Option<u64>) -> Value {
        let Value::Object(mut top) = snapshot else {
            return snapshot;
        };
        let services = match top.remove("services") {
            Some(Value::Object(s)) => s,
            other => {
                if let Some(v) = other {
                    top.insert("services".into(), v);
                }
                return Value::Object(top);
            }
        };
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        // Generations by field, bumping one generation for this answer's changes.
        let next = inner.generation + 1;
        let mut changed_any = false;
        let mut gens: HashMap<(String, String), u64> = HashMap::new();
        for (svc, v) in &services {
            let Value::Object(fields) = v else { continue };
            for (name, f) in fields {
                let key = (svc.clone(), name.clone());
                let h = hash(f);
                let g = match inner.seen.get(&key) {
                    Some(&(old, g)) if old == h => g,
                    _ => {
                        changed_any = true;
                        inner.seen.insert(key.clone(), (h, next));
                        next
                    }
                };
                gens.insert(key, g);
            }
        }
        // Forget fields and services that have gone.
        inner.seen.retain(|k, _| gens.contains_key(k));
        if changed_any {
            inner.generation = next;
        }
        let generation = inner.generation;
        drop(inner);

        let Some(since) = since else {
            top.insert("services".into(), Value::Object(services));
            return Value::Object(top);
        };
        let full = epoch != Some(self.epoch) || since > generation;
        let mut out = Map::new();
        for (svc, v) in services {
            let Value::Object(fields) = v else {
                out.insert(svc, v);
                continue;
            };
            let names: Vec<Value> = fields.keys().map(|k| Value::String(k.clone())).collect();
            let mut kept = Map::new();
            for (name, f) in fields {
                if full
                    || gens
                        .get(&(svc.clone(), name.clone()))
                        .is_some_and(|&g| g > since)
                {
                    kept.insert(name, f);
                }
            }
            kept.insert("_fields".into(), Value::Array(names));
            out.insert(svc, Value::Object(kept));
        }
        top.insert("services".into(), Value::Object(out));
        top.insert("generation".into(), json!(generation));
        top.insert("epoch".into(), json!(self.epoch));
        top.insert("full".into(), json!(full));
        Value::Object(top)
    }
}

fn hash(v: &Value) -> u64 {
    let mut h = DefaultHasher::new();
    // serde_json keeps object keys sorted (no preserve_order), so equal
    // content serialises the same.
    serde_json::to_string(v).unwrap_or_default().hash(&mut h);
    h.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snap(news: &str, recent: u64) -> Value {
        json!({ "generated_at": 1, "services": {
            "alpaca-news": { "news": [news], "health": "ok" },
            "relay": { "recent": [recent], "health": "ok" },
        } })
    }

    #[test]
    fn without_since_the_snapshot_is_unchanged() {
        let d = Delta::new(7);
        assert_eq!(d.answer(snap("a", 1), None, None), snap("a", 1));
    }

    #[test]
    fn only_changed_fields_follow() {
        let d = Delta::new(7);
        let first = d.answer(snap("a", 1), Some(0), None);
        assert_eq!(first["full"], true);
        let g = first["generation"].as_u64().unwrap();
        // Recent ticks move; news doesn't.
        let next = d.answer(snap("a", 2), Some(g), Some(7));
        assert_eq!(next["full"], false);
        assert_eq!(next["services"]["relay"]["recent"], json!([2]));
        assert!(next["services"]["alpaca-news"].get("news").is_none());
        assert!(next["services"]["relay"].get("health").is_none());
        assert_eq!(
            next["services"]["alpaca-news"]["_fields"],
            json!(["health", "news"])
        );
        // Nothing changed: nothing but the names.
        let g2 = next["generation"].as_u64().unwrap();
        let same = d.answer(snap("a", 2), Some(g2), Some(7));
        assert!(same["services"]["relay"].get("recent").is_none());
        assert_eq!(same["generation"].as_u64().unwrap(), g2);
    }

    #[test]
    fn a_change_seen_by_another_page_still_reaches_this_one() {
        let d = Delta::new(7);
        let g = d.answer(snap("a", 1), Some(0), None)["generation"]
            .as_u64()
            .unwrap();
        d.answer(snap("b", 1), Some(0), None); // another page sees the news change first
        let mine = d.answer(snap("b", 1), Some(g), Some(7));
        assert_eq!(mine["services"]["alpaca-news"]["news"], json!(["b"]));
    }

    #[test]
    fn another_epoch_gets_everything() {
        let d = Delta::new(7);
        d.answer(snap("a", 1), Some(0), None);
        let v = d.answer(snap("a", 1), Some(1), Some(6));
        assert_eq!(v["full"], true);
        assert_eq!(v["services"]["alpaca-news"]["news"], json!(["a"]));
    }
}

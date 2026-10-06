//! Reminders: the roadmap as lists of items the operator edits in the
//! console, after iOS Reminders. Kept in one JSON file, written atomically
//! (a temporary file, flushed, then renamed), so an edit is never half
//! saved. The first start seeds it from a Markdown roadmap, if one is given.
//!
//! `GET /api/reminders` returns `{ lists: [{ id, name, color }], items: [{ id,
//! list, title, notes, flagged, done, done_at, created_at }] }`; `POST
//! /api/reminders` takes one change (`{ "op": … }`, see [`Reminders::apply`])
//! and returns the whole document again.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde_json::{Value, json};

/// The list colours offered, after iOS Reminders.
pub const COLORS: [&str; 7] = [
    "#FF3B30", "#FF9500", "#FFCC00", "#34C759", "#007AFF", "#AF52DE", "#A2845E",
];

pub struct Reminders {
    path: PathBuf,
    doc: Mutex<Value>,
}

fn now() -> f64 {
    tinytrader_observe::unix_seconds()
}

fn new_id(doc: &Value) -> u64 {
    let used = |key: &str| {
        doc[key]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|x| x["id"].as_u64())
            .max()
            .unwrap_or(0)
    };
    used("lists").max(used("items")) + 1
}

fn text(v: &Value, max: usize) -> Option<String> {
    v.as_str().map(|s| s.trim().chars().take(max).collect())
}

impl Reminders {
    /// Open the reminders at `path`, seeding a new file from `seed` (a
    /// Markdown roadmap) when there is none.
    pub fn open(path: PathBuf, seed: Option<&Path>) -> Self {
        let doc = std::fs::read_to_string(&path)
            .ok()
            .and_then(|t| serde_json::from_str::<Value>(&t).ok())
            .unwrap_or_else(|| {
                let doc = seed.and_then(|p| std::fs::read_to_string(p).ok()).map_or_else(
                    || json!({ "lists": [{ "id": 1, "name": "Reminders", "color": COLORS[4] }], "items": [] }),
                    |md| from_markdown(&md),
                );
                let _ = write(&path, &doc);
                doc
            });
        Self {
            path,
            doc: Mutex::new(doc),
        }
    }

    pub fn get(&self) -> Value {
        lock(&self.doc).clone()
    }

    /// Apply one change and save. Changes:
    /// `add_item { list, title, notes? }`, `update_item { id, title?, notes?,
    /// flagged?, done?, list? }`, `delete_item { id }`, `add_list { name,
    /// color? }`, `update_list { id, name?, color? }`, `delete_list { id }`
    /// (only an empty list).
    ///
    /// # Errors
    /// If the change is malformed or names something that doesn't exist, or
    /// the file can't be written (the change is then not kept).
    pub fn apply(&self, change: &Value) -> Result<Value, String> {
        let mut doc = lock(&self.doc);
        let mut next = doc.clone();
        let id = change["id"].as_u64();
        let find = |doc: &mut Value, key: &str, id: Option<u64>| -> Result<usize, String> {
            doc[key]
                .as_array()
                .and_then(|a| a.iter().position(|x| x["id"].as_u64() == id))
                .ok_or_else(|| format!("no such {}", &key[..key.len() - 1]))
        };
        match change["op"].as_str().unwrap_or_default() {
            "add_item" => {
                let list = change["list"].as_u64().ok_or("which list?")?;
                find(&mut next, "lists", Some(list))?;
                let title = text(&change["title"], 300)
                    .filter(|t| !t.is_empty())
                    .ok_or("a reminder needs a title")?;
                let item = json!({ "id": new_id(&next), "list": list, "title": title, "notes": text(&change["notes"], 4000).unwrap_or_default(),
                    "flagged": change["flagged"] == true, "done": false, "done_at": null, "created_at": now() });
                next["items"]
                    .as_array_mut()
                    .ok_or("bad document")?
                    .push(item);
            }
            "update_item" => {
                let i = find(&mut next, "items", id)?;
                if let Some(list) = change["list"].as_u64() {
                    find(&mut next, "lists", Some(list))?;
                }
                let item = &mut next["items"][i];
                if let Some(t) = text(&change["title"], 300) {
                    if t.is_empty() {
                        return Err("a reminder needs a title".into());
                    }
                    item["title"] = json!(t);
                }
                if let Some(n) = text(&change["notes"], 4000) {
                    item["notes"] = json!(n);
                }
                if let Some(f) = change["flagged"].as_bool() {
                    item["flagged"] = json!(f);
                }
                if let Some(d) = change["done"].as_bool() {
                    item["done"] = json!(d);
                    item["done_at"] = if d { json!(now()) } else { Value::Null };
                }
                if let Some(list) = change["list"].as_u64() {
                    item["list"] = json!(list);
                }
            }
            "delete_item" => {
                let i = find(&mut next, "items", id)?;
                next["items"]
                    .as_array_mut()
                    .ok_or("bad document")?
                    .remove(i);
            }
            "add_list" => {
                let name = text(&change["name"], 80)
                    .filter(|t| !t.is_empty())
                    .ok_or("a list needs a name")?;
                let n = next["lists"].as_array().map_or(0, Vec::len);
                let color = text(&change["color"], 7)
                    .filter(|c| COLORS.contains(&c.as_str()))
                    .unwrap_or_else(|| COLORS[n % COLORS.len()].to_owned());
                let list = json!({ "id": new_id(&next), "name": name, "color": color });
                next["lists"]
                    .as_array_mut()
                    .ok_or("bad document")?
                    .push(list);
            }
            "update_list" => {
                let i = find(&mut next, "lists", id)?;
                if let Some(nm) = text(&change["name"], 80).filter(|t| !t.is_empty()) {
                    next["lists"][i]["name"] = json!(nm);
                }
                if let Some(c) = text(&change["color"], 7).filter(|c| COLORS.contains(&c.as_str()))
                {
                    next["lists"][i]["color"] = json!(c);
                }
            }
            "delete_list" => {
                let i = find(&mut next, "lists", id)?;
                if next["items"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .any(|x| x["list"].as_u64() == id)
                {
                    return Err("only an empty list can be deleted".into());
                }
                next["lists"]
                    .as_array_mut()
                    .ok_or("bad document")?
                    .remove(i);
            }
            other => return Err(format!("unknown change {other:?}")),
        }
        write(&self.path, &next)?;
        *doc = next;
        Ok(doc.clone())
    }
}

/// Write the document atomically.
fn write(path: &Path, doc: &Value) -> Result<(), String> {
    use std::io::Write as _;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let tmp = path.with_extension("json.tmp");
    let mut f = std::fs::File::create(&tmp).map_err(|e| e.to_string())?;
    f.write_all(
        serde_json::to_string_pretty(doc)
            .map_err(|e| e.to_string())?
            .as_bytes(),
    )
    .map_err(|e| e.to_string())?;
    f.sync_all().map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, path).map_err(|e| e.to_string())
}

/// Lists and items from a Markdown roadmap: each `###` section (or `##`
/// section without subsections) is a list; each bullet is an item, its bold
/// label (`- **Label:** rest`) the title and the rest its notes; `- [x]`
/// items are done.
pub fn from_markdown(md: &str) -> Value {
    let mut lists: Vec<Value> = Vec::new();
    let mut items: Vec<Value> = Vec::new();
    let mut id = 0u64;
    let mut section = String::new();
    let mut current: Option<u64> = None;
    let mut open_item: Option<usize> = None;
    let start_list = |name: &str, lists: &mut Vec<Value>, id: &mut u64| {
        *id += 1;
        lists.push(json!({ "id": *id, "name": name, "color": COLORS[lists.len() % COLORS.len()] }));
        *id
    };
    for line in md.lines() {
        if let Some(h) = line.strip_prefix("### ") {
            current = Some(start_list(h.trim(), &mut lists, &mut id));
            open_item = None;
        } else if let Some(h) = line.strip_prefix("## ") {
            section = h.trim().to_owned();
            current = None;
            open_item = None;
        } else if let Some(rest) = line.strip_prefix("- ") {
            let list = match current {
                Some(l) => l,
                None => {
                    let l = start_list(
                        if section.is_empty() {
                            "Roadmap"
                        } else {
                            &section
                        },
                        &mut lists,
                        &mut id,
                    );
                    current = Some(l);
                    l
                }
            };
            let (done, rest) = if let Some(r) = rest.strip_prefix("[x] ") {
                (true, r)
            } else if let Some(r) = rest.strip_prefix("[ ] ") {
                (false, r)
            } else {
                (false, rest)
            };
            let (title, notes) = match rest.strip_prefix("**").and_then(|r| r.split_once("**")) {
                Some((label, after)) => (
                    label.trim_end_matches(':').trim_end_matches(',').to_owned(),
                    after.trim_start_matches(':').trim().to_owned(),
                ),
                None => (rest.trim().to_owned(), String::new()),
            };
            id += 1;
            items.push(json!({ "id": id, "list": list, "title": title, "notes": notes, "flagged": false, "done": done,
                "done_at": if done { json!(now()) } else { Value::Null }, "created_at": now() }));
            open_item = Some(items.len() - 1);
        } else if let Some(i) = open_item
            && line.starts_with("  ")
            && !line.trim().is_empty()
        {
            // A wrapped bullet continues its item.
            let field = if items[i]["notes"].as_str().is_some_and(|n| !n.is_empty())
                || items[i]["title"].as_str().is_some_and(|t| t.len() > 60)
            {
                "notes"
            } else {
                "title"
            };
            let joined = format!(
                "{} {}",
                items[i][field].as_str().unwrap_or_default(),
                line.trim()
            )
            .trim()
            .to_owned();
            items[i][field] = json!(joined);
        } else if line.trim().is_empty() {
            open_item = None;
        }
    }
    if lists.is_empty() {
        lists.push(json!({ "id": 1, "name": "Reminders", "color": COLORS[4] }));
    }
    json!({ "lists": lists, "items": items })
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seeds_lists_and_items_from_a_roadmap() {
        let md = "# Roadmap\n\n## Now\n\n### Relays\n\n- [x] OPRA relays.\n- [ ] Clients move to\n  the relays.\n\n## Next\n\n- **Store:** running under\n  launchd.\n";
        let d = from_markdown(md);
        let names: Vec<&str> = d["lists"]
            .as_array()
            .unwrap()
            .iter()
            .map(|l| l["name"].as_str().unwrap())
            .collect();
        assert_eq!(names, ["Relays", "Next"]);
        let items = d["items"].as_array().unwrap();
        assert_eq!(items[0]["done"], true);
        assert_eq!(items[1]["title"], "Clients move to the relays.");
        assert_eq!(items[2]["title"], "Store");
        assert_eq!(items[2]["notes"], "running under launchd.");
    }

    #[test]
    fn edits_are_saved_and_checked() {
        let dir = std::env::temp_dir().join(format!("tinytrader-reminders-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let r = Reminders::open(dir.join("r.json"), None);
        let doc = r
            .apply(&json!({ "op": "add_item", "list": 1, "title": "Buy drives" }))
            .unwrap();
        let id = doc["items"][0]["id"].as_u64().unwrap();
        r.apply(&json!({ "op": "update_item", "id": id, "done": true }))
            .unwrap();
        assert!(
            r.apply(&json!({ "op": "add_item", "list": 1, "title": "  " }))
                .is_err()
        );
        assert!(
            r.apply(&json!({ "op": "delete_list", "id": 1 })).is_err(),
            "not empty"
        );
        let reopened = Reminders::open(dir.join("r.json"), None);
        assert_eq!(reopened.get()["items"][0]["done"], true);
        let _ = std::fs::remove_dir_all(&dir);
    }
}

//! Reminders: the roadmap as lists of items the operator edits in the
//! console, after iOS Reminders. Kept in one JSON file, written atomically
//! (a temporary file, flushed, then renamed), so an edit is never half
//! saved. The first start seeds it from a Markdown roadmap, if one is given.
//!
//! The file may be kept in a git checkout and changed under the console (a
//! pull, a checkout): a change on disk is read again before the next answer
//! or edit, so an edit never writes over it. A file that can't be read (a
//! merge left half done) is never replaced: edits are refused until it's
//! fixed.
//!
//! `GET /api/reminders` returns `{ lists: [{ id, name, color }], items: [{ id,
//! list, title, notes, flagged, done, done_at, created_at }] }`; `POST
//! /api/reminders` takes one change (`{ "op": … }`, see [`Reminders::apply`])
//! and returns the whole document again.

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use tinytrader_observe::lock;

use serde_json::{Value, json};

/// The list colours offered, after iOS Reminders.
pub const COLORS: [&str; 7] = [
    "#FF3B30", "#FF9500", "#FFCC00", "#34C759", "#007AFF", "#AF52DE", "#A2845E",
];

pub struct Reminders {
    path: PathBuf,
    held: Mutex<Held>,
}

/// The document, and the file's modification time and size when it was last
/// read or written.
struct Held {
    doc: Value,
    stamp: Stamp,
    /// Why the file on disk can't be used, while it can't.
    broken: Option<String>,
}

type Stamp = Option<(std::time::SystemTime, u64)>;

fn stamp(path: &Path) -> Stamp {
    let m = std::fs::metadata(path).ok()?;
    Some((m.modified().ok()?, m.len()))
}

/// The document in the file: `None` if there is no file.
///
/// # Errors
/// The file can't be read, or isn't a reminders document.
fn read(path: &Path) -> Result<Option<Value>, String> {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("{}: {e}", path.display())),
    };
    let doc: Value = serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?;
    if !(doc["lists"].is_array() && doc["items"].is_array()) {
        return Err(format!("{}: not a reminders document", path.display()));
    }
    Ok(Some(doc))
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
    ///
    /// # Errors
    /// The file is there but can't be read: it's left as it is.
    pub fn open(path: PathBuf, seed: Option<&Path>) -> Result<Self, String> {
        let doc = if let Some(doc) = read(&path)? {
            doc
        } else {
            let doc = seed.and_then(|p| std::fs::read_to_string(p).ok()).map_or_else(
                || json!({ "lists": [{ "id": 1, "name": "Reminders", "color": COLORS[4] }], "items": [] }),
                |md| from_markdown(&md),
            );
            let _ = write(&path, &doc);
            doc
        };
        Ok(Self {
            held: Mutex::new(Held {
                doc,
                stamp: stamp(&path),
                broken: None,
            }),
            path,
        })
    }

    /// The document, read again first if the file changed on disk.
    fn current(&self) -> std::sync::MutexGuard<'_, Held> {
        let mut held = lock(&self.held);
        let now = stamp(&self.path);
        if now != held.stamp {
            match read(&self.path) {
                Ok(Some(doc)) => {
                    held.doc = doc;
                    held.broken = None;
                }
                // Gone: the next edit writes it again.
                Ok(None) => held.broken = None,
                Err(e) => {
                    eprintln!("console: reminders: {e}");
                    held.broken = Some(e);
                }
            }
            held.stamp = now;
        }
        held
    }

    pub fn get(&self) -> Value {
        self.current().doc.clone()
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
        let mut held = self.current();
        if let Some(e) = &held.broken {
            return Err(format!(
                "the reminders file can't be read, so isn't changed: {e}"
            ));
        }
        let mut next = held.doc.clone();
        let id = change["id"].as_u64();
        let find = |doc: &mut Value, key: &str, id: Option<u64>| -> Result<usize, String> {
            doc[key]
                .as_array()
                .and_then(|a| a.iter().position(|x| x["id"].as_u64() == id))
                .ok_or_else(|| format!("no such {}", &key[..key.len() - 1]))
        };
        match change["op"].as_str().unwrap_or_default() {
            "add_item" => {
                // A roadmap, not a database: a bound keeps the file small.
                if next["items"].as_array().is_some_and(|i| i.len() >= 5000) {
                    return Err(
                        "the roadmap holds at most 5000 reminders; delete some first".into(),
                    );
                }
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
        held.stamp = stamp(&self.path);
        held.doc = next;
        Ok(held.doc.clone())
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
        let r = Reminders::open(dir.join("r.json"), None).unwrap();
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
        let reopened = Reminders::open(dir.join("r.json"), None).unwrap();
        assert_eq!(reopened.get()["items"][0]["done"], true);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn follows_the_file_and_never_overwrites_a_broken_one() {
        let dir =
            std::env::temp_dir().join(format!("tinytrader-reminders-git-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("r.json");
        let r = Reminders::open(path.clone(), None).unwrap();
        r.apply(&json!({ "op": "add_item", "list": 1, "title": "Mine" }))
            .unwrap();
        // Changed on disk (a pull): read again, and the next edit builds on it.
        let mut pulled = r.get();
        pulled["items"][0]["title"] = json!("Pulled");
        std::fs::write(&path, serde_json::to_string_pretty(&pulled).unwrap()).unwrap();
        assert_eq!(r.get()["items"][0]["title"], "Pulled");
        r.apply(&json!({ "op": "add_item", "list": 1, "title": "Next" }))
            .unwrap();
        let on_disk = read(&path).unwrap().unwrap();
        assert_eq!(on_disk["items"][0]["title"], "Pulled");
        assert_eq!(on_disk["items"][1]["title"], "Next");
        // A merge left half done: edits refused, the file left alone.
        let conflicted = "<<<<<<< HEAD\n{}\n=======\n{}\n>>>>>>> other\n";
        std::fs::write(&path, conflicted).unwrap();
        assert!(
            r.apply(&json!({ "op": "add_item", "list": 1, "title": "Lost" }))
                .is_err()
        );
        assert_eq!(std::fs::read_to_string(&path).unwrap(), conflicted);
        assert!(Reminders::open(path.clone(), None).is_err());
        // Fixed: edits work again.
        std::fs::write(&path, serde_json::to_string(&on_disk).unwrap()).unwrap();
        r.apply(&json!({ "op": "delete_item", "id": on_disk["items"][1]["id"] }))
            .unwrap();
        assert_eq!(
            read(&path).unwrap().unwrap()["items"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}

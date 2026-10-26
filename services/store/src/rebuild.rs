//! Rebuild merged days' coverage as pieces (decision 0018). Merges before
//! pieces recorded a day's coverage as all its contracts over the span of
//! its inputs' windows, which claims hours some contracts don't have. Each
//! input of a merged day is on record: the import ledgers in `migrations/`
//! name the files moved to or merged into the day, and either the ledger
//! (`verified`, since pieces) or the input's receipt in `provenance/<id>/`
//! (a provider batch's `metadata.json`: its contracts and time range) says
//! what each held. A day whose inputs can't all be found is left as it is,
//! and listed.

use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

use serde_json::{Value, json};

use crate::manifest::{is_partial, read_day, read_index, set_entry};
use crate::partial::Coverage;

const DAY: u64 = 86_400_000_000_000;

/// One file that went into a day: the import it came with, and its incoming
/// path.
struct Input {
    at: f64,
    id: String,
    file: String,
}

/// Every import ledger's inputs to each day file, and the coverage each
/// input file was verified with (where the ledger records it).
fn ledgers(roots: &[PathBuf]) -> (BTreeMap<String, Vec<Input>>, BTreeMap<String, Coverage>) {
    let (mut inputs, mut verified) = (BTreeMap::<String, Vec<Input>>::new(), BTreeMap::new());
    for root in roots {
        for e in fs::read_dir(root.join("migrations"))
            .into_iter()
            .flatten()
            .flatten()
        {
            let name = e.file_name().to_string_lossy().into_owned();
            if !name.contains("-import-") || !name.ends_with(".events.jsonl") {
                continue;
            }
            let Ok(text) = fs::read_to_string(e.path()) else {
                continue;
            };
            let mut id = String::new();
            for line in text.lines() {
                let Ok(ev) = serde_json::from_str::<Value>(line) else {
                    continue;
                };
                let at = ev["at"].as_f64().unwrap_or(0.0);
                let s = |k: &Value| k.as_str().unwrap_or_default().to_owned();
                match ev["step"].as_str() {
                    Some("begin") => id = s(&ev["import"]),
                    Some("verified") => {
                        if let Some(c) = Coverage::from_json(&ev["coverage"]) {
                            verified.insert(s(&ev["file"]), c);
                        }
                    }
                    Some("moved") => inputs.entry(s(&ev["to"])).or_default().push(Input {
                        at,
                        id: id.clone(),
                        file: s(&ev["from"]),
                    }),
                    Some("merged") => inputs.entry(s(&ev["inputs"][0])).or_default().push(Input {
                        at,
                        id: id.clone(),
                        file: s(&ev["inputs"][1]),
                    }),
                    _ => {}
                }
            }
        }
    }
    for list in inputs.values_mut() {
        list.sort_by(|a, b| a.at.total_cmp(&b.at));
        // The same move recorded on every volume's copy of a ledger.
        list.dedup_by(|a, b| a.id == b.id && a.file == b.file);
    }
    (inputs, verified)
}

/// A time as nanoseconds: a number, or `YYYY-MM-DDTHH:MM:SS[.f]` in UTC.
fn ns(v: &Value) -> Option<u64> {
    if let Some(n) = v.as_u64() {
        return Some(n);
    }
    let s = v.as_str()?;
    if s.bytes().all(|b| b.is_ascii_digit()) {
        return s.parse().ok();
    }
    let mut at = crate::ingest::day_ns(s.get(..10)?)?;
    let time = s.get(10..).unwrap_or("").trim_start_matches(['T', ' ']);
    let time = time
        .strip_suffix('Z')
        .or_else(|| time.strip_suffix("+00:00"))
        .unwrap_or(time);
    if !time.is_empty() {
        let (hms, frac) = time.split_once('.').unwrap_or((time, ""));
        let mut it = hms.split(':').map(str::parse::<u64>);
        let (h, m, sec) = (
            it.next()?.ok()?,
            it.next()?.ok()?,
            it.next().unwrap_or(Ok(0)).ok()?,
        );
        let frac_ns = if frac.is_empty() {
            0
        } else {
            let digits: String = frac.chars().chain(std::iter::repeat('0')).take(9).collect();
            digits.parse::<u64>().ok()?
        };
        at += ((h * 60 + m) * 60 + sec) * 1_000_000_000 + frac_ns;
    }
    Some(at)
}

/// What a provider batch held of `day`, from its receipt: its contracts
/// over the part of its time range in the day.
fn from_receipt(roots: &[PathBuf], id: &str, day: &str) -> Option<Coverage> {
    let meta: Value = roots
        .iter()
        .map(|r| r.join("provenance").join(id).join("metadata.json"))
        .find_map(|p| fs::read_to_string(p).ok())
        .and_then(|t| serde_json::from_str(&t).ok())?;
    let q = &meta["query"];
    if q["stype_in"] != "raw_symbol" {
        return None;
    }
    let symbols = q["symbols"]
        .as_array()?
        .iter()
        .filter_map(|s| s.as_str().map(str::to_owned))
        .collect();
    let from = crate::ingest::day_ns(day)?;
    let (start, end) = (ns(&q["start"])?.max(from), ns(&q["end"])?.min(from + DAY));
    (start < end).then(|| Coverage::one(symbols, start, end))
}

/// Rebuild the coverage of every merged partial day in `folders` (schema
/// folders), from the ledgers and receipts on `roots` (provider folders).
/// Answers `{ changed: [{ folder, day, pieces_before, pieces }], unchanged,
/// not_rebuilt: [{ folder, day, why }] }`.
///
/// # Errors
/// If a manifest can't be read or written.
pub fn rebuild(roots: &[PathBuf], folders: &[PathBuf]) -> Result<Value, String> {
    let (inputs, verified) = ledgers(roots);
    let (mut changed, mut unchanged, mut left) = (Vec::new(), 0, Vec::new());
    for dir in folders {
        for (day, stub) in read_index(dir)? {
            if !is_partial(&stub) {
                continue;
            }
            let path = dir.join(format!("{day}.dbn.zst")).display().to_string();
            let Some(list) = inputs.get(&path).filter(|l| l.len() > 1) else {
                // Not merged: its coverage is its one file's header.
                continue;
            };
            let mut pieces = Vec::new();
            let mut why = None;
            for input in list {
                match verified
                    .get(&input.file)
                    .cloned()
                    .or_else(|| from_receipt(roots, &input.id, &day))
                {
                    Some(c) => pieces.push(c),
                    None => {
                        why = Some(format!(
                            "what {} held isn't on record (no coverage in its ledger, no receipt)",
                            input.id
                        ));
                        break;
                    }
                }
            }
            let Some(entry) = read_day(dir, &day)? else {
                continue;
            };
            let now = Coverage::from_json(&entry["coverage"]);
            let rebuilt = pieces.into_iter().reduce(|a, b| a.union(&b));
            let (Some(rebuilt), Some(now), None) = (rebuilt, now, &why) else {
                left.push(json!({ "folder": dir, "day": day, "why": why.unwrap_or_else(|| "its coverage is unreadable".into()) }));
                continue;
            };
            if rebuilt.symbols() != now.symbols() {
                left.push(json!({ "folder": dir, "day": day, "why": "its inputs' contracts aren't the contracts it holds" }));
                continue;
            }
            if rebuilt == now {
                unchanged += 1;
                continue;
            }
            let mut entry = entry;
            entry.as_object_mut().map(|o| o.remove("pending"));
            entry["coverage"] = rebuilt.to_json();
            set_entry(dir, &day, Some(entry))?;
            changed.push(json!({ "folder": dir, "day": day, "pieces_before": now.pieces.len(), "pieces": rebuilt.pieces.len() }));
        }
    }
    Ok(json!({ "changed": changed, "unchanged": unchanged, "not_rebuilt": left }))
}

/// The folders of `changed`, for the store to read again.
#[must_use]
pub fn folders(report: &Value) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = report["changed"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|c| c["folder"].as_str().map(PathBuf::from))
        .collect();
    out.sort();
    out.dedup();
    out
}

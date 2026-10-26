//! What's had already (decision 0019, sections 2 and 6): one set, used by
//! `/price` and `/submit` alike, of everything that already covers
//! some of a symbol's contracts over some hours. It is the union of the
//! store's holdings, every job on Databento's list that isn't expired, and
//! every purchase in the ledger, listed or not.
//!
//! Coverage is contracts × time window. Job and ledger symbols are parsed,
//! never compared as one string: a list naming the parent, or
//! `ALL_SYMBOLS`, covers the whole parent; its contracts of that parent count
//! one by one; and anything that can't be read (instrument IDs, a symbol
//! that may belong to the parent under another name) covers it all.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use databento::dbn::SType;
use databento::historical::batch::BatchJob;
use serde_json::Value;

use crate::ledger::Entry;

/// Nanoseconds since the epoch, start inclusive, end exclusive.
pub type Window = (i64, i64);

/// Every hour there is: what something whose time can't be read covers.
pub const ALWAYS: Window = (i64::MIN, i64::MAX);

const DAY_NS: i64 = 86_400 * 1_000_000_000;

/// Which of a symbol's contracts something covers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Scope {
    All,
    Contracts(BTreeSet<String>),
}

impl Scope {
    fn covers(&self, contract: &str) -> bool {
        match self {
            Self::All => true,
            Self::Contracts(c) => c.contains(contract),
        }
    }
}

/// Where a piece comes from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Kind {
    /// In the store.
    Held,
    /// A job on Databento's list that isn't expired.
    Pending,
    /// Bought here: its job is on that list, or its outcome isn't known yet.
    Bought,
    /// Bought here, and its job is expired or no longer listed: only this
    /// may be bought again, naming the job.
    Gone(String),
}

/// Something that already covers some contracts over some hours.
#[derive(Clone, Debug)]
pub struct Piece {
    pub scope: Scope,
    pub window: Window,
    pub kind: Kind,
    /// What it is, for a person.
    pub what: String,
}

/// The window of a whole UTC day.
#[must_use]
pub fn day_window(d: time::Date) -> Window {
    let start = ns(d.midnight().assume_utc());
    (start, start.saturating_add(DAY_NS))
}

#[must_use]
pub fn ns(t: time::OffsetDateTime) -> i64 {
    i64::try_from(t.unix_timestamp_nanos()).unwrap_or(i64::MAX)
}

#[must_use]
pub fn at(ns: i64) -> time::OffsetDateTime {
    time::OffsetDateTime::from_unix_timestamp_nanos(i128::from(ns))
        .unwrap_or(time::OffsetDateTime::UNIX_EPOCH)
}

/// A time as the ledger writes it: the date alone at midnight UTC, else
/// RFC 3339 to the nanosecond.
#[must_use]
pub fn stamp(ns: i64) -> String {
    let t = at(ns);
    if t.time() == time::Time::MIDNIGHT {
        return t.date().to_string();
    }
    format!(
        "{}T{:02}:{:02}:{:02}.{:09}Z",
        t.date(),
        t.hour(),
        t.minute(),
        t.second(),
        t.nanosecond()
    )
}

fn parse_stamp(s: &str) -> Option<i64> {
    let fmt = time::macros::format_description!("[year]-[month]-[day]");
    if let Ok(d) = time::Date::parse(s, fmt) {
        return Some(day_window(d).0);
    }
    time::OffsetDateTime::parse(s, &time::format_description::well_known::Rfc3339)
        .ok()
        .map(ns)
}

/// The root of an OCC option symbol (`SPY   261002C00728000`).
pub fn occ_root(s: &str) -> Option<&str> {
    let b = s.as_bytes();
    let digits = |r: std::ops::Range<usize>| b[r].iter().all(u8::is_ascii_digit);
    (b.len() == 21
        && s.is_ascii()
        && digits(6..12)
        && matches!(b[12], b'C' | b'P')
        && digits(13..21))
    .then(|| s[..6].trim_end())
}

/// The root of an option parent (`SPY.OPT`).
fn option_parent(s: &str) -> Option<&str> {
    let root = s.strip_suffix(".OPT")?;
    (!root.is_empty() && root.bytes().all(|b| b.is_ascii_alphanumeric())).then_some(root)
}

/// How much of `target` (a parent such as `SPY.OPT`, or a plain symbol) a
/// list of symbols covers; `None`: none of it. Fails closed: only symbols
/// recognised as someone else's are judged not to overlap (another option
/// parent, an OCC contract of another root, and for a plain target another
/// plain symbol). An empty name, a number (an instrument ID), `ALL_SYMBOLS`
/// and anything not recognised (a continuous `ES.c.0`, a futures contract,
/// any other dotted name) cover it all.
#[must_use]
pub fn scope_of(symbols: &[&str], target: &str) -> Option<Scope> {
    let target = target.trim().to_ascii_uppercase();
    let (root, kind) = match target.split_once('.') {
        Some((r, k)) => (r, Some(k)),
        None => (target.as_str(), None),
    };
    if symbols.is_empty() {
        return Some(Scope::All);
    }
    let mut contracts = BTreeSet::new();
    for s in symbols {
        let m = s.trim().to_ascii_uppercase();
        if m.is_empty()
            || m == "ALL_SYMBOLS"
            || m == target
            || m.bytes().all(|b| b.is_ascii_digit())
        {
            return Some(Scope::All);
        }
        if let Some(r) = occ_root(&m) {
            if r != root {
                continue;
            }
            if kind == Some("OPT") {
                contracts.insert(m);
                continue;
            }
            return Some(Scope::All);
        }
        if option_parent(&m).is_some_and(|r| r != root) {
            continue;
        }
        let other_plain = kind.is_none()
            && !m.contains('.')
            && !m.starts_with(&target)
            && !target.starts_with(&m);
        if !other_plain {
            return Some(Scope::All);
        }
    }
    (!contracts.is_empty()).then_some(Scope::Contracts(contracts))
}

/// A ledger fingerprint read back: dataset, schema, symbols and window.
#[must_use]
pub fn parse_fingerprint(f: &str) -> Option<(String, String, Vec<String>, Window)> {
    let [d, s, sym, a, b] = f.split('|').collect::<Vec<_>>()[..] else {
        return None;
    };
    let symbols = sym.split(',').map(|x| x.trim().to_owned()).collect();
    Some((
        d.to_ascii_uppercase(),
        s.to_owned(),
        symbols,
        (parse_stamp(a)?, parse_stamp(b)?),
    ))
}

/// A job's symbols as a list (Databento may answer with one comma-separated
/// string); `None` when they are instrument IDs.
fn job_symbols(j: &BatchJob) -> Option<Vec<String>> {
    if j.stype_in == SType::InstrumentId || matches!(j.symbols, databento::Symbols::Ids(_)) {
        return None;
    }
    Some(
        j.symbols
            .to_api_string()
            .split(',')
            .map(|s| s.trim().to_owned())
            .collect(),
    )
}

/// Whether a ledger request and a job are the same request.
#[must_use]
pub fn same_request(fingerprint: &str, j: &BatchJob) -> bool {
    let Some((d, s, sym, w)) = parse_fingerprint(fingerprint) else {
        return false;
    };
    let set =
        |v: &[String]| -> BTreeSet<String> { v.iter().map(|x| x.to_ascii_uppercase()).collect() };
    d == j.dataset.to_ascii_uppercase()
        && s == j.schema.as_str()
        && w == (ns(j.start), ns(j.end))
        && job_symbols(j).is_some_and(|js| set(&js) == set(&sym))
}

/// Jobs on Databento's list (not expired) that cover some of `target`.
#[must_use]
pub fn job_pieces(jobs: &[BatchJob], dataset: &str, schema: &str, target: &str) -> Vec<Piece> {
    jobs.iter()
        .filter(|j| j.dataset.eq_ignore_ascii_case(dataset) && j.schema.as_str() == schema)
        .filter_map(|j| {
            let scope = match job_symbols(j) {
                None => Scope::All,
                Some(s) => scope_of(&s.iter().map(String::as_str).collect::<Vec<_>>(), target)?,
            };
            let state = format!("{:?}", j.state).to_lowercase();
            Some(Piece {
                scope,
                window: (ns(j.start), ns(j.end)),
                kind: Kind::Pending,
                what: format!(
                    "{} ({state} on Databento; download it rather than buy it again)",
                    j.id
                ),
            })
        })
        .collect()
}

/// Purchases in the ledger (see `Ledger::purchases`) that cover some of
/// `target`. `live`: the jobs on Databento's list that aren't expired. An
/// entry that can't be read covers everything.
#[must_use]
pub fn ledger_pieces(
    entries: &[Entry],
    dataset: &str,
    schema: &str,
    target: &str,
    live: &HashSet<String>,
) -> Vec<Piece> {
    entries
        .iter()
        .filter_map(|e| {
            let (scope, window) = match parse_fingerprint(&e.request) {
                None => (Scope::All, ALWAYS),
                Some((d, s, sym, w)) => {
                    if !d.eq_ignore_ascii_case(dataset) || s != schema {
                        return None;
                    }
                    let sym: Vec<&str> = sym.iter().map(String::as_str).collect();
                    (scope_of(&sym, target)?, w)
                }
            };
            let (kind, what) = match &e.job {
                Some(j) if live.contains(j) => (Kind::Bought, format!("{j} (bought here)")),
                Some(j) => (
                    Kind::Gone(j.clone()),
                    format!("{j} (bought here; expired or no longer on Databento's list)"),
                ),
                None => (
                    Kind::Bought,
                    format!(
                        "a submission here at {} whose outcome isn't known yet ({}); it blocks until found on Databento's list or resolved (POST /ledger/resolve)",
                        tinytrader_schedule::describe((e.at.max(0.0) * 1e9) as u64),
                        e.request
                    ),
                ),
            };
            Some(Piece {
                scope,
                window,
                kind,
                what,
            })
        })
        .collect()
}

/// The store's holdings of `target` (`symbol` in the store), from its
/// `/catalog` and `/coverage` (asked for this dataset, symbol and schema over
/// `days`). Every copy of a day the catalog shows must be accounted for by
/// that day's `/coverage` row (same volume, Databento's folder, the
/// target's exact symbol); any other copy (outside Databento's folder,
/// under a symbol spelled differently or one that may include the target's,
/// on a volume the row doesn't list) counts the whole day as held.
#[must_use]
pub fn store_pieces(
    catalog: &Value,
    coverage: &Value,
    dataset: &str,
    schema: &str,
    target: &str,
    days: Window,
) -> Vec<Piece> {
    let mut out = Vec::new();
    let mut accounted: HashMap<String, HashSet<String>> = HashMap::new();
    for row in coverage["days"].as_array().into_iter().flatten() {
        let Some(day) = row["day"].as_str() else {
            out.push(held(
                Scope::All,
                ALWAYS,
                "a day /coverage lists without a date".into(),
            ));
            continue;
        };
        accounted.entry(day.to_owned()).or_default().extend(
            row["volumes"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|v| v.as_str().map(str::to_owned)),
        );
        let whole = parse_stamp(day).map_or(ALWAYS, |s| (s, s.saturating_add(DAY_NS)));
        let c = &row["coverage"];
        let what = format!(
            "{day} ({}, in the store)",
            row["kind"].as_str().unwrap_or("held")
        );
        // A complete day, or one being merged (until it settles), is held
        // in full.
        if row["kind"] == "complete" || row["pending"] == true {
            out.push(held(Scope::All, whole, what));
            continue;
        }
        // Each piece's contracts over its own window; without pieces, the
        // one list and window.
        let one = [c.clone()];
        let pieces = c["pieces"].as_array().map_or(&one[..], Vec::as_slice);
        for piece in pieces {
            let symbols: Option<Vec<&str>> = piece["symbols"]
                .as_array()
                .map(|a| a.iter().map(|s| s.as_str().unwrap_or("")).collect());
            let scope = symbols.map_or(Some(Scope::All), |s| scope_of(&s, target));
            let window = match (piece["window"][0].as_i64(), piece["window"][1].as_i64()) {
                (Some(a), Some(b)) => (a, b),
                _ => whole,
            };
            out.push(held(scope.unwrap_or(Scope::All), window, what.clone()));
        }
    }
    for root in catalog["roots"].as_array().into_iter().flatten() {
        let volume = root["name"].as_str();
        for d in root["datasets"]
            .as_array()
            .into_iter()
            .flatten()
            // Without a provider, it may be Databento's.
            .filter(|d| {
                d["provider"]
                    .as_str()
                    .is_none_or(|p| p.eq_ignore_ascii_case("databento"))
            })
            .filter(|d| {
                d["id"]
                    .as_str()
                    .is_some_and(|i| i.eq_ignore_ascii_case(dataset))
            })
        {
            let symbol = d["symbol"].as_str().unwrap_or("");
            if !symbol.eq_ignore_ascii_case(target) && scope_of(&[symbol], target).is_none() {
                continue;
            }
            let exact =
                symbol == target && d["provider"].as_str().is_some_and(|p| p == "databento");
            for day in d["schemas"][schema]["days"]
                .as_object()
                .into_iter()
                .flat_map(|m| m.keys())
            {
                let Some(start) = parse_stamp(day) else {
                    continue;
                };
                let explained = exact
                    && volume.is_some_and(|v| accounted.get(day).is_some_and(|a| a.contains(v)));
                if start < days.0 || start >= days.1 || explained {
                    continue;
                }
                let why = format!(
                    "{day} (in the store as {} {symbol} on {}, a copy /coverage doesn't account for)",
                    d["provider"].as_str().unwrap_or("no provider"),
                    volume.unwrap_or("an unnamed volume")
                );
                out.push(held(Scope::All, (start, start.saturating_add(DAY_NS)), why));
            }
        }
    }
    out
}

fn held(scope: Scope, window: Window, what: String) -> Piece {
    Piece {
        scope,
        window,
        kind: Kind::Held,
        what,
    }
}

/// What's left of `whole` after taking out `cut` (each clipped to it), in order.
#[must_use]
pub fn subtract(whole: Window, cut: &[Window]) -> Vec<Window> {
    let mut cut: Vec<Window> = cut
        .iter()
        .map(|&(a, b)| (a.max(whole.0), b.min(whole.1)))
        .filter(|(a, b)| a < b)
        .collect();
    cut.sort_unstable();
    let mut out = Vec::new();
    let mut from = whole.0;
    for (a, b) in cut {
        if a > from {
            out.push((from, a));
        }
        from = from.max(b);
    }
    if from < whole.1 {
        out.push((from, whole.1));
    }
    out
}

/// What a day needs of the contracts wanted.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Need {
    /// Contracts with nothing of the day had: bought for the whole day.
    pub whole: Vec<String>,
    /// Contracts with some hours had, by the hours they still need.
    pub hours: BTreeMap<Vec<Window>, Vec<String>>,
    /// Contracts had all day: in the store, or else pending or bought here.
    pub skipped_held: usize,
    pub skipped_pending: usize,
}

/// What `day` needs of `wanted`, given what's had.
#[must_use]
pub fn need(wanted: &BTreeSet<String>, pieces: &[Piece], day: Window) -> Need {
    let today: Vec<&Piece> = pieces
        .iter()
        .filter(|p| p.window.0 < day.1 && day.0 < p.window.1)
        .collect();
    let mut out = Need::default();
    for c in wanted {
        let on = |held_only: bool| -> Vec<Window> {
            today
                .iter()
                .filter(|p| p.scope.covers(c) && (!held_only || p.kind == Kind::Held))
                .map(|p| p.window)
                .collect()
        };
        let missing = subtract(day, &on(false));
        if missing.is_empty() {
            if subtract(day, &on(true)).is_empty() {
                out.skipped_held += 1;
            } else {
                out.skipped_pending += 1;
            }
        } else if missing == [day] {
            out.whole.push(c.clone());
        } else {
            out.hours.entry(missing).or_default().push(c.clone());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const C1: &str = "SPY   261002C00728000";
    const C2: &str = "SPY   261002P00728000";
    const Q1: &str = "QQQ   261002C00500000";

    fn d(s: &str) -> time::Date {
        time::Date::parse(s, time::macros::format_description!("[year]-[month]-[day]")).unwrap()
    }

    fn set(v: &[&str]) -> BTreeSet<String> {
        v.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn job_symbols_are_parsed_not_compared_whole() {
        let all = Some(Scope::All);
        assert_eq!(scope_of(&["SPY.OPT", "QQQ.OPT"], "SPY.OPT"), all);
        assert_eq!(scope_of(&["spy.opt"], "SPY.OPT"), all);
        assert_eq!(scope_of(&["ALL_SYMBOLS"], "SPY.OPT"), all);
        assert_eq!(scope_of(&["12345"], "SPY.OPT"), all, "an instrument ID");
        assert_eq!(scope_of(&[""], "SPY.OPT"), all, "unreadable");
        assert_eq!(scope_of(&[], "SPY.OPT"), all);
        assert_eq!(
            scope_of(&[C1, Q1], "SPY.OPT"),
            Some(Scope::Contracts(set(&[C1])))
        );
        assert_eq!(scope_of(&["QQQ.OPT", Q1], "SPY.OPT"), None);
        // Futures contracts can't be told from their names: any of the root's count.
        assert_eq!(scope_of(&["ESZ6"], "ES.FUT"), all);
        assert_eq!(scope_of(&["ES.FUT"], "ESZ6"), all);
        // Anything not recognised as someone else's covers it all.
        for m in [
            "ES.c.0",
            "SPY.n.0",
            "SPY.v.1",
            "NQZ6",
            "SPY.OPT.X",
            "SPY  2610",
        ] {
            assert_eq!(scope_of(&[m], "SPY.OPT"), all, "{m}");
        }
        assert_eq!(scope_of(&["NQZ6"], "ES.FUT"), all);
        // For an option parent, only another root's strict OCC contracts or
        // another option parent are someone else's.
        for m in ["SPY261002C00728000", "AAPL", "SPY", "QQQ   261002X00500000"] {
            assert_eq!(scope_of(&[m], "SPY.OPT"), all, "{m}");
        }
        assert_eq!(
            scope_of(&["QQQ.OPT", "QQQ   261002P00500000"], "SPY.OPT"),
            None
        );
        assert_eq!(scope_of(&["AAPL"], "TSLA"), None);
    }

    #[test]
    fn reads_fingerprints_with_dates_or_times() {
        let (ds, s, sym, w) =
            parse_fingerprint("opra.pillar|cmbp-1|SPY.OPT,QQQ.OPT|2026-10-01|2026-10-02T13:00:00Z")
                .unwrap();
        assert_eq!(
            (ds.as_str(), s.as_str(), sym.len()),
            ("OPRA.PILLAR", "cmbp-1", 2)
        );
        assert_eq!(w.1 - w.0, 37 * 3_600 * 1_000_000_000);
        assert_eq!(stamp(w.0), "2026-10-01");
        assert_eq!(parse_stamp(&stamp(w.1)), Some(w.1));
        assert!(parse_fingerprint("a|b|c").is_none());
    }

    #[test]
    fn a_catalog_day_coverage_doesnt_explain_counts_as_held_in_full() {
        let catalog = json!({ "roots": [{ "name": "v1", "datasets": [
            { "provider": "databento", "id": "OPRA.PILLAR", "symbol": "SPY.OPT", "schemas": { "cmbp-1": { "days": { "2026-10-01": {}, "2026-10-02": {} } } } },
            { "provider": "alpaca", "id": "OPRA.PILLAR", "symbol": "SPY.OPT", "schemas": { "cmbp-1": { "days": { "2026-10-03": {} } } } },
        ] }] });
        let coverage = json!({ "days": [
            { "day": "2026-10-02", "kind": "partial", "volumes": ["v1"], "coverage": { "symbols": [C1], "window": [day_window(d("2026-10-02")).0 + 13 * 3_600_000_000_000_i64, day_window(d("2026-10-02")).1] } },
        ] });
        let range = (day_window(d("2026-10-01")).0, day_window(d("2026-10-04")).0);
        let p = store_pieces(
            &catalog,
            &coverage,
            "OPRA.PILLAR",
            "cmbp-1",
            "SPY.OPT",
            range,
        );
        assert_eq!(p.len(), 2, "{p:?}");
        assert!(
            p.iter()
                .any(|x| x.scope == Scope::All && x.window == day_window(d("2026-10-01")))
        );
        assert!(p.iter().any(|x| x.scope == Scope::Contracts(set(&[C1]))));
    }

    #[test]
    fn a_copy_coverage_doesnt_account_for_counts_the_day_as_held() {
        let day = day_window(d("2026-10-02"));
        let coverage = json!({ "days": [
            { "day": "2026-10-02", "kind": "partial", "volumes": ["v1"], "coverage": { "symbols": [C1], "window": [day.0, day.1] } },
        ] });
        let copy = |volume: &str, provider: Value, symbol: &str| {
            json!({ "roots": [
            { "name": "v1", "datasets": [{ "provider": "databento", "id": "OPRA.PILLAR", "symbol": "SPY.OPT", "schemas": { "cmbp-1": { "days": { "2026-10-02": {} } } } }] },
            { "name": volume, "datasets": [{ "provider": provider, "id": "opra.pillar", "symbol": symbol, "schemas": { "cmbp-1": { "days": { "2026-10-02": {} } } } }] },
        ] })
        };
        let whole = |catalog: &Value| {
            store_pieces(catalog, &coverage, "OPRA.PILLAR", "cmbp-1", "SPY.OPT", day)
                .iter()
                .any(|p| p.scope == Scope::All)
        };
        assert!(
            !whole(&copy("v1", json!("databento"), "SPY.OPT")),
            "accounted for"
        );
        assert!(
            whole(&copy("v2", json!(null), "SPY.OPT")),
            "outside Databento's folder"
        );
        assert!(
            whole(&copy("v1", json!(null), "SPY.OPT")),
            "outside Databento's folder, even on a volume /coverage lists"
        );
        assert!(
            whole(&copy("v1", json!("databento"), "spy.opt")),
            "spelled differently"
        );
        assert!(
            whole(&copy("v2", json!("databento"), "SPY.OPT")),
            "a volume the row doesn't list"
        );
        assert!(
            !whole(&copy("v2", json!("alpaca"), "SPY.OPT")),
            "another provider's"
        );
    }

    #[test]
    fn a_partial_day_is_held_piece_by_piece() {
        let day = day_window(d("2026-10-02"));
        let h = 3_600_000_000_000_i64;
        let coverage = json!({ "days": [
            { "day": "2026-10-02", "kind": "partial", "volumes": ["v1"], "coverage": {
                "kind": "contracts", "stype": "raw_symbol",
                // The old union and hull, kept for older readers.
                "symbols": [C1, C2], "window": [day.0, day.1],
                "pieces": [
                    { "symbols": [C1], "window": [day.0, day.0 + 13 * h] },
                    { "symbols": [C2], "window": [day.0 + 13 * h, day.1] },
                ] } },
        ] });
        let p = store_pieces(
            &json!({}),
            &coverage,
            "OPRA.PILLAR",
            "cmbp-1",
            "SPY.OPT",
            day,
        );
        let n = need(&set(&[C1, C2]), &p, day);
        assert_eq!(
            n.hours.get(&vec![(day.0 + 13 * h, day.1)]),
            Some(&vec![C1.to_owned()])
        );
        assert_eq!(
            n.hours.get(&vec![(day.0, day.0 + 13 * h)]),
            Some(&vec![C2.to_owned()])
        );
        // A day being merged is held in full until it settles.
        let mut merging = coverage.clone();
        merging["days"][0]["pending"] = json!(true);
        let p = store_pieces(
            &json!({}),
            &merging,
            "OPRA.PILLAR",
            "cmbp-1",
            "SPY.OPT",
            day,
        );
        assert_eq!(need(&set(&[C1, C2, Q1]), &p, day).skipped_held, 3);
    }

    #[test]
    fn a_partial_window_leaves_the_missing_hours_to_buy() {
        let day = day_window(d("2026-10-02"));
        let h = 3_600_000_000_000_i64;
        let pieces = vec![
            held(
                Scope::Contracts(set(&[C1])),
                (day.0 + 13 * h, day.1),
                "held".into(),
            ),
            Piece {
                scope: Scope::Contracts(set(&[C2])),
                window: day,
                kind: Kind::Pending,
                what: "job".into(),
            },
        ];
        let n = need(&set(&[C1, C2, "SPY   261002C00729000"]), &pieces, day);
        assert_eq!(n.whole, vec!["SPY   261002C00729000".to_owned()]);
        assert_eq!(
            n.hours.get(&vec![(day.0, day.0 + 13 * h)]),
            Some(&vec![C1.to_owned()])
        );
        assert_eq!((n.skipped_held, n.skipped_pending), (0, 1));
        assert_eq!(
            subtract((0, 10), &[(2, 4), (3, 6), (8, 12)]),
            vec![(0, 2), (6, 8)]
        );
    }
}

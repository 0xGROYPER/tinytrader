//! What the screens show, read out of the fleet's snapshot: the streams
//! services serve are JSON, so each is read here once, into plain structs.

use serde_json::Value;
use tinytrader_fleet::{ServiceInfo, Snapshot, Status, Topology};

/// A service that is down or needs attention.
pub struct Alert {
    pub name: String,
    pub status: Status,
    pub detail: String,
}

pub fn alerts(t: &Topology, s: &Snapshot) -> Vec<Alert> {
    let mut out: Vec<Alert> = t
        .services
        .iter()
        .filter_map(|svc| {
            let st = s.services.get(&svc.id)?;
            let status = st.status();
            matches!(status, Status::Warn | Status::Crit).then(|| Alert {
                name: svc.name().to_owned(),
                status,
                detail: st.detail(),
            })
        })
        .collect();
    out.sort_by_key(|a| u8::from(a.status != Status::Crit));
    out
}

/// A bundle (decision 0011) and how it stands.
pub struct Bundle {
    pub name: String,
    pub class: String,
    pub members: usize,
    pub live: usize,
    /// The earliest "until …" any member gives, such as `Thu 09:25 ET`.
    pub next: Option<String>,
}

pub fn bundles(t: &Topology, s: &Snapshot) -> Vec<Bundle> {
    t.bundles
        .iter()
        .map(|b| {
            let members: Vec<&str> = b["members"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .collect();
            let health = |id: &str| {
                s.services
                    .get(id)
                    .and_then(|x| x.health.as_ref())
                    .and_then(|h| h.body.as_ref())
                    .cloned()
            };
            let live = members
                .iter()
                .filter(|m| {
                    health(m)
                        .and_then(|h| h.state)
                        .is_some_and(|st| st != "idle")
                })
                .count();
            let mut next: Vec<String> = members
                .iter()
                .filter_map(|m| health(m).and_then(|h| h.detail))
                .filter_map(|d| d.split_once("until ").map(|(_, u)| u.to_owned()))
                .collect();
            next.sort();
            Bundle {
                name: b["name"].as_str().unwrap_or("bundle").to_owned(),
                class: b["class"].as_str().unwrap_or("").to_owned(),
                members: members.len(),
                live,
                next: next.into_iter().next(),
            }
        })
        .collect()
}

/// A brokerage account and its positions.
pub struct Account {
    pub label: String,
    pub environment: String,
    pub equity: Option<f64>,
    pub day: Option<f64>,
    pub day_pct: Option<f64>,
    pub cash: Option<f64>,
    pub buying_power: Option<f64>,
    pub positions: Vec<Position>,
}

pub struct Position {
    pub symbol: String,
    pub qty: String,
    pub value: Option<f64>,
    pub unrealized: Option<f64>,
    pub unrealized_pct: Option<f64>,
    pub today_pct: Option<f64>,
}

fn num(v: &Value) -> Option<f64> {
    v.as_f64()
        .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
}

pub fn accounts(s: &Snapshot) -> Vec<Account> {
    let mut out: Vec<Account> = s
        .services
        .values()
        .filter_map(|st| st.accounts.as_ref())
        .flat_map(|a| a["accounts"].as_array().cloned().unwrap_or_default())
        .map(|a| {
            let acc = &a["account"];
            let (equity, last) = (num(&acc["equity"]), num(&acc["last_equity"]));
            let day = equity.zip(last).map(|(e, l)| e - l);
            Account {
                label: a["label"].as_str().unwrap_or("account").to_owned(),
                environment: a["environment"].as_str().unwrap_or("").to_owned(),
                equity,
                day,
                day_pct: day.zip(last).filter(|(_, l)| *l != 0.0).map(|(d, l)| d / l),
                cash: num(&acc["cash"]),
                buying_power: num(&acc["buying_power"]),
                positions: a["positions"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(|p| Position {
                        symbol: p["symbol"].as_str().unwrap_or("").to_owned(),
                        qty: p["qty"]
                            .as_str()
                            .map_or_else(|| p["qty"].to_string(), str::to_owned),
                        value: num(&p["market_value"]),
                        unrealized: num(&p["unrealized_pl"]),
                        unrealized_pct: num(&p["unrealized_plpc"]),
                        today_pct: num(&p["change_today"]),
                    })
                    .collect(),
            }
        })
        .collect();
    // Live first, then by name.
    out.sort_by(|a, b| {
        (a.environment != "live")
            .cmp(&(b.environment != "live"))
            .then(a.label.cmp(&b.label))
    });
    out
}

/// A price series a service serves (a relay's trades, sampled).
pub struct Series {
    pub key: String,
    pub via: String,
    pub points: Vec<f64>,
}

pub fn prices(t: &Topology, s: &Snapshot) -> Vec<Series> {
    let mut out = Vec::new();
    for svc in &t.services {
        let Some(recent) = s
            .services
            .get(&svc.id)
            .and_then(|x| x.recent.as_ref())
            .and_then(Value::as_object)
        else {
            continue;
        };
        // Accounts' equity curves are shown with the accounts.
        if s.services
            .get(&svc.id)
            .is_some_and(|x| x.accounts.is_some())
        {
            continue;
        }
        for (key, pts) in recent {
            let points: Vec<f64> = pts
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|p| p.get(1).and_then(Value::as_f64))
                .collect();
            if points.len() >= 2 {
                out.push(Series {
                    key: key.clone(),
                    via: svc.name().to_owned(),
                    points,
                });
            }
        }
    }
    out
}

/// A scheduled economic release.
pub struct Release {
    pub date: String,
    pub time: String,
    pub title: String,
    pub source: String,
    pub high: bool,
}

/// Releases from `from` (a New York date) on.
pub fn releases(s: &Snapshot, from: &str) -> Vec<Release> {
    let mut out: Vec<Release> = s
        .services
        .values()
        .filter_map(|x| x.econ.as_ref())
        .flat_map(|e| e["events"].as_array().cloned().unwrap_or_default())
        .filter_map(|e| {
            let date = e["date"].as_str()?.to_owned();
            (date.as_str() >= from).then(|| Release {
                date,
                time: e["time"].as_str().unwrap_or("").to_owned(),
                title: e["title"].as_str().unwrap_or("").to_owned(),
                source: e["source"].as_str().unwrap_or("").to_owned(),
                high: e["importance"] == "high",
            })
        })
        .collect();
    out.sort_by(|a, b| (&a.date, &a.time).cmp(&(&b.date, &b.time)));
    out
}

/// A data root the store serves, with its largest folders.
pub struct Drive {
    pub name: String,
    pub path: String,
    pub present: bool,
    pub writable: bool,
    pub used: Option<f64>,
    pub total: Option<f64>,
    pub free: Option<f64>,
    /// Room for new data below the critical threshold.
    pub room: Option<f64>,
    pub status: String,
    pub folders: Vec<(String, f64)>,
}

pub fn drives(s: &Snapshot) -> (Vec<Drive>, Vec<String>) {
    let mut out = Vec::new();
    let mut found = Vec::new();
    for c in s.services.values().filter_map(|x| x.catalog.as_ref()) {
        let critical = c["thresholds"]["critical"].as_f64().unwrap_or(90.0) / 100.0;
        for r in c["roots"].as_array().into_iter().flatten() {
            let v = &r["volume"];
            let (total, free) = (v["total_bytes"].as_f64(), v["available_bytes"].as_f64());
            let mut folders: Vec<(String, f64)> = r["datasets"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|d| {
                    let name = match d["symbol"].as_str() {
                        Some(sym) => format!("{} {sym}", d["id"].as_str().unwrap_or("")),
                        None => d["id"].as_str().unwrap_or("").to_owned(),
                    };
                    let bytes = d["schemas"]
                        .as_object()
                        .into_iter()
                        .flatten()
                        .filter_map(|(_, x)| x["bytes"].as_f64())
                        .sum();
                    (name, bytes)
                })
                .chain(r["other"].as_array().into_iter().flatten().map(|o| {
                    (
                        format!("{} (other)", o["name"].as_str().unwrap_or("")),
                        o["bytes"].as_f64().unwrap_or(0.0),
                    )
                }))
                .collect();
            folders.sort_by(|a, b| b.1.total_cmp(&a.1));
            out.push(Drive {
                name: r["name"].as_str().unwrap_or("").to_owned(),
                path: r["path"].as_str().unwrap_or("").to_owned(),
                present: r["present"] == true,
                writable: r["writable"] != false,
                used: v["used_fraction"].as_f64(),
                total,
                free,
                room: total
                    .zip(free)
                    .map(|(t, f)| (f - t * (1.0 - critical)).max(0.0)),
                status: v["status"].as_str().unwrap_or("unknown").to_owned(),
                folders,
            });
        }
        for cand in c["candidates"].as_array().into_iter().flatten() {
            found.push(cand["label"].as_str().unwrap_or("").to_owned());
        }
    }
    (out, found)
}

/// A metric's value for a service (summed over its label sets).
pub fn metric(s: &Snapshot, id: &str, name: &str) -> Option<f64> {
    let list = s
        .services
        .get(id)?
        .metrics
        .as_ref()?
        .get(name)?
        .as_array()?;
    let values: Vec<f64> = list.iter().filter_map(|m| m["value"].as_f64()).collect();
    (!values.is_empty()).then(|| values.iter().sum())
}

/// How a relay's feed is moving, from two polls' counters.
#[derive(Clone, Copy, Default)]
pub struct Rates {
    pub records_s: f64,
    pub bytes_s: f64,
    pub skips_min: f64,
    pub last_age: Option<f64>,
    pub clients: f64,
}

/// The counters rates are taken from.
#[derive(Clone, Copy)]
pub struct Counters {
    pub records: f64,
    pub bytes: f64,
    pub skips: f64,
}

pub fn counters(s: &Snapshot, id: &str) -> Option<Counters> {
    Some(Counters {
        records: metric(s, id, "tinytrader_relay_records_total")?,
        bytes: metric(s, id, "tinytrader_relay_bytes_total").unwrap_or(0.0),
        skips: metric(s, id, "tinytrader_relay_upstream_skips_total").unwrap_or(0.0),
    })
}

/// Every (service, symbol) some relay serves bars for.
pub fn chart_choices(t: &Topology) -> Vec<(String, String)> {
    t.services
        .iter()
        .filter(|s| s.bars.is_some())
        .flat_map(|s| {
            let mut symbols: Vec<String> = s
                .schedule
                .as_ref()
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .flat_map(|x| x["symbols"].as_array().cloned().unwrap_or_default())
                .filter_map(|v| v.as_str().map(str::to_owned))
                .collect();
            symbols.dedup();
            symbols
                .into_iter()
                .map(|sym| (s.id.clone(), sym))
                .collect::<Vec<_>>()
        })
        .collect()
}

/// A client connected to a relay.
pub struct Client {
    pub name: Option<String>,
    pub account: Option<String>,
    pub since: f64,
    pub replay: bool,
}

pub fn clients(s: &Snapshot, svc: &ServiceInfo) -> Vec<Client> {
    s.services
        .get(&svc.id)
        .and_then(|x| x.clients.as_ref())
        .and_then(|c| c["clients"].as_array())
        .into_iter()
        .flatten()
        .map(|c| Client {
            name: c["client"].as_str().map(str::to_owned),
            account: c["account"].as_str().map(str::to_owned),
            since: c["since"].as_f64().unwrap_or(0.0),
            replay: c["replay"] == true,
        })
        .collect()
}

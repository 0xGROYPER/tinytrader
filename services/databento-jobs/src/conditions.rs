//! Databento's day conditions for what the store holds: once at start and
//! once a day (background work, decision 0011: it waits while capture is
//! live), each Databento dataset the store holds is asked for its days'
//! conditions over the days held, and the store is told
//! (`POST /conditions/databento`). The store keeps them per day; this
//! service only reports what changed. Databento repairs degraded days and
//! releases them again, so a day can go from degraded to available: when the
//! store holds it from a batch, a fresh download may be worth it. Nothing is
//! bought here.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::time::Duration;

use databento::historical::metadata::{DatasetCondition, GetDatasetConditionParams};
use serde_json::{Value, json};

use crate::{Jobs, Shared, known_dataset, live_capture, parse_day};

/// Days asked for in one call.
const CHUNK_DAYS: i64 = 1_000;

/// What a pass did.
#[derive(Debug, Default)]
pub struct Synced {
    pub rows: usize,
    pub changed: Vec<String>,
    pub failed: Vec<String>,
}

/// Each Databento dataset the store's catalog holds (provider `databento`,
/// or none): the days held, the days held from a batch download not yet
/// filed (its `sources` name a `databento batch`), and each symbol and
/// schema it is filed under in Databento's folder.
type Held = BTreeMap<
    String,
    (
        BTreeSet<String>,
        HashSet<String>,
        BTreeSet<(Option<String>, String)>,
    ),
>;

fn held(catalog: &Value) -> Held {
    let mut out: Held = BTreeMap::new();
    for d in catalog["roots"]
        .as_array()
        .into_iter()
        .flatten()
        .flat_map(|r| r["datasets"].as_array().into_iter().flatten())
        .filter(|d| {
            d["provider"]
                .as_str()
                .is_none_or(|p| p.eq_ignore_ascii_case("databento"))
        })
    {
        let Some(id) = d["id"].as_str() else { continue };
        let batch = d["sources"]
            .as_array()
            .into_iter()
            .flatten()
            .any(|s| s.as_str().is_some_and(|s| s.starts_with("databento batch")));
        let entry = out.entry(id.to_ascii_uppercase()).or_default();
        let ours = d["provider"].as_str().is_some();
        for (schema, days) in d["schemas"]
            .as_object()
            .into_iter()
            .flatten()
            .filter_map(|(k, s)| Some((k, s["days"].as_object()?)))
        {
            if ours {
                entry
                    .2
                    .insert((d["symbol"].as_str().map(str::to_owned), schema.clone()));
            }
            for day in days.keys() {
                entry.0.insert(day.clone());
                if batch {
                    entry.1.insert(day.clone());
                }
            }
        }
    }
    out
}

fn state(c: DatasetCondition) -> Option<&'static str> {
    match c {
        DatasetCondition::Available => Some("available"),
        DatasetCondition::Degraded => Some("degraded"),
        DatasetCondition::Missing => Some("missing"),
        // Not settled yet: asked again tomorrow.
        DatasetCondition::Pending => None,
    }
}

/// One pass, unless capture is live (`None`). Failures are reported, never
/// fatal.
pub async fn sync(jobs: &Jobs) -> Option<Synced> {
    if live_capture(jobs).await.is_some() {
        return None;
    }
    let mut out = Synced::default();
    let catalog = match jobs
        .http
        .get(format!("{}/catalog", jobs.store))
        .timeout(Duration::from_secs(120))
        .send()
        .await
    {
        Ok(r) if r.status().is_success() => r.json::<Value>().await.unwrap_or_default(),
        Ok(r) => {
            out.failed
                .push(format!("the store's catalog: HTTP {}", r.status()));
            return Some(out);
        }
        Err(e) => {
            out.failed.push(format!("the store's catalog: {e}"));
            return Some(out);
        }
    };
    for (dataset, (days, mut batch, filed)) in held(&catalog) {
        // Only Databento's own datasets (not another vendor's under no provider).
        if known_dataset(jobs, &dataset).await.is_err() {
            continue;
        }
        let (Some(first), Some(last)) = (days.first(), days.last()) else {
            continue;
        };
        let last_day = last.clone();
        let (Ok(mut from), Ok(last)) = (parse_day(first), parse_day(last)) else {
            continue;
        };
        let mut rows = Vec::new();
        while from <= last {
            let to = (from + time::Duration::days(CHUNK_DAYS - 1)).min(last);
            let params = GetDatasetConditionParams::builder()
                .dataset(&dataset)
                .date_range(from..to.next_day().unwrap_or(to))
                .build();
            match jobs.db.conditions(&params).await {
                Ok(list) => rows.extend(list.into_iter().filter_map(|c| {
                    Some(json!({
                        "dataset": dataset, "day": c.date.to_string(), "state": state(c.condition)?,
                        "last_modified": c.last_modified_date.map(|d| d.to_string()),
                    }))
                })),
                Err(e) => out.failed.push(format!("{dataset} from {from}: {e}")),
            }
            from = to + time::Duration::days(1);
        }
        if rows.is_empty() {
            continue;
        }
        // Filed days whose condition the store has from a batch (read before
        // it is replaced by this one).
        for (symbol, schema) in &filed {
            let mut query = vec![
                ("provider", "databento"),
                ("dataset", dataset.as_str()),
                ("schema", schema.as_str()),
                ("start", first.as_str()),
                ("end", last_day.as_str()),
            ];
            if let Some(s) = symbol {
                query.push(("symbol", s.as_str()));
            }
            let Ok(url) =
                reqwest::Url::parse_with_params(&format!("{}/coverage", jobs.store), &query)
            else {
                continue;
            };
            let Ok(r) = jobs
                .http
                .get(url)
                .timeout(Duration::from_secs(120))
                .send()
                .await
            else {
                continue;
            };
            let v: Value = r.json().await.unwrap_or_default();
            batch.extend(
                v["days"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter(|d| {
                        d["condition"]["source"]
                            .as_str()
                            .is_some_and(|s| s.starts_with("batch "))
                    })
                    .filter_map(|d| d["day"].as_str().map(str::to_owned)),
            );
        }
        let released: BTreeMap<String, String> = rows
            .iter()
            .filter_map(|r| {
                Some((
                    r["day"].as_str()?.to_owned(),
                    r["last_modified"].as_str()?.to_owned(),
                ))
            })
            .collect();
        out.rows += rows.len();
        let sent = jobs
            .store_write(
                jobs.http
                    .post(format!("{}/conditions/databento", jobs.store)),
            )
            .json(&rows)
            .timeout(Duration::from_secs(120))
            .send()
            .await;
        let reply: Value = match sent {
            Ok(r) if r.status().is_success() => r.json().await.unwrap_or_default(),
            Ok(r) => {
                out.failed
                    .push(format!("{dataset}: the store answered {}", r.status()));
                continue;
            }
            Err(e) => {
                out.failed.push(format!("{dataset}: the store: {e}"));
                continue;
            }
        };
        for c in reply["changed"].as_array().into_iter().flatten() {
            let (day, from_state, to_state) = (
                c["day"].as_str().unwrap_or("?"),
                // `null`: no condition was recorded before.
                c["from"].as_str().unwrap_or("none"),
                c["to"].as_str().unwrap_or("unknown"),
            );
            let mut text = format!("{dataset} {day}: {from_state} → {to_state}");
            if let Some(v) = c["volume"].as_str() {
                text.push_str(&format!(" (on {v})"));
            }
            if let Some(at) = released.get(day) {
                text.push_str(&format!(", re-released {at}"));
            }
            if from_state == "degraded" && to_state == "available" && batch.contains(day) {
                text.push_str(
                    "; the store holds it from a batch, so downloading it again may be worth it",
                );
            }
            jobs.events.push(text.clone(), text.clone());
            out.changed.push(text);
        }
    }
    Some(out)
}

/// Once at start, then once a day; while capture is live, it waits and
/// tries again in 15 minutes.
pub async fn every_day(jobs: Shared) {
    loop {
        let wait = match sync(&jobs).await {
            None => Duration::from_secs(15 * 60),
            Some(s) => {
                for f in &s.failed {
                    eprintln!("databento-jobs: day conditions: {f}; tried again tomorrow");
                }
                Duration::from_secs(24 * 3600)
            }
        };
        tokio::time::sleep(wait).await;
    }
}

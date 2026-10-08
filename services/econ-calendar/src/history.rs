//! A company's past earnings, from two free sources, kept on disk:
//!
//! - **SEC EDGAR** (no daily limit; `SEC_CONTACT` names who is asking, as the
//!   SEC requires): each earnings release, a form 8-K with item 2.02, and
//!   the moment it was filed, which says whether the company reported before
//!   the open or after the close. US companies, back to about 2015.
//! - **Alpha Vantage** (`ALPHAVANTAGE_API_KEY`; 25 requests a day on the free
//!   plan): reported EPS against the analysts' estimate, and the surprise,
//!   back to the 1990s, foreign companies included.
//!
//! Served at `/history?symbol=AAPL` as the last 12 reports, newest first:
//! EDGAR's dates and times where it has them, with Alpha Vantage's EPS,
//! estimate and surprise matched to each by date (within three days).
//!
//! Everything fetched is kept (with `ECON_CACHE`, on disk across restarts):
//! Alpha Vantage's whole answer and EDGAR's filings, per company. A source is
//! asked again only when it has nothing kept for the company, or when the
//! company has reported since: [`DUE_DAYS`] after its latest report, checked
//! at most weekly. At most 20 Alpha Vantage requests are sent a day.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Duration;

use axum::http::StatusCode;
use serde_json::{Value, json};
use tinytrader_observe::{lock, unix_seconds};
use tinytrader_schedule::{ny_date_time, ny_to_unix};

/// Days after a company's latest report before the next is expected.
pub const DUE_DAYS: f64 = 80.0;
/// While a report is due, how often to look again.
const RECHECK: f64 = 7.0 * 86_400.0;
const AV_PER_DAY: u32 = 20;
const QUARTERS: usize = 12;

/// Unix seconds at noon New York time of a YYYY-MM-DD date.
fn noon(d: &str) -> Option<f64> {
    let p = |a: usize, b: usize| d.get(a..b)?.parse::<i64>().ok();
    Some(ny_to_unix(p(0, 4)?, p(5, 7)?, p(8, 10)?, 12, 0) as f64)
}

/// Whether what's kept still stands: fetched in the last week, or with its
/// latest report (the first of `dates`, newest first) too recent for another
/// to have come out since.
fn still_good(fetched_at: f64, latest: Option<&str>, now: f64) -> bool {
    now - fetched_at < RECHECK
        || latest
            .and_then(noon)
            .is_some_and(|t| now - t < DUE_DAYS * 86_400.0)
}

/// Alpha Vantage's reports, newest first, as they come.
///
/// # Errors
/// When the answer has no reports (Alpha Vantage says why in a note).
pub fn alpha_vantage(v: &Value) -> Result<Vec<Value>, String> {
    let list = v["quarterlyEarnings"].as_array().ok_or_else(|| {
        let why = v["Note"]
            .as_str()
            .or(v["Information"].as_str())
            .or(v["Error Message"].as_str());
        why.map_or_else(|| "no earnings history".to_owned(), str::to_owned)
    })?;
    let num = |x: &Value| {
        x.as_str()
            .and_then(|s| s.parse::<f64>().ok())
            .or_else(|| x.as_f64())
    };
    Ok(list
        .iter()
        .filter(|q| q["reportedDate"].as_str().is_some_and(|d| d.len() == 10))
        .map(|q| {
            json!({
                "date": q["reportedDate"], "fiscal": q["fiscalDateEnding"],
                "when": match q["reportTime"].as_str() { Some("pre-market") => "before-open", Some("post-market") => "after-close", _ => "unknown" },
                "eps": num(&q["reportedEPS"]), "estimate": num(&q["estimatedEPS"]), "surprise_pct": num(&q["surprisePercentage"]),
            })
        })
        .collect())
}

/// When an earnings release was filed (EDGAR's acceptance time, UTC), as its
/// New York date and whether that was before the open, after the close, or
/// during the session.
fn filed(accepted: &str) -> Option<(String, &'static str)> {
    let p = |a: usize, b: usize| accepted.get(a..b)?.parse::<i64>().ok();
    let t = utc_midnight(p(0, 4)?, p(5, 7)?, p(8, 10)?) + p(11, 13)? * 3600 + p(14, 16)? * 60;
    let (date, hm, _) = ny_date_time(t);
    let when = if hm.as_str() < "09:30" {
        "before-open"
    } else if hm.as_str() >= "16:00" {
        "after-close"
    } else {
        "during"
    };
    Some((date, when))
}

/// Unix seconds of midnight UTC on a date.
fn utc_midnight(y: i64, m: i64, d: i64) -> i64 {
    // Days from the civil date (Howard Hinnant's algorithm).
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (m + if m > 2 { -3 } else { 9 }) + 2) / 5 + d - 1;
    (era * 146_097 + yoe * 365 + yoe / 4 - yoe / 100 + doy - 719_468) * 86_400
}

/// The earnings releases in an EDGAR submissions answer, newest first.
#[must_use]
pub fn edgar(v: &Value) -> Vec<Value> {
    let r = &v["filings"]["recent"];
    let col = |k: &str| r[k].as_array().cloned().unwrap_or_default();
    let (form, items, accepted) = (col("form"), col("items"), col("acceptanceDateTime"));
    (0..form.len())
        .filter(|&i| {
            form[i] == "8-K"
                && items
                    .get(i)
                    .and_then(Value::as_str)
                    .is_some_and(|s| s.split(',').any(|x| x.trim() == "2.02"))
        })
        .filter_map(|i| {
            let (date, when) = filed(accepted.get(i)?.as_str()?)?;
            Some(json!({ "date": date, "when": when }))
        })
        .collect()
}

/// EDGAR's dates and times, each with Alpha Vantage's figures for the report
/// within three days of it; Alpha Vantage's alone where EDGAR has none.
#[must_use]
pub fn merge(edgar: &[Value], av: &[Value]) -> Vec<Value> {
    if edgar.is_empty() {
        return av.iter().take(QUARTERS).cloned().collect();
    }
    edgar
        .iter()
        .take(QUARTERS)
        .map(|e| {
            let t = e["date"].as_str().and_then(noon).unwrap_or(0.0);
            let m = av.iter().find(|a| {
                a["date"]
                    .as_str()
                    .and_then(noon)
                    .is_some_and(|u| (u - t).abs() <= 3.0 * 86_400.0)
            });
            json!({
                "date": e["date"], "when": e["when"],
                "fiscal": m.map_or(Value::Null, |a| a["fiscal"].clone()),
                "eps": m.map_or(Value::Null, |a| a["eps"].clone()),
                "estimate": m.map_or(Value::Null, |a| a["estimate"].clone()),
                "surprise_pct": m.map_or(Value::Null, |a| a["surprise_pct"].clone()),
            })
        })
        .collect()
}

fn symbol_ok(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 10
        && s.bytes()
            .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'.')
}

type Answer = Result<Value, (StatusCode, String)>;

pub struct History {
    http: reqwest::Client,
    key: Option<String>,
    /// The SEC's required User-Agent, with the contact address; no EDGAR without it.
    sec_agent: Option<String>,
    dir: Option<PathBuf>,
    /// What's kept, by file name (as on disk): { fetched_at, ... }.
    kept: Mutex<HashMap<String, Value>>,
    /// Alpha Vantage requests sent on a New York date.
    sent: Mutex<(String, u32)>,
}

impl History {
    #[must_use]
    pub fn new(key: Option<String>, sec_contact: Option<String>, dir: Option<PathBuf>) -> Self {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .expect("an HTTP client");
        let sec_agent = sec_contact.filter(|c| !c.is_empty()).map(|c| {
            format!(
                "tinytrader-econ-calendar/{} ({c})",
                env!("CARGO_PKG_VERSION")
            )
        });
        Self {
            http,
            key,
            sec_agent,
            dir,
            kept: Mutex::default(),
            sent: Mutex::default(),
        }
    }

    /// What's kept under `name` (memory, else disk).
    fn load(&self, name: &str) -> Option<Value> {
        if let Some(v) = lock(&self.kept).get(name) {
            return Some(v.clone());
        }
        let v: Value =
            serde_json::from_str(&std::fs::read_to_string(self.dir.as_ref()?.join(name)).ok()?)
                .ok()?;
        lock(&self.kept).insert(name.to_owned(), v.clone());
        Some(v)
    }

    /// Keep `v` under `name`, on disk too (atomically; best effort).
    fn save(&self, name: &str, v: &Value) {
        if let Some(f) = self.dir.as_ref().map(|d| d.join(name)) {
            let _ = f.parent().map(std::fs::create_dir_all);
            let tmp = f.with_extension("json.tmp");
            if std::fs::write(&tmp, v.to_string()).is_ok() {
                let _ = std::fs::rename(&tmp, &f);
            }
        }
        lock(&self.kept).insert(name.to_owned(), v.clone());
    }

    async fn get_json(&self, r: reqwest::RequestBuilder) -> Answer {
        let r = r
            .send()
            .await
            .map_err(|e| (StatusCode::BAD_GATEWAY, e.without_url().to_string()))?;
        if !r.status().is_success() {
            return Err((StatusCode::BAD_GATEWAY, format!("answered {}", r.status())));
        }
        r.json()
            .await
            .map_err(|e| (StatusCode::BAD_GATEWAY, e.without_url().to_string()))
    }

    /// Alpha Vantage's whole answer for `symbol` (its reports, newest first),
    /// fetched only when nothing is kept or a report is due.
    async fn alpha_vantage(&self, symbol: &str, now: f64) -> Result<Vec<Value>, String> {
        let name = format!("alphavantage/{symbol}.json");
        let kept = self.load(&name);
        let reports = |v: &Value| v["reports"].as_array().cloned().unwrap_or_default();
        if let Some(v) = &kept
            && still_good(
                v["fetched_at"].as_f64().unwrap_or(0.0),
                v["reports"][0]["date"].as_str(),
                now,
            )
        {
            return Ok(reports(v));
        }
        let Some(key) = &self.key else {
            return kept
                .map(|v| reports(&v))
                .ok_or_else(|| "no Alpha Vantage key".into());
        };
        {
            let today = ny_date_time(now as i64).0;
            let mut sent = lock(&self.sent);
            if sent.0 != today {
                *sent = (today, 0);
            }
            if sent.1 >= AV_PER_DAY {
                return kept
                    .map(|v| reports(&v))
                    .ok_or_else(|| "today's Alpha Vantage requests are used up".into());
            }
            sent.1 += 1;
        }
        let fetched = self
            .get_json(self.http.get("https://www.alphavantage.co/query").query(&[
                ("function", "EARNINGS"),
                ("symbol", symbol),
                ("apikey", key.as_str()),
            ]))
            .await
            .map_err(|(_, e)| e)
            .and_then(|v| alpha_vantage(&v));
        match fetched {
            Ok(list) => {
                // The whole history, kept: it only ever grows.
                self.save(
                    &name,
                    &json!({ "symbol": symbol, "fetched_at": now, "reports": list }),
                );
                Ok(list)
            }
            Err(e) => kept.map(|v| reports(&v)).ok_or(e),
        }
    }

    /// EDGAR's company number for a ticker, from its ticker list (kept a week).
    async fn cik(&self, symbol: &str, agent: &str, now: f64) -> Option<u64> {
        let name = "edgar/tickers.json";
        let mut list = self.load(name);
        if list
            .as_ref()
            .is_none_or(|v| now - v["fetched_at"].as_f64().unwrap_or(0.0) > RECHECK)
        {
            let r = self
                .http
                .get("https://www.sec.gov/files/company_tickers.json")
                .header("User-Agent", agent);
            if let Ok(v) = self.get_json(r).await {
                let map: serde_json::Map<String, Value> = v
                    .as_object()
                    .into_iter()
                    .flatten()
                    .filter_map(|(_, c)| {
                        Some((c["ticker"].as_str()?.to_owned(), c["cik_str"].clone()))
                    })
                    .collect();
                let kept = json!({ "fetched_at": now, "ciks": map });
                self.save(name, &kept);
                list = Some(kept);
            }
        }
        list?["ciks"][symbol].as_u64()
    }

    /// EDGAR's earnings releases for `symbol`, newest first (empty for a
    /// company EDGAR doesn't have them for), fetched only when due.
    async fn edgar(&self, symbol: &str, now: f64) -> Vec<Value> {
        let Some(agent) = &self.sec_agent else {
            return Vec::new();
        };
        let name = format!("edgar/{symbol}.json");
        let kept = self.load(&name);
        let releases = |v: &Value| v["releases"].as_array().cloned().unwrap_or_default();
        if let Some(v) = &kept
            && still_good(
                v["fetched_at"].as_f64().unwrap_or(0.0),
                v["releases"][0]["date"].as_str(),
                now,
            )
        {
            return releases(v);
        }
        let Some(cik) = self.cik(symbol, agent, now).await else {
            return kept.map(|v| releases(&v)).unwrap_or_default();
        };
        let r = self
            .http
            .get(format!(
                "https://data.sec.gov/submissions/CIK{cik:010}.json"
            ))
            .header("User-Agent", agent);
        match self.get_json(r).await {
            Ok(v) => {
                let list = edgar(&v);
                self.save(
                    &name,
                    &json!({ "symbol": symbol, "cik": cik, "fetched_at": now, "releases": list }),
                );
                list
            }
            Err(_) => kept.map(|v| releases(&v)).unwrap_or_default(),
        }
    }

    /// The last reports for `symbol`, merged from both sources.
    ///
    /// # Errors
    /// A bad symbol, or neither source having anything.
    pub async fn get(&self, symbol: &str) -> Answer {
        let symbol = symbol.trim().to_uppercase();
        if !symbol_ok(&symbol) {
            return Err((StatusCode::BAD_REQUEST, "symbol: a stock symbol".into()));
        }
        let now = unix_seconds();
        let ed = self.edgar(&symbol, now).await;
        let av = self.alpha_vantage(&symbol, now).await;
        if ed.is_empty() && av.as_ref().is_ok_and(Vec::is_empty) || ed.is_empty() && av.is_err() {
            let why = av.err().unwrap_or_else(|| "no earnings history".into());
            return Err((StatusCode::BAD_GATEWAY, why));
        }
        let estimates = av.as_ref().map_or(0, Vec::len);
        let reports = merge(&ed, av.as_deref().unwrap_or(&[]));
        let mut sources = Vec::new();
        if !ed.is_empty() {
            sources.push("SEC EDGAR");
        }
        if estimates > 0 {
            sources.push("Alpha Vantage");
        }
        Ok(json!({
            "symbol": symbol, "reports": reports, "sources": sources,
            "estimates": av.err().map_or_else(|| "ok".to_owned(), |e| format!("unavailable: {e}")),
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_alpha_vantage() {
        let v = json!({ "symbol": "AAPL", "quarterlyEarnings": [
            { "fiscalDateEnding": "2026-06-30", "reportedDate": "2026-07-30", "reportedEPS": "2.02", "estimatedEPS": "1.88", "surprisePercentage": "7.4468", "reportTime": "post-market" },
            { "fiscalDateEnding": "2026-03-31", "reportedDate": "2026-04-30", "reportedEPS": "2.01", "estimatedEPS": "None", "surprisePercentage": "None", "reportTime": "pre-market" },
        ]});
        let r = alpha_vantage(&v).unwrap();
        assert_eq!(
            (r[0]["when"].as_str(), r[0]["eps"].as_f64()),
            (Some("after-close"), Some(2.02))
        );
        assert!(r[1]["estimate"].is_null() && r[1]["when"] == "before-open");
        assert_eq!(
            alpha_vantage(&json!({ "Note": "limit reached" })).unwrap_err(),
            "limit reached"
        );
    }

    #[test]
    fn reads_edgar_release_times() {
        let v = json!({ "filings": { "recent": {
            "form": ["8-K", "10-Q", "8-K", "8-K"],
            "items": ["2.02,9.01", "", "5.02", "2.02"],
            // 00:30 UTC on 31 July is 20:30 in New York on the 30th; 10:45 UTC in January is 05:45.
            "acceptanceDateTime": ["2026-07-31T00:30:28.000Z", "2026-08-01T20:00:00.000Z", "2026-06-01T12:00:00.000Z", "2026-01-13T10:45:00.000Z"],
        }}});
        let r = edgar(&v);
        assert_eq!(r.len(), 2);
        assert_eq!(
            (r[0]["date"].as_str(), r[0]["when"].as_str()),
            (Some("2026-07-30"), Some("after-close"))
        );
        assert_eq!(
            (r[1]["date"].as_str(), r[1]["when"].as_str()),
            (Some("2026-01-13"), Some("before-open"))
        );
    }

    #[test]
    fn merges_dates_with_estimates() {
        let ed = [
            json!({ "date": "2026-07-30", "when": "after-close" }),
            json!({ "date": "2026-04-30", "when": "after-close" }),
        ];
        let av = [
            json!({ "date": "2026-07-31", "eps": 2.02, "estimate": 1.88, "surprise_pct": 7.4, "fiscal": "2026-06-30" }),
        ];
        let m = merge(&ed, &av);
        assert_eq!(m[0]["eps"], 2.02);
        assert!(m[1]["eps"].is_null());
        assert_eq!(merge(&[], &av).len(), 1);
    }

    #[test]
    fn keeps_until_a_new_report_is_due() {
        let day = 86_400.0;
        let now = noon("2026-10-07").unwrap();
        assert!(
            still_good(now - 2.0 * day, Some("2026-04-30"), now),
            "fetched this week"
        );
        assert!(
            still_good(now - 30.0 * day, Some("2026-07-30"), now),
            "next report not due yet"
        );
        assert!(
            !still_good(now - 30.0 * day, Some("2026-07-10"), now),
            "a report is due"
        );
        assert!(!still_good(0.0, None, now));
    }

    #[test]
    fn answers_from_what_is_kept() {
        let dir = std::env::temp_dir().join(format!("tinytrader-history-{}", std::process::id()));
        let h = History::new(None, None, Some(dir.clone()));
        let now = unix_seconds();
        h.save("alphavantage/AAPL.json", &json!({ "fetched_at": now, "reports": [{ "date": "2026-07-30", "when": "after-close", "eps": 2.02 }] }));
        // Forget memory: it must come back from disk.
        lock(&h.kept).clear();
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let v = rt.block_on(h.get("aapl")).unwrap();
        assert_eq!(v["reports"][0]["eps"], 2.02);
        assert_eq!(v["sources"], json!(["Alpha Vantage"]));
        assert_eq!(
            rt.block_on(h.get("A/B")).unwrap_err().0,
            StatusCode::BAD_REQUEST
        );
        let _ = std::fs::remove_dir_all(dir);
    }
}

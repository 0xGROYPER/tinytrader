//! A company's past earnings: each report's date and time (before the open or
//! after the close), its EPS against the estimate, and the surprise, from
//! Alpha Vantage's EARNINGS (back to the 1990s for large companies).
//!
//! Served at `/history?symbol=AAPL` as the last 12 reports, newest first.
//! Alpha Vantage's free plan allows 25 requests a day, and a history changes
//! once a quarter, so each is kept a week, in memory and (with
//! `ECON_CACHE`) on disk across restarts, and at most 20 are fetched a day.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Duration;

use axum::http::StatusCode;
use serde_json::{Value, json};
use tinytrader_observe::{lock, unix_seconds};
use tinytrader_schedule::ny_date_time;

const KEEP: f64 = 7.0 * 86_400.0;
const PER_DAY: u32 = 20;
const QUARTERS: usize = 12;

pub struct History {
    http: reqwest::Client,
    key: Option<String>,
    dir: Option<PathBuf>,
    kept: Mutex<HashMap<String, (f64, Value)>>,
    /// Requests sent on a New York date.
    sent: Mutex<(String, u32)>,
}

/// The reports in an EARNINGS answer, newest first, at most `QUARTERS`.
///
/// # Errors
/// When the answer has no reports (Alpha Vantage says why in a note).
pub fn parse(v: &Value) -> Result<Vec<Value>, String> {
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
        .take(QUARTERS)
        .map(|q| {
            json!({
                "date": q["reportedDate"], "fiscal": q["fiscalDateEnding"],
                "when": match q["reportTime"].as_str() { Some("pre-market") => "before-open", Some("post-market") => "after-close", _ => "unknown" },
                "eps": num(&q["reportedEPS"]), "estimate": num(&q["estimatedEPS"]), "surprise_pct": num(&q["surprisePercentage"]),
            })
        })
        .collect())
}

fn symbol_ok(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 10
        && s.bytes()
            .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'.')
}

impl History {
    #[must_use]
    pub fn new(key: Option<String>, dir: Option<PathBuf>) -> Self {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .expect("an HTTP client");
        Self {
            http,
            key,
            dir,
            kept: Mutex::default(),
            sent: Mutex::default(),
        }
    }

    fn file(&self, symbol: &str) -> Option<PathBuf> {
        self.dir
            .as_ref()
            .map(|d| d.join("history").join(format!("{symbol}.json")))
    }

    /// The last reports for `symbol`, from what's kept if under a week old.
    ///
    /// # Errors
    /// A bad symbol, no key, the day's requests used up, or Alpha Vantage failing.
    pub async fn get(&self, symbol: &str) -> Result<Value, (StatusCode, String)> {
        let symbol = symbol.trim().to_uppercase();
        if !symbol_ok(&symbol) {
            return Err((StatusCode::BAD_REQUEST, "symbol: a stock symbol".into()));
        }
        let now = unix_seconds();
        if let Some((at, v)) = lock(&self.kept).get(&symbol)
            && now - at < KEEP
        {
            return Ok(v.clone());
        }
        let on_disk = self
            .file(&symbol)
            .and_then(|f| std::fs::read_to_string(f).ok())
            .and_then(|t| serde_json::from_str::<Value>(&t).ok());
        if let Some(v) = &on_disk
            && v["fetched_at"].as_f64().is_some_and(|at| now - at < KEEP)
        {
            lock(&self.kept).insert(
                symbol.clone(),
                (v["fetched_at"].as_f64().unwrap_or(now), v.clone()),
            );
            return Ok(v.clone());
        }
        let Some(key) = &self.key else {
            return Err((
                StatusCode::NOT_FOUND,
                "no Alpha Vantage key (ALPHAVANTAGE_API_KEY)".into(),
            ));
        };
        {
            let today = ny_date_time(now as i64).0;
            let mut sent = lock(&self.sent);
            if sent.0 != today {
                *sent = (today, 0);
            }
            if sent.1 >= PER_DAY {
                // An older copy is better than nothing.
                return on_disk.ok_or((
                    StatusCode::TOO_MANY_REQUESTS,
                    "today's Alpha Vantage requests for histories are used up".into(),
                ));
            }
            sent.1 += 1;
        }
        let r = self
            .http
            .get("https://www.alphavantage.co/query")
            .query(&[
                ("function", "EARNINGS"),
                ("symbol", symbol.as_str()),
                ("apikey", key.as_str()),
            ])
            .send()
            .await
            .map_err(|e| (StatusCode::BAD_GATEWAY, e.without_url().to_string()))?;
        let v: Value = r
            .json()
            .await
            .map_err(|e| (StatusCode::BAD_GATEWAY, e.without_url().to_string()))?;
        let reports =
            parse(&v).map_err(|e| (StatusCode::BAD_GATEWAY, format!("Alpha Vantage: {e}")))?;
        let out = json!({ "symbol": symbol, "reports": reports, "fetched_at": now, "source": "Alpha Vantage" });
        if let Some(f) = self.file(&symbol) {
            // Best effort: the copy in memory serves either way.
            let _ = f.parent().map(std::fs::create_dir_all);
            let tmp = f.with_extension("json.tmp");
            if std::fs::write(&tmp, out.to_string()).is_ok() {
                let _ = std::fs::rename(&tmp, &f);
            }
        }
        lock(&self.kept).insert(symbol, (now, out.clone()));
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_earnings_history() {
        let v = json!({ "symbol": "AAPL", "quarterlyEarnings": [
            { "fiscalDateEnding": "2026-06-30", "reportedDate": "2026-07-30", "reportedEPS": "2.02", "estimatedEPS": "1.88", "surprise": "0.14", "surprisePercentage": "7.4468", "reportTime": "post-market" },
            { "fiscalDateEnding": "2026-03-31", "reportedDate": "2026-04-30", "reportedEPS": "2.01", "estimatedEPS": "None", "surprisePercentage": "None", "reportTime": "pre-market" },
        ]});
        let r = parse(&v).unwrap();
        assert_eq!(r.len(), 2);
        assert_eq!(r[0]["when"], "after-close");
        assert_eq!(r[0]["eps"], 2.02);
        assert!(r[1]["estimate"].is_null() && r[1]["when"] == "before-open");
        assert_eq!(
            parse(&json!({ "Note": "limit reached" })).unwrap_err(),
            "limit reached"
        );
    }

    #[test]
    fn keeps_histories_on_disk() {
        let dir = std::env::temp_dir().join(format!("tinytrader-history-{}", std::process::id()));
        let h = History::new(None, Some(dir.clone()));
        let f = h.file("AAPL").unwrap();
        std::fs::create_dir_all(f.parent().unwrap()).unwrap();
        std::fs::write(
            &f,
            json!({ "symbol": "AAPL", "reports": [], "fetched_at": unix_seconds() }).to_string(),
        )
        .unwrap();
        let rt = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();
        // No key, but a fresh copy on disk answers.
        assert_eq!(rt.block_on(h.get("aapl")).unwrap()["symbol"], "AAPL");
        assert_eq!(
            rt.block_on(h.get("A/B")).unwrap_err().0,
            StatusCode::BAD_REQUEST
        );
        let _ = std::fs::remove_dir_all(dir);
    }
}

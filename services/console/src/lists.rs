//! External lists (`CONSOLE_LISTS`): read-only tables whose rows come from
//! JSON endpoints the deployment names. Each source's URL answers a JSON
//! array of objects; the console keeps each row's configured columns, values
//! as they came, and drops the rest. A source is asked when its list is read,
//! at most once per `fresh` however many pages read it, and a failure keeps
//! the last rows it got and says so.

use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use tokio::sync::Mutex;

/// How long a source has to answer.
pub const TIMEOUT: Duration = Duration::from_secs(3);
/// How long an answer is served before the source is asked again.
pub const FRESH: Duration = Duration::from_secs(3);
/// The most of one answer that is read.
const MAX_BYTES: usize = 1 << 20;

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Column {
    key: String,
    label: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Source {
    id: String,
    label: String,
    url: String,
    columns: Vec<Column>,
}

/// A source's last answer: when it was asked, the rows of its last success
/// and when that was (Unix ms), and the last attempt's error.
#[derive(Default)]
struct Seen {
    asked: Option<Instant>,
    rows: Vec<Value>,
    updated_at: Option<u64>,
    error: Option<String>,
}

/// The configured sources (none by default).
#[derive(Default)]
pub struct Lists {
    sources: Vec<(Source, Mutex<Seen>)>,
    http: reqwest::Client,
    timeout: Duration,
    fresh: Duration,
}

impl Lists {
    /// The sources in a configuration file's JSON, checked.
    pub fn parse(text: &str, timeout: Duration, fresh: Duration) -> Result<Self, String> {
        let list: Vec<Source> = serde_json::from_str(text).map_err(|e| e.to_string())?;
        let word = |c: char| c.is_ascii_alphanumeric() || c == '-' || c == '_';
        for (i, s) in list.iter().enumerate() {
            let id = &s.id;
            if id.is_empty() || !id.chars().all(word) {
                return Err(format!("list id {id:?}: letters, digits, - and _ only"));
            }
            if list[..i].iter().any(|o| o.id == *id) {
                return Err(format!("list id {id:?} is given twice"));
            }
            if !(s.url.starts_with("http://") || s.url.starts_with("https://")) {
                return Err(format!("list {id}: the url must be http:// or https://"));
            }
            if s.columns.is_empty() {
                return Err(format!("list {id}: no columns"));
            }
        }
        let sources = list.into_iter().map(|s| (s, Mutex::default())).collect();
        Ok(Self {
            sources,
            timeout,
            fresh,
            ..Self::default()
        })
    }

    /// Every list, in the configured order, the sources asked together.
    pub async fn all(&self) -> Vec<Value> {
        let lists = self.sources.iter().map(|(s, seen)| self.answer(s, seen));
        futures_util::future::join_all(lists).await
    }

    /// One list, or `None` if no source has this id.
    pub async fn one(&self, id: &str) -> Option<Value> {
        let (s, seen) = self.sources.iter().find(|(s, _)| s.id == id)?;
        Some(self.answer(s, seen).await)
    }

    /// A source's list, asking the source again once its last answer is
    /// older than `fresh`. Pages reading it meanwhile wait for that request.
    async fn answer(&self, s: &Source, seen: &Mutex<Seen>) -> Value {
        let mut seen = seen.lock().await;
        if seen.asked.is_none_or(|t| t.elapsed() >= self.fresh) {
            seen.asked = Some(Instant::now());
            match self.fetch(s).await {
                Ok(rows) => {
                    seen.rows = rows;
                    seen.updated_at = Some(now_ms());
                    seen.error = None;
                }
                Err(e) => seen.error = Some(e),
            }
        }
        json!({
            "id": s.id,
            "label": s.label,
            "columns": s.columns,
            "rows": seen.rows,
            "updated_at": seen.updated_at,
            "age_ms": seen.updated_at.map(|t| now_ms().saturating_sub(t)),
            "error": seen.error,
        })
    }

    /// The source's rows, each with only the configured columns.
    async fn fetch(&self, s: &Source) -> Result<Vec<Value>, String> {
        let why = |e: reqwest::Error| {
            if e.is_timeout() {
                format!("no answer within {} ms", self.timeout.as_millis())
            } else if e.is_connect() {
                "connection refused or host unreachable".to_owned()
            } else {
                e.without_url().to_string()
            }
        };
        let mut r = self
            .http
            .get(&s.url)
            .timeout(self.timeout)
            .send()
            .await
            .map_err(why)?;
        if !r.status().is_success() {
            return Err(format!("the source answered {}", r.status()));
        }
        let mut body = Vec::new();
        while let Some(chunk) = r.chunk().await.map_err(why)? {
            body.extend_from_slice(&chunk);
            if body.len() > MAX_BYTES {
                return Err("the source answered more than 1 MiB".into());
            }
        }
        let not_rows = || "the source didn't answer a JSON array of objects".to_owned();
        let Ok(Value::Array(rows)) = serde_json::from_slice::<Value>(&body) else {
            return Err(not_rows());
        };
        let mut kept = Vec::with_capacity(rows.len());
        for row in rows {
            let Value::Object(mut row) = row else {
                return Err(not_rows());
            };
            let mut out = Map::new();
            for c in &s.columns {
                if let Some(v) = row.remove(&c.key) {
                    out.insert(c.key.clone(), v);
                }
            }
            kept.push(Value::Object(out));
        }
        Ok(kept)
    }
}

fn now_ms() -> u64 {
    (tinytrader_observe::unix_seconds() * 1000.0) as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    const COLUMNS: &str = r#"[{"key":"k","label":"K"}]"#;

    #[test]
    fn a_configuration_is_checked() {
        let one = |id: &str, url: &str, columns: &str| {
            format!(r#"[{{"id":"{id}","label":"L","url":"{url}","columns":{columns}}}]"#)
        };
        let parse = |t: String| Lists::parse(&t, TIMEOUT, FRESH).map(|l| l.sources.len());
        let ok = one("a-1", "https://127.0.0.1:1/x", COLUMNS);
        assert_eq!(parse(ok.clone()), Ok(1));
        assert_eq!(parse("[]".into()), Ok(0));
        let bad = [
            format!("[{0}, {0}]", &ok[1..ok.len() - 1]),
            one("a b", "http://x", COLUMNS),
            one("", "http://x", COLUMNS),
            one("a", "file:///etc/hosts", COLUMNS),
            one("a", "http://x", "[]"),
            one("a", "http://x", r#"[{"key":"k"}]"#),
            format!(r#"[{{"id":"a","label":"L","url":"http://x","columns":{COLUMNS},"x":1}}]"#),
            "{}".into(),
        ];
        for t in bad {
            assert!(parse(t.clone()).is_err(), "{t}");
        }
    }
}

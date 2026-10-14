//! Asking Alpaca: one shared pace for every request, and a day's page chain
//! for one lane (trades or quotes), each page with its receipt.
//!
//! The pace (decision 0014) stays below both the configured rate and what
//! Alpaca advertises, waits out the window when `x-ratelimit-remaining`
//! reaches a small reserve, and honours `Retry-After`. A 429 is never
//! success: it pauses the pace, and three in a row stop the day as rate
//! limited (the run resumes later). Server errors and dropped connections are
//! retried with a doubling backoff; anything else is the day's failure.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tokio::sync::Mutex;

/// Requests left in Alpaca's window below which the pace waits for the reset.
const RESERVE: u64 = 6;
const RETRIES: u32 = 5;

/// Why a lane stopped.
#[derive(Debug)]
pub enum Failure {
    /// Three 429s in a row: try again later.
    RateLimited,
    /// Alpaca refused the request itself (401, 403, 404, a bad request).
    Refused(String),
    /// A page didn't make sense (unreadable, out of order, outside the day).
    PageIntegrity(String),
    /// The network or Alpaca kept failing.
    Unavailable(String),
}

impl std::fmt::Display for Failure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::RateLimited => write!(f, "rate limited (three 429s in a row)"),
            Self::Refused(e) => write!(f, "refused: {e}"),
            Self::PageIntegrity(e) => write!(f, "page integrity: {e}"),
            Self::Unavailable(e) => write!(f, "unavailable: {e}"),
        }
    }
}

/// One pace for every request the service makes.
pub struct Pace {
    gap: Duration,
    state: Mutex<PaceState>,
    /// Requests made, and 429s seen, for `/metrics`.
    pub requests: Arc<AtomicU64>,
    pub limited: Arc<AtomicU64>,
}

struct PaceState {
    next: Instant,
    in_a_row: u32,
}

impl Pace {
    pub fn new(per_minute: u32) -> Self {
        Self {
            gap: Duration::from_secs_f64(60.0 / f64::from(per_minute.max(1))),
            state: Mutex::new(PaceState {
                next: Instant::now(),
                in_a_row: 0,
            }),
            requests: Arc::default(),
            limited: Arc::default(),
        }
    }

    async fn turn(&self) {
        let wait = {
            let mut s = self.state.lock().await;
            let now = Instant::now();
            let at = s.next.max(now);
            s.next = at + self.gap;
            self.requests.fetch_add(1, Ordering::Relaxed);
            at - now
        };
        tokio::time::sleep(wait).await;
    }

    /// After an answer: wait out the window when it's nearly spent.
    async fn heard(&self, resp: &reqwest::Response) {
        let header = |n: &str| {
            resp.headers()
                .get(n)
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.trim().parse::<u64>().ok())
        };
        let mut s = self.state.lock().await;
        if resp.status() != reqwest::StatusCode::TOO_MANY_REQUESTS {
            s.in_a_row = 0;
        }
        if let (Some(left), Some(reset)) =
            (header("x-ratelimit-remaining"), header("x-ratelimit-reset"))
            && left <= RESERVE
        {
            let now = tinytrader_observe::unix_seconds();
            let secs = (reset as f64 - now).clamp(0.0, 60.0);
            s.next = s.next.max(Instant::now() + Duration::from_secs_f64(secs + 0.5));
        }
    }

    /// A 429: pause; the third in a row stops.
    async fn limited(&self, retry_after: Option<u64>) -> Result<(), Failure> {
        let mut s = self.state.lock().await;
        self.limited.fetch_add(1, Ordering::Relaxed);
        s.in_a_row += 1;
        let pause = Duration::from_secs(retry_after.unwrap_or(5).clamp(1, 60));
        s.next = s.next.max(Instant::now() + pause);
        if s.in_a_row >= 3 {
            s.in_a_row = 0;
            return Err(Failure::RateLimited);
        }
        Ok(())
    }
}

/// How to reach Alpaca.
pub struct Alpaca {
    pub http: reqwest::Client,
    pub data: String,
    pub key_id: String,
    pub secret: String,
    pub pace: Pace,
}

impl Alpaca {
    /// One GET, paced and retried: the body as received.
    pub async fn get(&self, url: &str) -> Result<Vec<u8>, Failure> {
        let mut backoff = Duration::from_millis(500);
        let mut last = String::new();
        for _ in 0..RETRIES {
            self.pace.turn().await;
            let sent = self
                .http
                .get(url)
                .header("APCA-API-KEY-ID", &self.key_id)
                .header("APCA-API-SECRET-KEY", &self.secret)
                .timeout(Duration::from_secs(60))
                .send()
                .await;
            match sent {
                Ok(resp) => {
                    self.pace.heard(&resp).await;
                    let status = resp.status();
                    if status.is_success() {
                        match resp.bytes().await {
                            Ok(b) => return Ok(b.to_vec()),
                            Err(e) => last = format!("reading the answer: {e}"),
                        }
                    } else if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
                        let after = resp
                            .headers()
                            .get("retry-after")
                            .and_then(|v| v.to_str().ok())
                            .and_then(|v| v.parse().ok());
                        self.pace.limited(after).await?;
                        continue;
                    } else if status.is_server_error() {
                        last = format!("Alpaca answered {status}");
                    } else {
                        let body = resp.text().await.unwrap_or_default();
                        return Err(Failure::Refused(format!(
                            "{status}: {}",
                            body.chars().take(200).collect::<String>()
                        )));
                    }
                }
                Err(e) => last = e.to_string(),
            }
            tokio::time::sleep(backoff).await;
            backoff = (backoff * 2).min(Duration::from_secs(30));
        }
        Err(Failure::Unavailable(last))
    }
}

/// A lane's pages, decoded: its rows (by the caller's parser), and receipts.
pub struct Lane<T> {
    pub rows: Vec<T>,
    pub receipts: Vec<Value>,
}

/// `YYYY-MM-DD` → nanoseconds at 00:00 UTC.
pub fn day_ns(day: &str) -> Option<u64> {
    let d = time::Date::parse(day, time::macros::format_description!("[year]-[month]-[day]")).ok()?;
    let s = d.midnight().assume_utc().unix_timestamp();
    u64::try_from(s).ok().map(|s| s * 1_000_000_000)
}

/// Fetch one lane of one symbol-day: every page from the first to the one
/// without a next token. `parse` turns a page's body into rows and the
/// times of its first and last row; rows must stay in time order and within
/// the day.
pub async fn lane<T>(
    alpaca: &Alpaca,
    feed: &str,
    symbol: &str,
    kind: &str,
    day: &str,
    parse: impl Fn(&[u8]) -> Result<(Vec<T>, Option<String>), String>,
    ts: impl Fn(&T) -> u64,
) -> Result<Lane<T>, Failure> {
    let start = day_ns(day).ok_or_else(|| Failure::PageIntegrity(format!("bad day {day}")))?;
    let end = start + 86_400 * 1_000_000_000;
    let base = format!(
        "{}/v2/stocks/{symbol}/{kind}?start={day}T00:00:00Z&end={day}T23:59:59.999999999Z&limit=10000&sort=asc&feed={feed}",
        alpaca.data
    );
    let mut out = Lane {
        rows: Vec::new(),
        receipts: Vec::new(),
    };
    let mut token: Option<String> = None;
    let mut prev = start;
    loop {
        let url = match &token {
            Some(t) => format!("{base}&page_token={t}"),
            None => base.clone(),
        };
        let body = alpaca.get(&url).await?;
        let (rows, next) = parse(&body).map_err(|e| {
            Failure::PageIntegrity(format!("{kind} page {}: {e}", out.receipts.len()))
        })?;
        let (first, last) = (rows.first().map(&ts), rows.last().map(&ts));
        for r in &rows {
            let t = ts(r);
            if t < prev || t >= end {
                return Err(Failure::PageIntegrity(format!(
                    "{kind} page {}: a row at {t} is out of order or outside {day}",
                    out.receipts.len()
                )));
            }
            prev = t;
        }
        if let Some(t) = &next
            && (t.is_empty() || t.len() > 256 || !t.bytes().all(|b| b.is_ascii_graphic()))
        {
            return Err(Failure::PageIntegrity(format!("{kind}: an unusable page token")));
        }
        out.receipts.push(json!({
            "page": out.receipts.len(), "rows": rows.len(), "first_ns": first, "last_ns": last,
            "bytes": body.len(), "sha256": format!("{:x}", Sha256::digest(&body)), "continues": next.is_some(),
        }));
        out.rows.extend(rows);
        match next {
            // The same token again would never end.
            Some(t) if token.as_deref() == Some(t.as_str()) => {
                return Err(Failure::PageIntegrity(format!("{kind}: the same page token twice")));
            }
            Some(t) => token = Some(t),
            None => return Ok(out),
        }
    }
}

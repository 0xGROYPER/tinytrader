//! Example service: polls Alpaca's market news (supplied by Benzinga, free
//! with an Alpaca account) over REST and serves recent stories.
//!
//! Polling over REST, rather than streaming, keeps it independent of
//! Alpaca's one-stream-per-account limit for market data.
//!
//! Serves `/news` (`{ "items": [...], "updated_at": unix_seconds }`, newest
//! first, each item as Alpaca returns it: `id`, `headline`, `summary`,
//! `author`, `source`, `url`, `symbols`, `created_at`, `updated_at`), new
//! stories as a live tail at `/events`, and the usual `/health`, `/metrics`
//! and `/describe`.
//!
//! Credentials: `APCA_API_KEY_ID` and `APCA_API_SECRET_KEY`, or the same
//! names with a `_FILE` suffix pointing at files containing them.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::Router;
use axum::routing::get;
use clap::Parser;
use glacier_observe::{Events, Observer, State, listen_address, unix_seconds};
use serde_json::{Value, json};

const NEWS_URL: &str = "https://data.alpaca.markets/v1beta1/news";

/// Polls Alpaca's news feed and serves recent stories.
#[derive(Parser, Debug)]
#[command(version, about)]
struct Args {
    /// Symbols to follow, comma-separated; all news when empty.
    #[arg(long, env = "ALPACA_NEWS_SYMBOLS", default_value = "")]
    symbols: String,

    /// Seconds between polls.
    #[arg(long, env = "ALPACA_NEWS_INTERVAL", default_value_t = 60)]
    interval: u64,

    /// Stories kept and served.
    #[arg(long, default_value_t = 300)]
    keep: usize,
}

#[derive(Default)]
struct Shared {
    items: Mutex<VecDeque<Value>>,
    updated_at: Mutex<Option<f64>>,
    polls: AtomicU64,
    errors: AtomicU64,
    events: Events,
}

#[tokio::main]
async fn main() {
    let args = Args::parse();
    let (key, secret) = match (secret("APCA_API_KEY_ID"), secret("APCA_API_SECRET_KEY")) {
        (Ok(k), Ok(s)) => (k, s),
        (Err(e), _) | (_, Err(e)) => {
            eprintln!("alpaca-news: {e}");
            std::process::exit(2);
        }
    };
    let symbols: Vec<String> = args
        .symbols
        .split(',')
        .map(|s| s.trim().to_uppercase())
        .filter(|s| !s.is_empty())
        .collect();
    let listen = listen_address("ALPACA_NEWS_LISTEN", "127.0.0.1:9106");
    let observer = Observer::new("alpaca-news", env!("CARGO_PKG_VERSION"));
    let shared = Arc::new(Shared::default());
    observer.describe(json!({
        "role": if symbols.is_empty() { "Market news from Alpaca (Benzinga), all symbols.".to_owned() } else { format!("Market news from Alpaca (Benzinga) for {}.", symbols.join(", ")) },
        "upstream": [{ "id": "benzinga", "label": "benzinga", "sub": "news via Alpaca", "kind": "flow", "via": "REST, every minute" }],
        "streams": { "events": "/events", "news": "/news" },
    }));
    {
        let s = shared.clone();
        observer.set_metrics(move |w| {
            w.counter(
                "glacier_news_polls_total",
                "Polls of the news feed.",
                &[],
                s.polls.load(Ordering::Relaxed) as f64,
            );
            w.counter(
                "glacier_news_errors_total",
                "Failed polls.",
                &[],
                s.errors.load(Ordering::Relaxed) as f64,
            );
            w.gauge(
                "glacier_news_items",
                "Stories held.",
                &[],
                lock(&s.items).len() as f64,
            );
            if let Some(at) = *lock(&s.updated_at) {
                w.gauge(
                    "glacier_news_last_poll_age_seconds",
                    "Seconds since the last successful poll.",
                    &[],
                    unix_seconds() - at,
                );
            }
        });
    }

    let app = Router::new()
        .route(
            "/news",
            get({
                let s = shared.clone();
                move || async move {
                    let items: Vec<Value> = lock(&s.items).iter().cloned().collect();
                    axum::Json(json!({ "items": items, "updated_at": *lock(&s.updated_at) }))
                }
            }),
        )
        .merge(observer.router())
        .merge(shared.events.router());
    let listener = match tokio::net::TcpListener::bind(&listen).await {
        Ok(l) => l,
        Err(e) => {
            eprintln!("alpaca-news: cannot listen on {listen}: {e}");
            std::process::exit(2);
        }
    };
    eprintln!("alpaca-news: endpoints on http://{listen}");
    tokio::spawn(async move {
        if let Err(e) = axum::serve(listener, app).await {
            eprintln!("alpaca-news: http server stopped: {e}");
        }
    });

    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        .build()
        .expect("http client");
    let every = Duration::from_secs(args.interval.max(15));
    loop {
        match poll(&http, &key, &secret, &symbols).await {
            Ok(fresh) => {
                shared.polls.fetch_add(1, Ordering::Relaxed);
                merge(&shared, fresh, args.keep);
                *lock(&shared.updated_at) = Some(unix_seconds());
                observer.set_state(
                    State::Ready,
                    &format!("{} stories", lock(&shared.items).len()),
                );
            }
            Err(e) => {
                shared.errors.fetch_add(1, Ordering::Relaxed);
                eprintln!("alpaca-news: {e}");
                observer.record_error(&e);
                observer.set_state(State::Degraded, "news feed unreachable; retrying");
            }
        }
        tokio::time::sleep(every).await;
    }
}

/// The newest stories, newest first.
async fn poll(
    http: &reqwest::Client,
    key: &str,
    secret: &str,
    symbols: &[String],
) -> Result<Vec<Value>, String> {
    let mut req = http
        .get(NEWS_URL)
        .header("APCA-API-KEY-ID", key)
        .header("APCA-API-SECRET-KEY", secret)
        .query(&[("limit", "50"), ("sort", "desc")]);
    if !symbols.is_empty() {
        req = req.query(&[("symbols", symbols.join(","))]);
    }
    let r = req
        .send()
        .await
        .map_err(|e| format!("request failed: {e}"))?;
    let status = r.status();
    if !status.is_success() {
        return Err(format!("news feed answered {status}"));
    }
    let body: Value = r
        .json()
        .await
        .map_err(|e| format!("unreadable response: {e}"))?;
    Ok(body["news"].as_array().cloned().unwrap_or_default())
}

/// Add stories not seen before (newest first), announce them, keep `keep`.
fn merge(shared: &Shared, fresh: Vec<Value>, keep: usize) {
    let first_poll = lock(&shared.updated_at).is_none();
    let mut items = lock(&shared.items);
    let mut new: Vec<Value> = fresh
        .into_iter()
        .filter(|n| !items.iter().any(|i| i["id"] == n["id"]))
        .collect();
    new.sort_by(|a, b| b["created_at"].as_str().cmp(&a["created_at"].as_str()));
    for n in new.iter().rev() {
        if !first_poll {
            let syms: Vec<&str> = n["symbols"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .take(4)
                .collect();
            shared.events.push(
                format!(
                    "{} [{}]",
                    n["headline"].as_str().unwrap_or("?"),
                    syms.join(", ")
                ),
                n.to_string(),
            );
        }
        items.push_front(n.clone());
    }
    let mut all: Vec<Value> = items.drain(..).collect();
    all.sort_by(|a, b| b["created_at"].as_str().cmp(&a["created_at"].as_str()));
    all.truncate(keep);
    items.extend(all);
}

/// A credential from `NAME`, or from the file named by `NAME_FILE`.
fn secret(name: &str) -> Result<String, String> {
    if let Ok(path) = std::env::var(format!("{name}_FILE")) {
        return std::fs::read_to_string(&path)
            .map(|s| s.trim().to_owned())
            .map_err(|e| format!("cannot read {name}_FILE {path}: {e}"));
    }
    std::env::var(name).map_err(|_| format!("set {name} or {name}_FILE"))
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merges_newest_first_without_duplicates() {
        let s = Shared::default();
        merge(
            &s,
            vec![json!({ "id": 1, "created_at": "2026-10-05T10:00:00Z" })],
            3,
        );
        *lock(&s.updated_at) = Some(1.0);
        merge(
            &s,
            vec![
                json!({ "id": 1, "created_at": "2026-10-05T10:00:00Z" }),
                json!({ "id": 2, "created_at": "2026-10-05T11:00:00Z", "headline": "Up", "symbols": ["SPY"] }),
                json!({ "id": 3, "created_at": "2026-10-05T09:00:00Z" }),
                json!({ "id": 4, "created_at": "2026-10-05T12:00:00Z" }),
            ],
            3,
        );
        let ids: Vec<i64> = lock(&s.items)
            .iter()
            .map(|i| i["id"].as_i64().unwrap())
            .collect();
        assert_eq!(ids, [4, 2, 1]);
        assert_eq!(s.events.since(0, 10)["events"].as_array().unwrap().len(), 3);
    }
}

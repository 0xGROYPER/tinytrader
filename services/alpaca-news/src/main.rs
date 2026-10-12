//! Example service: streams Alpaca's market news (supplied by Benzinga, free
//! with an Alpaca account) and serves recent stories.
//!
//! Stories arrive over Alpaca's news websocket as they are published. On
//! start and after every reconnect, one REST request fills in what was missed.
//! The news stream is separate from the market data streams, so it works
//! alongside another process holding the account's market data stream.
//!
//! Serves `/news` (`{ "items": [...], "updated_at": unix_seconds }`, newest
//! first, each item as Alpaca returns it: `id`, `headline`, `summary`,
//! `author`, `source`, `url`, `symbols`, `created_at`, `updated_at`), new
//! stories as a live tail at `/events`, and the usual `/health`, `/metrics`
//! and `/describe`.
//!
//! With `--rss` (`ALPACA_NEWS_RSS`), headlines from those RSS feeds join
//! the same list, polled every five minutes, with `source` naming the site
//! and no `symbols` (see `rss.rs`).
//!
//! Credentials: `APCA_API_KEY_ID` and `APCA_API_SECRET_KEY`, or the same
//! names with a `_FILE` suffix pointing at files containing them.

mod rss;

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tinytrader_observe::{lock, secret};

use axum::Router;
use axum::routing::get;
use clap::Parser;
use futures_util::{SinkExt as _, StreamExt as _};
use serde_json::{Value, json};
use tinytrader_observe::{Events, Observer, State, listen_address, unix_seconds};
use tokio_tungstenite::tungstenite::Message;

const NEWS_URL: &str = "https://data.alpaca.markets/v1beta1/news";
const STREAM_URL: &str = "wss://stream.data.alpaca.markets/v1beta1/news";

/// Streams Alpaca's news feed and serves recent stories.
#[derive(Parser, Debug)]
#[command(version, about)]
struct Args {
    /// Symbols to follow, comma-separated; all news when empty.
    #[arg(long, env = "ALPACA_NEWS_SYMBOLS", default_value = "")]
    symbols: String,

    /// RSS feeds whose headlines join the news, comma-separated URLs.
    #[arg(long, env = "ALPACA_NEWS_RSS", default_value = "")]
    rss: String,

    /// Stories kept and served.
    #[arg(long, default_value_t = 300)]
    keep: usize,
}

#[derive(Default)]
struct Shared {
    items: Mutex<VecDeque<Value>>,
    updated_at: Mutex<Option<f64>>,
    polls: AtomicU64,
    reconnects: AtomicU64,
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
    let feeds: Vec<String> = args
        .rss
        .split(',')
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
        .collect();
    let mut upstream = vec![
        json!({ "id": "benzinga", "label": "benzinga", "sub": "news via Alpaca", "kind": "flow", "via": "websocket" }),
    ];
    let mut schedule = vec![
        json!({ "name": "News stream", "kind": "always", "when": "always" }),
        json!({ "name": "Catch-up of missed stories", "kind": "event", "when": "at start and after each reconnect" }),
    ];
    for f in &feeds {
        let name = rss::source(f);
        upstream.push(json!({ "id": format!("rss-{name}"), "label": name, "sub": "RSS headlines", "kind": "flow", "via": "http" }));
    }
    if !feeds.is_empty() {
        schedule.push(
            json!({ "name": "RSS headlines", "kind": "interval", "when": "every 5 minutes" }),
        );
    }
    observer.describe(json!({
        "role": if symbols.is_empty() { "Market news from Alpaca (Benzinga), all symbols.".to_owned() } else { format!("Market news from Alpaca (Benzinga) for {}.", symbols.join(", ")) },
        "upstream": upstream,
        "streams": { "events": "/events", "news": "/news" },
        "schedule": schedule,
    }));
    {
        let s = shared.clone();
        observer.set_metrics(move |w| {
            w.counter(
                "tinytrader_news_polls_total",
                "Polls of the news feed.",
                &[],
                s.polls.load(Ordering::Relaxed) as f64,
            );
            w.counter(
                "tinytrader_news_reconnects_total",
                "Reconnections to the news stream.",
                &[],
                s.reconnects.load(Ordering::Relaxed) as f64,
            );
            w.counter(
                "tinytrader_news_errors_total",
                "Failed polls.",
                &[],
                s.errors.load(Ordering::Relaxed) as f64,
            );
            w.gauge(
                "tinytrader_news_items",
                "Stories held.",
                &[],
                lock(&s.items).len() as f64,
            );
            if let Some(at) = *lock(&s.updated_at) {
                w.gauge(
                    "tinytrader_news_last_poll_age_seconds",
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
    let _ = rustls::crypto::ring::default_provider().install_default();
    for url in feeds {
        tokio::spawn(follow_rss(
            http.clone(),
            url,
            shared.clone(),
            observer.clone(),
            args.keep,
        ));
    }
    let mut backoff = Duration::from_secs(1);
    loop {
        // Catch up on anything published while disconnected, then stream.
        match poll(&http, &key, &secret, &symbols).await {
            Ok(fresh) => {
                shared.polls.fetch_add(1, Ordering::Relaxed);
                merge(&shared, fresh, args.keep);
                *lock(&shared.updated_at) = Some(unix_seconds());
            }
            Err(e) => {
                shared.errors.fetch_add(1, Ordering::Relaxed);
                eprintln!("alpaca-news: {e}");
                observer.record_error(&e);
            }
        }
        let started = std::time::Instant::now();
        let streamed = stream(&shared, &observer, &key, &secret, &symbols, args.keep).await;
        if started.elapsed() > Duration::from_secs(60) {
            backoff = Duration::from_secs(1);
        }
        shared.reconnects.fetch_add(1, Ordering::Relaxed);
        if let Err(e) = streamed {
            shared.errors.fetch_add(1, Ordering::Relaxed);
            eprintln!("alpaca-news: {e}");
            observer.record_error(&e);
        }
        observer.set_state(State::Degraded, "news stream disconnected; reconnecting");
        tokio::time::sleep(backoff).await;
        backoff = (backoff * 2).min(Duration::from_secs(60));
    }
}

/// Follow the news stream until it ends. Returns `Ok` when the server
/// closes the connection, `Err` on failure.
async fn stream(
    shared: &Shared,
    observer: &Observer,
    key: &str,
    secret: &str,
    symbols: &[String],
    keep: usize,
) -> Result<(), String> {
    let (mut ws, _) = tokio_tungstenite::connect_async(STREAM_URL)
        .await
        .map_err(|e| format!("cannot connect to the news stream: {e}"))?;
    let subscribe = if symbols.is_empty() {
        vec!["*".to_owned()]
    } else {
        symbols.to_vec()
    };
    while let Some(msg) = ws.next().await {
        let text = match msg.map_err(|e| format!("news stream failed: {e}"))? {
            Message::Text(t) => t,
            Message::Close(_) => return Ok(()),
            _ => continue,
        };
        let batch: Vec<Value> =
            serde_json::from_str(&text).map_err(|e| format!("unreadable stream message: {e}"))?;
        for m in batch {
            match (m["T"].as_str(), m["msg"].as_str()) {
                (Some("success"), Some("connected")) => {
                    let auth = json!({ "action": "auth", "key": key, "secret": secret });
                    ws.send(Message::text(auth.to_string()))
                        .await
                        .map_err(|e| format!("cannot authenticate: {e}"))?;
                }
                (Some("success"), Some("authenticated")) => {
                    let sub = json!({ "action": "subscribe", "news": subscribe });
                    ws.send(Message::text(sub.to_string()))
                        .await
                        .map_err(|e| format!("cannot subscribe: {e}"))?;
                }
                (Some("subscription"), _) => {
                    observer.set_state(
                        State::Ready,
                        &format!("streaming; {} stories", lock(&shared.items).len()),
                    );
                }
                (Some("error"), _) => {
                    return Err(format!(
                        "news stream refused: {} {}",
                        m["code"],
                        m["msg"].as_str().unwrap_or("")
                    ));
                }
                (Some("n"), _) => {
                    merge(shared, vec![m], keep);
                    *lock(&shared.updated_at) = Some(unix_seconds());
                    observer.set_state(
                        State::Ready,
                        &format!("streaming; {} stories", lock(&shared.items).len()),
                    );
                }
                _ => {}
            }
        }
    }
    Ok(())
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

/// Poll one RSS feed every five minutes, merging its headlines.
async fn follow_rss(
    http: reqwest::Client,
    url: String,
    shared: Arc<Shared>,
    observer: Observer,
    keep: usize,
) {
    let source = rss::source(&url);
    let mut tick = tokio::time::interval(Duration::from_secs(300));
    loop {
        tick.tick().await;
        let got = async {
            let r = http
                .get(&url)
                .header("User-Agent", "tinytrader-news")
                .send()
                .await
                .map_err(|e| format!("{source} feed: request failed: {e}"))?;
            if !r.status().is_success() {
                return Err(format!("{source} feed answered {}", r.status()));
            }
            r.text()
                .await
                .map_err(|e| format!("{source} feed: unreadable response: {e}"))
        }
        .await;
        match got {
            Ok(xml) => merge(&shared, rss::parse(&xml, &source), keep),
            Err(e) => {
                shared.errors.fetch_add(1, Ordering::Relaxed);
                eprintln!("alpaca-news: {e}");
                observer.record_error(&e);
            }
        }
    }
}

/// Add stories not seen before (newest first), announce them, keep `keep`.
fn merge(shared: &Shared, fresh: Vec<Value>, keep: usize) {
    let first_poll = lock(&shared.updated_at).is_none();
    let mut items = lock(&shared.items);
    let mut new: Vec<Value> = fresh
        .into_iter()
        .filter(|n| !items.iter().any(|i| i["id"] == n["id"]))
        // Once full, a story older than all those kept was trimmed (or
        // would be), so it isn't news.
        .filter(|n| {
            items.len() < keep
                || items
                    .back()
                    .is_none_or(|o| n["created_at"].as_str() > o["created_at"].as_str())
        })
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

    #[test]
    fn trimmed_stories_dont_come_back() {
        let s = Shared::default();
        let at = |id: i64, h: u32| json!({ "id": id, "created_at": format!("2026-10-05T{h:02}:00:00Z") });
        merge(&s, vec![at(1, 9), at(2, 10), at(3, 11)], 2);
        *lock(&s.updated_at) = Some(1.0);
        // The feed still lists 1, trimmed above; 4 is new.
        merge(&s, vec![at(1, 9), at(4, 12)], 2);
        let ids: Vec<i64> = lock(&s.items)
            .iter()
            .map(|i| i["id"].as_i64().unwrap())
            .collect();
        assert_eq!(ids, [4, 3]);
        assert_eq!(s.events.since(0, 10)["events"].as_array().unwrap().len(), 1);
    }
}

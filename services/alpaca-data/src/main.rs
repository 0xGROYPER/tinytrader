//! Market data for any symbol from Alpaca's REST API: stock snapshots, bars
//! and option chains. The relays carry a few symbols live; this answers for
//! the rest, on request.
//!
//! - `/snapshots?symbols=USO,XLE`: last price, quote, previous close and the
//!   day's change for each (at most 100).
//! - `/bars?symbol=AAPL&days=5&interval=60`: bars as the relays serve them
//!   (`&end=<unix seconds>`: the `days` before then, for older history)
//!   (`[t, o, h, l, c, v]`, `t` in Unix seconds): one-minute (`60`, the
//!   default), 5- and 15-minute, hourly (`3600`) or daily (`86400`), over the
//!   last `days` trading days.
//! - `/chain?underlying=AAPL&from=YYYY-MM-DD&to=YYYY-MM-DD`: option contracts
//!   with quotes, IV and greeks, and per expiry the at-the-money IV and the
//!   straddle as an implied move; by default the next 60 days.
//!
//! - `/news?symbols=USO,XLE&limit=50`: recent stories on those symbols
//!   (Benzinga, through Alpaca), in the news service's shape, newest first.
//!
//! - `/option_bars?symbols=OCC,...&start=&end=`: daily bars of option
//!   contracts, expired ones included (from 2024).
//! - `/option_contracts?underlying=&from=&to=`: the contracts expiring between
//!   two dates, expired ones included (from Alpaca's trading API, read only).
//!
//! - `/screener?min_cap=2e9&min_price=5&scan=150`: stocks with options,
//!   ranked by what the options expect. The universe is Nasdaq's stock
//!   screener (every US listing, with market capitalisation), filtered to the
//!   size and price asked; the largest `scan` of them, the day's biggest
//!   movers and most traded in it, and Alpaca's most-active and movers lists,
//!   are scanned in the background for the at-the-money IV nearest 30 days,
//!   the next expiry's implied move, 20-day realized volatility and average
//!   volume. Each stock is rescanned every 15 minutes while asked for; until
//!   scanned, its row has no option measures (`pending` counts them).
//!
//! Answers are kept briefly (snapshots 2 s, chains 15 s, bars a minute) and
//! requests to Alpaca are held under its free plan's 200 a minute, so many
//! panels asking at once don't multiply the load.
//!
//! Credentials: a data-only key (`APCA_API_KEY_ID` and `APCA_API_SECRET_KEY`,
//! or `_FILE`), not one that trades. Feeds: `ALPACA_DATA_FEED` (`iex`, or
//! `sip` with a subscription) and `ALPACA_OPTIONS_FEED` (`indicative`, or
//! `opra` with a subscription).

mod chain;
mod screener;
mod smile;

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::extract::{Query, State as AxState};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use clap::Parser;
use serde::Deserialize;
use serde_json::{Value, json};
use tinytrader_observe::{Observer, State, listen_address, lock, secret, unix_seconds};
use tinytrader_schedule::ny_date_time;

const BASE: &str = "https://data.alpaca.markets";
/// Alpaca's trading API, read here only for option contract listings.
const TRADING: &str = "https://api.alpaca.markets";
/// Requests to Alpaca allowed in any minute (its free plan allows 200).
const PER_MINUTE: usize = 180;
/// Answers kept, at most, by their size as JSON (a long hourly history is
/// thousands of bars, so a count alone isn't a bound).
const CACHE_BYTES: usize = 40 * 1024 * 1024;
/// Nasdaq's stock screener: every US listing, with market capitalisation.
const NASDAQ: &str = "https://api.nasdaq.com/api/screener/stocks?tableonly=true&download=true";
/// How long a screener measure is kept before the stock is scanned again.
const RESCAN: Duration = Duration::from_secs(15 * 60);
/// The background scan stops this long after the screener was last asked for.
const SCAN_IDLE: Duration = Duration::from_secs(30 * 60);

/// Serves stock snapshots, bars and option chains from Alpaca.
#[derive(Parser, Debug)]
#[command(version, about)]
struct Args {
    /// Stock feed: iex (free) or sip (subscription).
    #[arg(long, env = "ALPACA_DATA_FEED", default_value = "iex")]
    feed: String,
    /// Options feed: indicative (free) or opra (subscription).
    #[arg(long, env = "ALPACA_OPTIONS_FEED", default_value = "indicative")]
    options_feed: String,
    /// Feed for bars older than 15 minutes: sip (every exchange, free for
    /// history on every plan) by default; the stock feed serves the rest.
    #[arg(long, env = "ALPACA_HISTORY_FEED", default_value = "sip")]
    history_feed: String,
}

struct App {
    http: reqwest::Client,
    key: String,
    secret: String,
    feed: String,
    options_feed: String,
    history_feed: String,
    observer: Observer,
    /// Recent requests to Alpaca, for the rate limit.
    sent: Mutex<VecDeque<Instant>>,
    /// Answers kept briefly, by request.
    /// Answers kept briefly, by request: when, the answer, its size.
    cache: Mutex<HashMap<String, (Instant, Value, usize)>>,
    /// The screener's universe, its scans and what it was last asked for.
    screen: Mutex<Screen>,
}

/// The options screener's state, shared with its background scan.
#[derive(Default)]
struct Screen {
    /// Nasdaq's listings, and when they were fetched.
    universe: Option<(Instant, Arc<Vec<screener::Stock>>)>,
    /// Stocks to scan (with their price), in order, and when last asked for.
    wanted: Vec<(String, f64)>,
    asked: Option<Instant>,
    /// Option measures by stock, and when they were taken.
    options: HashMap<String, (Instant, screener::Measures)>,
    /// Realized volatility and average volume from daily bars, by stock.
    daily: HashMap<String, (Instant, Option<f64>, Option<f64>)>,
}

impl App {
    /// GET from Alpaca, within the rate limit.
    async fn get(
        &self,
        path: &str,
        query: &[(&str, String)],
    ) -> Result<Value, (StatusCode, String)> {
        {
            let mut sent = lock(&self.sent);
            while sent
                .front()
                .is_some_and(|t| t.elapsed() > Duration::from_secs(60))
            {
                sent.pop_front();
            }
            if sent.len() >= PER_MINUTE {
                return Err((
                    StatusCode::TOO_MANY_REQUESTS,
                    "busy: Alpaca's request limit for this minute is used up".into(),
                ));
            }
            sent.push_back(Instant::now());
        }
        // Contract listings live on the trading API; everything else on the data API.
        let host = if path.starts_with("/v2/options/contracts") {
            TRADING
        } else {
            BASE
        };
        let r = self
            .http
            .get(format!("{host}{path}"))
            .header("APCA-API-KEY-ID", &self.key)
            .header("APCA-API-SECRET-KEY", &self.secret)
            .query(query)
            .send()
            .await
            .map_err(|e| (StatusCode::BAD_GATEWAY, e.without_url().to_string()))?;
        let status = r.status();
        let v: Value = r
            .json()
            .await
            .map_err(|e| (StatusCode::BAD_GATEWAY, e.without_url().to_string()))?;
        if !status.is_success() {
            let why = v["message"]
                .as_str()
                .unwrap_or("no reason given")
                .to_owned();
            let e = format!("Alpaca answered {status}: {why}");
            self.observer.record_error(&e);
            // Credentials and entitlements are worth showing on the service itself.
            if matches!(status.as_u16(), 401 | 403) {
                self.observer.set_state(State::Degraded, &e);
            }
            return Err((StatusCode::BAD_GATEWAY, e));
        }
        self.observer.set_state(State::Ready, "answering");
        Ok(v)
    }

    /// A kept answer for `key` if younger than `ttl`, else `make`'s.
    async fn kept<F, Fut>(
        &self,
        key: String,
        ttl: Duration,
        make: F,
    ) -> Result<Value, (StatusCode, String)>
    where
        F: FnOnce() -> Fut,
        Fut: std::future::Future<Output = Result<Value, (StatusCode, String)>>,
    {
        if let Some((at, v, _)) = lock(&self.cache).get(&key)
            && at.elapsed() < ttl
        {
            return Ok(v.clone());
        }
        let v = make().await?;
        let mut cache = lock(&self.cache);
        let size = v.to_string().len();
        // Expired answers go first, then the oldest, until this one fits.
        cache.retain(|_, (t, _, _)| t.elapsed() < Duration::from_secs(86_400));
        let mut total: usize = cache.values().map(|e| e.2).sum();
        if total + size > CACHE_BYTES {
            let mut ages: Vec<(String, Instant, usize)> = cache
                .iter()
                .map(|(k, (t, _, n))| (k.clone(), *t, *n))
                .collect();
            ages.sort_by_key(|(_, t, _)| *t);
            for (k, _, n) in ages {
                if total + size <= CACHE_BYTES {
                    break;
                }
                cache.remove(&k);
                total -= n;
            }
        }
        if size <= CACHE_BYTES {
            cache.insert(key, (Instant::now(), v.clone(), size));
        }
        Ok(v)
    }
}

/// A stock or ETF symbol: letters, digits and dots, at most 10.
fn symbol_ok(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 10
        && s.bytes()
            .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'.')
}

fn refuse(code: StatusCode, why: impl Into<String>) -> Response {
    (code, Json(json!({ "error": why.into() }))).into_response()
}

fn answer(r: Result<Value, (StatusCode, String)>) -> Response {
    match r {
        Ok(v) => Json(v).into_response(),
        Err((code, e)) => refuse(code, e),
    }
}

/// Unix seconds as an RFC 3339 time in UTC, such as `2026-10-07T19:04:00Z`.
fn rfc3339(t: i64) -> String {
    let (days, secs) = (t.div_euclid(86_400), t.rem_euclid(86_400));
    // Civil date from days (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        secs / 3600,
        secs % 3600 / 60,
        secs % 60
    )
}

/// Seconds since the epoch of an RFC 3339 time such as `2026-10-07T19:04:00Z`.
fn unix(t: &str) -> Option<i64> {
    let p = |a: usize, b: usize| t.get(a..b)?.parse::<i64>().ok();
    let (y, mo, d, h, mi, s) = (
        p(0, 4)?,
        p(5, 7)?,
        p(8, 10)?,
        p(11, 13)?,
        p(14, 16)?,
        p(17, 19)?,
    );
    // Days from the civil date (Howard Hinnant's algorithm).
    let y = if mo <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (mo + if mo > 2 { -3 } else { 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    Some((era * 146_097 + doe - 719_468) * 86_400 + h * 3600 + mi * 60 + s)
}

#[derive(Deserialize)]
struct SnapAsk {
    symbols: String,
}

async fn snapshots(AxState(app): AxState<Arc<App>>, Query(ask): Query<SnapAsk>) -> Response {
    let mut list: Vec<String> = ask
        .symbols
        .split(',')
        .map(|s| s.trim().to_uppercase())
        .filter(|s| !s.is_empty())
        .collect();
    list.sort();
    list.dedup();
    if list.is_empty() || list.len() > 100 || !list.iter().all(|s| symbol_ok(s)) {
        return refuse(
            StatusCode::BAD_REQUEST,
            "symbols: 1 to 100 stock symbols, comma-separated",
        );
    }
    let joined = list.join(",");
    let a = app.clone();
    answer(
        app.kept(format!("snap|{joined}"), Duration::from_secs(2), || async move {
            let v = a.get("/v2/stocks/snapshots", &[("symbols", joined), ("feed", a.feed.clone())]).await?;
            let out: Vec<Value> = list
                .iter()
                .filter_map(|s| {
                    let x = v.get(s)?;
                    let price = x["latestTrade"]["p"].as_f64().or_else(|| x["dailyBar"]["c"].as_f64());
                    let prev = x["prevDailyBar"]["c"].as_f64();
                    let change = price.zip(prev).map(|(p, c)| p - c);
                    Some(json!({
                        "symbol": s, "price": price, "bid": x["latestQuote"]["bp"], "ask": x["latestQuote"]["ap"],
                        "prev_close": prev, "change": change, "change_pct": change.zip(prev).map(|(d, c)| d / c),
                        "day": { "o": x["dailyBar"]["o"], "h": x["dailyBar"]["h"], "l": x["dailyBar"]["l"], "c": x["dailyBar"]["c"], "v": x["dailyBar"]["v"] },
                        "at": x["latestTrade"]["t"].as_str().and_then(unix),
                    }))
                })
                .collect();
            Ok(json!({ "snapshots": out, "feed": a.feed }))
        })
        .await,
    )
}

#[derive(Deserialize)]
struct OptionBarsAsk {
    symbols: String,
    start: String,
    end: String,
}

/// An OCC option symbol, such as `AAPL260807C00210000`.
fn occ_ok(s: &str) -> bool {
    chain::occ(s).is_some()
        && s.len() <= 21
        && s.bytes()
            .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit())
}

fn date_ok(d: &str) -> bool {
    d.len() == 10
        && d.bytes().enumerate().all(|(i, b)| {
            if i == 4 || i == 7 {
                b == b'-'
            } else {
                b.is_ascii_digit()
            }
        })
}

/// Daily bars of option contracts, expired ones included (Alpaca keeps them
/// from 2024): `/option_bars?symbols=OCC,...&start=&end=` (at most 20).
async fn option_bars(
    AxState(app): AxState<Arc<App>>,
    Query(ask): Query<OptionBarsAsk>,
) -> Response {
    let list: Vec<String> = ask
        .symbols
        .split(',')
        .map(|s| s.trim().to_uppercase())
        .filter(|s| !s.is_empty())
        .collect();
    if list.is_empty()
        || list.len() > 20
        || !list.iter().all(|s| occ_ok(s))
        || !date_ok(&ask.start)
        || !date_ok(&ask.end)
    {
        return refuse(
            StatusCode::BAD_REQUEST,
            "symbols: 1 to 20 option contracts (OCC symbols); start and end: YYYY-MM-DD",
        );
    }
    let joined = list.join(",");
    // The past doesn't change: kept a day; what includes today, a minute.
    let today = ny_date_time(unix_seconds() as i64).0;
    let ttl = Duration::from_secs(if ask.end < today { 86_400 } else { 60 });
    let a = app.clone();
    let (start, end) = (ask.start.clone(), ask.end.clone());
    answer(
        app.kept(
            format!("obars|{joined}|{start}|{end}"),
            ttl,
            || async move {
                let mut out = serde_json::Map::new();
                let mut token: Option<String> = None;
                for _ in 0..10 {
                    let mut q = vec![
                        ("symbols", joined.clone()),
                        ("timeframe", "1Day".to_owned()),
                        ("start", start.clone()),
                        ("end", end.clone()),
                        ("limit", "10000".to_owned()),
                    ];
                    if let Some(t) = &token {
                        q.push(("page_token", t.clone()));
                    }
                    let v = a.get("/v1beta1/options/bars", &q).await?;
                    for (sym, bars) in v["bars"].as_object().into_iter().flatten() {
                        let rows = out.entry(sym.clone()).or_insert_with(|| json!([]));
                        for b in bars.as_array().into_iter().flatten() {
                            if let (Some(t), Some(arr)) =
                                (b["t"].as_str().and_then(unix), rows.as_array_mut())
                            {
                                arr.push(json!([t, b["o"], b["h"], b["l"], b["c"], b["v"]]));
                            }
                        }
                    }
                    token = v["next_page_token"].as_str().map(str::to_owned);
                    if token.is_none() {
                        break;
                    }
                }
                Ok(json!({ "bars": out }))
            },
        )
        .await,
    )
}

#[derive(Deserialize)]
struct ContractsAsk {
    underlying: String,
    from: String,
    to: String,
}

/// Option contracts of an underlying expiring between two dates, expired
/// ones included: `/option_contracts?underlying=&from=&to=`.
async fn option_contracts(
    AxState(app): AxState<Arc<App>>,
    Query(ask): Query<ContractsAsk>,
) -> Response {
    let underlying = ask.underlying.trim().to_uppercase();
    if !symbol_ok(&underlying) || !date_ok(&ask.from) || !date_ok(&ask.to) {
        return refuse(
            StatusCode::BAD_REQUEST,
            "underlying: a stock symbol; from and to: YYYY-MM-DD",
        );
    }
    let today = ny_date_time(unix_seconds() as i64).0;
    let ttl = Duration::from_secs(if ask.to < today { 86_400 } else { 3600 });
    let a = app.clone();
    let (from, to) = (ask.from.clone(), ask.to.clone());
    answer(
        app.kept(format!("contracts|{underlying}|{from}|{to}"), ttl, || async move {
            let mut out = Vec::new();
            // Expired contracts are "inactive"; listed ones "active".
            for status in ["inactive", "active"] {
                let mut token: Option<String> = None;
                for _ in 0..10 {
                    let mut q = vec![
                        ("underlying_symbols", underlying.clone()), ("status", status.to_owned()),
                        ("expiration_date_gte", from.clone()), ("expiration_date_lte", to.clone()), ("limit", "10000".to_owned()),
                    ];
                    if let Some(t) = &token {
                        q.push(("page_token", t.clone()));
                    }
                    let v = a.get("/v2/options/contracts", &q).await?;
                    for c in v["option_contracts"].as_array().into_iter().flatten() {
                        out.push(json!({ "symbol": c["symbol"], "expiry": c["expiration_date"], "type": c["type"], "strike": c["strike_price"].as_str().and_then(|x| x.parse::<f64>().ok()) }));
                    }
                    token = v["next_page_token"].as_str().map(str::to_owned);
                    if token.is_none() {
                        break;
                    }
                }
            }
            Ok(json!({ "underlying": underlying, "contracts": out }))
        })
        .await,
    )
}

#[derive(Deserialize)]
struct NewsAsk {
    symbols: String,
    limit: Option<u32>,
}

async fn news(AxState(app): AxState<Arc<App>>, Query(ask): Query<NewsAsk>) -> Response {
    let mut list: Vec<String> = ask
        .symbols
        .split(',')
        .map(|s| s.trim().to_uppercase())
        .filter(|s| !s.is_empty())
        .collect();
    list.sort();
    list.dedup();
    if list.is_empty() || list.len() > 50 || !list.iter().all(|s| symbol_ok(s)) {
        return refuse(
            StatusCode::BAD_REQUEST,
            "symbols: 1 to 50 stock symbols, comma-separated",
        );
    }
    let limit = ask.limit.unwrap_or(50).clamp(1, 50);
    let joined = list.join(",");
    let a = app.clone();
    answer(
        app.kept(format!("news|{joined}|{limit}"), Duration::from_secs(60), || async move {
            let v = a.get("/v1beta1/news", &[("symbols", joined), ("limit", limit.to_string()), ("sort", "desc".into())]).await?;
            // As the news service serves them, without the full text.
            let items: Vec<Value> = v["news"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|n| json!({ "id": n["id"], "headline": n["headline"], "summary": n["summary"], "author": n["author"], "source": n["source"], "url": n["url"], "symbols": n["symbols"], "created_at": n["created_at"], "updated_at": n["updated_at"] }))
                .collect();
            Ok(json!({ "items": items }))
        })
        .await,
    )
}

#[derive(Deserialize)]
struct BarAsk {
    symbol: String,
    days: Option<u64>,
    interval: Option<u64>,
    /// Only bars before this time (unix seconds): the `days` before it, so a
    /// chart can reach back a chunk at a time. Absent: up to now.
    end: Option<i64>,
    /// `split` (the default: prices comparable across splits), `raw` (as
    /// traded that day: what option strikes of the time refer to), or `all`
    /// (splits and dividends).
    adjustment: Option<String>,
}

async fn bars(AxState(app): AxState<Arc<App>>, Query(ask): Query<BarAsk>) -> Response {
    let symbol = ask.symbol.trim().to_uppercase();
    if !symbol_ok(&symbol) {
        return refuse(StatusCode::BAD_REQUEST, "symbol: a stock symbol");
    }
    let interval = ask.interval.unwrap_or(60);
    let (timeframe, max_days) = match interval {
        60 => ("1Min", 30),
        300 => ("5Min", 60),
        900 => ("15Min", 120),
        3600 => ("1Hour", 730),
        86_400 => ("1Day", 3650),
        // `days` counts weeks here.
        604_800 => ("1Week", 1040),
        _ => {
            return refuse(
                StatusCode::BAD_REQUEST,
                "interval: 60, 300, 900, 3600, 86400 or 604800 seconds",
            );
        }
    };
    let days = ask.days.unwrap_or(1).clamp(1, max_days);
    let adjustment = ask.adjustment.unwrap_or_else(|| "split".into());
    if !matches!(adjustment.as_str(), "split" | "raw" | "all" | "dividend") {
        return refuse(
            StatusCode::BAD_REQUEST,
            "adjustment: split, raw, dividend or all",
        );
    }
    let end = ask.end.filter(|&e| e > 0);
    let a = app.clone();
    answer(
        app.kept(
            format!("bars|{symbol}|{interval}|{days}|{adjustment}|{}", end.unwrap_or(0)),
            Duration::from_secs(60),
            || async move {
                let now = unix_seconds() as i64;
                let upto = end.map_or(now, |e| e.min(now));
                // `days` counts trading days (sessions with bars; weeks for weekly
                // bars): reach back far enough for weekends and holidays, then
                // keep the last ones.
                let back = if interval == 604_800 { days as i64 * 7 + 7 } else { days as i64 * 7 / 5 + 4 };
                let start = ny_date_time(upto - back * 86_400).0;
                // History from the full market (SIP) up to 15 minutes ago, where
                // the free plan allows it; the stock feed (IEX) after that. IEX
                // alone is a small slice of trading: a thin ETF can go hours
                // without an IEX trade, leaving stale closes.
                let cutoff = now - 16 * 60;
                let mut out = Vec::new();
                let parts = feed_windows(&a.history_feed, &a.feed, cutoff, end);
                // Newest first, so if a long range is cut short it's the oldest
                // part that's missing (Alpaca sends about 200 bars a page for
                // full-market history, whatever the limit asked).
                let mut truncated = false;
                for (feed, from, until) in parts {
                    let mut token: Option<String> = None;
                    for page in 0..60 {
                        let mut q = vec![
                            ("timeframe", timeframe.to_owned()),
                            ("start", from.map_or_else(|| start.clone(), rfc3339)),
                            ("limit", "10000".to_owned()),
                            ("adjustment", adjustment.clone()),
                            ("feed", feed.clone()),
                            ("sort", "desc".to_owned()),
                        ];
                        if let Some(u) = until {
                            q.push(("end", rfc3339(u)));
                        }
                        if let Some(t) = &token {
                            q.push(("page_token", t.clone()));
                        }
                        let v = a.get(&format!("/v2/stocks/{symbol}/bars"), &q).await?;
                        for b in v["bars"].as_array().into_iter().flatten() {
                            if let Some(t) = b["t"].as_str().and_then(unix) {
                                out.push(json!([t, b["o"], b["h"], b["l"], b["c"], b["v"]]));
                            }
                        }
                        token = v["next_page_token"].as_str().map(str::to_owned);
                        if token.is_none() {
                            break;
                        }
                        truncated |= page == 59;
                    }
                }
                // In time order; where the two feeds meet, the full market's bar wins.
                out.sort_by_key(|b| b[0].as_i64().unwrap_or(0));
                out.dedup_by_key(|b| b[0].as_i64().unwrap_or(0));
                // Alpaca's end is inclusive; `end` here is not.
                if let Some(e) = end {
                    out.retain(|b| b[0].as_i64().unwrap_or(0) < e);
                }
                let keep = last_days(&out, days as usize, interval >= 86_400);
                let note = truncated.then(|| "only the most recent part of this range (too many bars)".to_owned());
                Ok(json!({ "symbol": symbol, "interval": interval, "bars": &out[out.len() - keep..], "note": note }))
            },
        )
        .await,
    )
}

/// The feeds to ask and their (start, end) overrides: the full market (SIP)
/// up to `cutoff` (15 minutes ago, where the free plan allows it) and the
/// stock feed after, or one feed when they're the same; all ending at `end`
/// when one is given.
fn feed_windows(history: &str, feed: &str, cutoff: i64, end: Option<i64>) -> Vec<(String, Option<i64>, Option<i64>)> {
    if history == feed {
        return vec![(feed.to_owned(), None, end)];
    }
    match end {
        Some(e) if e <= cutoff => vec![(history.to_owned(), None, Some(e))],
        Some(e) => vec![(history.to_owned(), None, Some(cutoff)), (feed.to_owned(), Some(cutoff), Some(e))],
        None => vec![(history.to_owned(), None, Some(cutoff)), (feed.to_owned(), Some(cutoff), None)],
    }
}

/// How many of `bars` (in time order) make up the last `days` trading days:
/// that many daily bars, or the minute bars of the last `days` New York dates.
fn last_days(bars: &[Value], days: usize, daily: bool) -> usize {
    if daily {
        return bars.len().min(days);
    }
    let mut dates = 0;
    let mut last: Option<String> = None;
    for (i, b) in bars.iter().enumerate().rev() {
        let d = ny_date_time(b[0].as_i64().unwrap_or(0)).0;
        if last.as_ref() != Some(&d) {
            dates += 1;
            if dates > days {
                return bars.len() - i - 1;
            }
            last = Some(d);
        }
    }
    bars.len()
}

#[derive(Deserialize)]
struct ChainAsk {
    underlying: String,
    from: Option<String>,
    to: Option<String>,
}

async fn option_chain(AxState(app): AxState<Arc<App>>, Query(ask): Query<ChainAsk>) -> Response {
    let underlying = ask.underlying.trim().to_uppercase();
    if !symbol_ok(&underlying) {
        return refuse(StatusCode::BAD_REQUEST, "underlying: a stock symbol");
    }
    let date_ok = |d: &str| {
        d.len() == 10
            && d.bytes().enumerate().all(|(i, b)| {
                if i == 4 || i == 7 {
                    b == b'-'
                } else {
                    b.is_ascii_digit()
                }
            })
    };
    let now = unix_seconds() as i64;
    let from = ask.from.unwrap_or_else(|| ny_date_time(now).0);
    let to = ask.to.unwrap_or_else(|| ny_date_time(now + 60 * 86_400).0);
    if !date_ok(&from) || !date_ok(&to) {
        return refuse(StatusCode::BAD_REQUEST, "from and to: YYYY-MM-DD");
    }
    let a = app.clone();
    answer(
        app.kept(format!("chain|{underlying}|{from}|{to}"), Duration::from_secs(15), || async move {
            let spot = a
                .get("/v2/stocks/snapshots", &[("symbols", underlying.clone()), ("feed", a.feed.clone())])
                .await?
                .get(&underlying)
                .and_then(|x| x["latestTrade"]["p"].as_f64());
            let mut contracts = Vec::new();
            let mut token: Option<String> = None;
            // At most 20 pages of 1,000 contracts.
            let mut truncated = false;
            for page in 0..20 {
                let mut q = vec![
                    ("feed", a.options_feed.clone()),
                    ("limit", "1000".to_owned()),
                    ("expiration_date_gte", from.clone()),
                    ("expiration_date_lte", to.clone()),
                ];
                if let Some(t) = &token {
                    q.push(("page_token", t.clone()));
                }
                let v = a.get(&format!("/v1beta1/options/snapshots/{underlying}"), &q).await?;
                contracts.extend(chain::contracts(&v));
                token = v["next_page_token"].as_str().map(str::to_owned);
                if token.is_none() {
                    break;
                }
                truncated = page == 19;
            }
            // By expiry, then strike, calls before puts.
            contracts.sort_by(|a, b| a.expiry.cmp(&b.expiry).then(a.strike.total_cmp(&b.strike)).then(b.call.cmp(&a.call)));
            Ok(json!({
                "underlying": underlying, "spot": spot, "feed": a.options_feed, "from": from, "to": to,
                "expiries": chain::expiries(&contracts, spot, &|x| years_to_close(x, now)),
                "contracts": contracts.iter().map(chain::Contract::to_json).collect::<Vec<_>>(),
                "truncated": truncated,
            }))
        })
        .await,
    )
}

#[derive(Deserialize)]
struct ScreenAsk {
    min_cap: Option<f64>,
    min_price: Option<f64>,
    scan: Option<usize>,
}

/// Nasdaq's listings, fetched at most every 10 minutes (the last ones kept
/// if Nasdaq doesn't answer).
async fn universe(app: &App) -> Result<Arc<Vec<screener::Stock>>, (StatusCode, String)> {
    let kept = lock(&app.screen).universe.clone();
    if let Some((at, u)) = &kept
        && at.elapsed() < Duration::from_secs(600)
    {
        return Ok(u.clone());
    }
    let fetched = async {
        let r = app
            .http
            .get(NASDAQ)
            // Nasdaq answers browsers only.
            .header("User-Agent", "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.0 Safari/605.1.15")
            .header("Accept", "application/json")
            .send()
            .await
            .map_err(|e| e.without_url().to_string())?;
        if !r.status().is_success() {
            return Err(format!("answered {}", r.status()));
        }
        let v: Value = r.json().await.map_err(|e| e.without_url().to_string())?;
        let list = screener::nasdaq(&v);
        if list.len() < 1000 {
            return Err(format!("only {} listings", list.len()));
        }
        Ok(list)
    }
    .await;
    match (fetched, kept) {
        (Ok(list), _) => {
            let u = Arc::new(list);
            lock(&app.screen).universe = Some((Instant::now(), u.clone()));
            Ok(u)
        }
        (Err(e), Some((_, u))) => {
            app.observer.record_error(&format!("Nasdaq's screener: {e}"));
            Ok(u)
        }
        (Err(e), None) => Err((StatusCode::BAD_GATEWAY, format!("Nasdaq's screener: {e}"))),
    }
}

/// Stocks ranked by what their options expect: `/screener?min_cap=&min_price=&scan=`.
async fn screen(AxState(app): AxState<Arc<App>>, Query(ask): Query<ScreenAsk>) -> Response {
    let min_cap = ask.min_cap.unwrap_or(2e9).max(0.0);
    let min_price = ask.min_price.unwrap_or(5.0).max(0.0);
    let scan_n = ask.scan.unwrap_or(150).clamp(10, 300);
    let all = match universe(&app).await {
        Ok(u) => u,
        Err((code, e)) => return refuse(code, e),
    };
    // Alpaca's own lists (unfiltered: warrants and penny stocks crowd them);
    // only what passes the filters is used, and the screener answers without them.
    let a = app.clone();
    let actives = app
        .kept("screen|actives".into(), Duration::from_secs(300), || async move {
            a.get("/v1beta1/screener/stocks/most-actives", &[("by", "volume".into()), ("top", "50".into())]).await
        })
        .await
        .map(|v| screener::most_actives(&v))
        .unwrap_or_default();
    let a = app.clone();
    let movers = app
        .kept("screen|movers".into(), Duration::from_secs(300), || async move {
            a.get("/v1beta1/screener/stocks/movers", &[("top", "50".into())]).await
        })
        .await
        .map(|v| screener::movers(&v))
        .unwrap_or_default();

    let fits: Vec<&screener::Stock> = all
        .iter()
        .filter(|s| s.market_cap.unwrap_or(0.0) >= min_cap && s.price.unwrap_or(0.0) >= min_price)
        .collect();
    let by: HashMap<&str, &screener::Stock> = fits.iter().map(|s| (s.symbol.as_str(), *s)).collect();
    // Each stock once, in the order first listed, with why it's listed.
    let mut order: Vec<&str> = Vec::new();
    let mut tags: HashMap<&str, Vec<&'static str>> = HashMap::new();
    let mut add = |sym: &str, tag: &'static str| {
        if let Some(s) = by.get(sym) {
            let k = s.symbol.as_str();
            let t = tags.entry(k).or_default();
            if t.is_empty() {
                order.push(k);
            }
            if !t.contains(&tag) {
                t.push(tag);
            }
        }
    };
    let top = |key: &dyn Fn(&screener::Stock) -> f64, n: usize| {
        let mut list = fits.clone();
        list.sort_by(|a, b| key(b).total_cmp(&key(a)));
        list.truncate(n);
        list
    };
    for s in top(&|s| s.market_cap.unwrap_or(0.0), scan_n) {
        add(&s.symbol, "large");
    }
    for s in top(&|s| s.change_pct.unwrap_or(0.0).abs(), 25) {
        add(&s.symbol, "mover");
    }
    for s in top(&|s| s.volume.unwrap_or(0.0) * s.price.unwrap_or(0.0), 25) {
        add(&s.symbol, "active");
    }
    for s in &actives {
        add(s, "alpaca-active");
    }
    for (s, side) in &movers {
        add(s, if *side == "gainer" { "alpaca-gainer" } else { "alpaca-loser" });
    }

    let today = ny_date_time(unix_seconds() as i64).0;
    let mut sc = lock(&app.screen);
    sc.wanted = order.iter().map(|k| ((*k).to_owned(), by[k].price.unwrap_or(0.0))).collect();
    sc.asked = Some(Instant::now());
    let mut pending = 0;
    let rows: Vec<Value> = order
        .iter()
        .map(|k| {
            let s = by[k];
            let m = sc.options.get(*k).map(|x| &x.1);
            pending += usize::from(m.is_none());
            let (rv, avg) = sc.daily.get(*k).map_or((None, None), |x| (x.1, x.2));
            let iv = m.and_then(|m| m.atm_iv_30d);
            let move_expiry = m.and_then(|m| m.move_expiry.clone());
            json!({
                "symbol": s.symbol, "name": s.name, "sector": s.sector, "price": s.price,
                "change_pct": s.change_pct, "volume": s.volume,
                "dollar_volume": s.volume.zip(s.price).map(|(v, p)| v * p),
                "avg_volume_20d": avg,
                "rel_volume": s.volume.zip(avg).filter(|(_, a)| *a > 0.0).map(|(v, a)| v / a),
                "market_cap": s.market_cap,
                "atm_iv_30d": iv, "iv_expiry": m.and_then(|m| m.iv_expiry.clone()),
                "implied_move": m.and_then(|m| m.implied_move),
                "move_sessions": move_expiry.as_deref().and_then(|x| screener::sessions(&today, x)),
                "move_expiry": move_expiry,
                "rv_20d": rv,
                "skew": m.and_then(|m| m.skew),
                "iv_rv": iv.zip(rv).filter(|(_, r)| *r > 0.0).map(|(i, r)| i / r),
                "note": m.and_then(|m| m.note.clone()),
                "sources": tags[k],
            })
        })
        .collect();
    Json(json!({
        "as_of": unix_seconds() as i64, "universe_size": all.len(), "matched": fits.len(),
        "min_cap": min_cap, "min_price": min_price, "pending": pending,
        "options_feed": app.options_feed, "rows": rows,
    }))
    .into_response()
}

/// The screener's background scan, while it's being asked for: daily bars for
/// the stocks it lists, in batches, then each stock's near option chain, one
/// at a time. It uses at most a third of the request budget: the limit is
/// the account's, shared with other services on the same key, and panels'
/// own requests must still get through.
async fn scan(app: Arc<App>) {
    loop {
        tokio::time::sleep(Duration::from_millis(700)).await;
        while lock(&app.sent).len() >= PER_MINUTE / 3 {
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
        let (bars_due, chain_due) = {
            let sc = lock(&app.screen);
            if sc.asked.is_none_or(|t| t.elapsed() > SCAN_IDLE) {
                (Vec::new(), None)
            } else {
                let stale = |t: &Instant| t.elapsed() >= RESCAN;
                let bars: Vec<String> = sc
                    .wanted
                    .iter()
                    .filter(|(s, _)| sc.daily.get(s).is_none_or(|x| stale(&x.0)))
                    .map(|(s, _)| s.clone())
                    .take(100)
                    .collect();
                let chain = sc
                    .wanted
                    .iter()
                    .find(|(s, p)| *p > 0.0 && sc.options.get(s).is_none_or(|x| stale(&x.0)))
                    .cloned();
                (bars, chain)
            }
        };
        if !bars_due.is_empty() {
            daily_bars(&app, &bars_due).await;
        } else if let Some((sym, spot)) = chain_due {
            near_chain(&app, &sym, spot).await;
        } else {
            tokio::time::sleep(Duration::from_secs(5)).await;
        }
    }
}

/// Realized volatility (the last 20 daily returns) and average volume (the 20
/// sessions before the latest, which may still be trading) for `symbols`.
async fn daily_bars(app: &App, symbols: &[String]) {
    let now = unix_seconds() as i64;
    let mut got: HashMap<String, Vec<(i64, f64, f64)>> = HashMap::new();
    let mut token: Option<String> = None;
    for _ in 0..10 {
        let mut q = vec![
            ("symbols", symbols.join(",")),
            ("timeframe", "1Day".to_owned()),
            ("start", rfc3339(now - 45 * 86_400)),
            // The full market's history is free from 15 minutes back.
            ("end", rfc3339(now - 16 * 60)),
            ("limit", "10000".to_owned()),
            ("adjustment", "split".to_owned()),
            ("feed", app.history_feed.clone()),
        ];
        if let Some(t) = &token {
            q.push(("page_token", t.clone()));
        }
        let v = match app.get("/v2/stocks/bars", &q).await {
            Ok(v) => v,
            // Over the limit: try again later. Otherwise, mark them scanned.
            Err((StatusCode::TOO_MANY_REQUESTS, _)) => return,
            Err(_) => break,
        };
        for (sym, bars) in v["bars"].as_object().into_iter().flatten() {
            let list = got.entry(sym.clone()).or_default();
            for b in bars.as_array().into_iter().flatten() {
                if let (Some(t), Some(c), Some(vol)) = (b["t"].as_str().and_then(unix), b["c"].as_f64(), b["v"].as_f64()) {
                    list.push((t, c, vol));
                }
            }
        }
        token = v["next_page_token"].as_str().map(str::to_owned);
        if token.is_none() {
            break;
        }
    }
    let at = Instant::now();
    let mut sc = lock(&app.screen);
    for s in symbols {
        let mut b = got.remove(s).unwrap_or_default();
        b.sort_by_key(|x| x.0);
        let closes: Vec<f64> = b.iter().rev().take(21).rev().map(|x| x.1).collect();
        let before = b.len().saturating_sub(1);
        let vols: Vec<f64> = b[before.saturating_sub(20)..before].iter().map(|x| x.2).collect();
        sc.daily.insert(s.clone(), (at, screener::realized_vol(&closes), screener::mean(&vols)));
    }
}

/// Years from `now` to an expiry's 16:00 New York close; none once it has passed.
fn years_to_close(expiry: &str, now: i64) -> Option<f64> {
    let day = screener::days(expiry)?;
    // 16:00 is 20:00 UTC in daylight saving time, else 21:00.
    let close = [20, 21].into_iter().map(|h| day * 86_400 + h * 3600).find(|&t| ny_date_time(t).1 == "16:00")?;
    let s = close - now;
    (s > 0).then(|| s as f64 / (365.0 * 86_400.0))
}

/// The at-the-money IV nearest 30 days and the next expiry's implied move,
/// from the contracts within 15% of `spot` expiring in the next 50 days.
async fn near_chain(app: &App, sym: &str, spot: f64) {
    let now = unix_seconds() as i64;
    let today = ny_date_time(now).0;
    let mut contracts = Vec::new();
    let mut token: Option<String> = None;
    let mut m = screener::Measures::default();
    for _ in 0..4 {
        let mut q = vec![
            ("feed", app.options_feed.clone()),
            ("limit", "1000".to_owned()),
            ("expiration_date_gte", today.clone()),
            ("expiration_date_lte", ny_date_time(now + 50 * 86_400).0),
            ("strike_price_gte", format!("{:.2}", spot * 0.85)),
            ("strike_price_lte", format!("{:.2}", spot * 1.15)),
        ];
        if let Some(t) = &token {
            q.push(("page_token", t.clone()));
        }
        match app.get(&format!("/v1beta1/options/snapshots/{sym}"), &q).await {
            Ok(v) => {
                contracts.extend(chain::contracts(&v));
                token = v["next_page_token"].as_str().map(str::to_owned);
            }
            Err((StatusCode::TOO_MANY_REQUESTS, _)) => return,
            Err((_, e)) => {
                m.note = Some(e);
                break;
            }
        }
        if token.is_none() {
            break;
        }
    }
    if contracts.is_empty() && m.note.is_none() {
        m.note = Some("no options quoted".into());
    }
    let exps = chain::expiries(&contracts, Some(spot), &|x| years_to_close(x, now));
    let (iv, mv) = screener::pick(&exps, &today);
    (m.atm_iv_30d, m.iv_expiry) = iv.map_or((None, None), |(v, x)| (Some(v), Some(x)));
    // Skew at the 30-day expiry (25-delta risk reversal), when its implied distribution is usable.
    m.skew = m.iv_expiry.as_deref().and_then(|x| {
        let d = &exps.iter().find(|e| e["expiry"] == x)?["distribution"];
        (d["quality"] != "poor").then(|| d["rr25"].as_f64()).flatten()
    });
    (m.implied_move, m.move_expiry) = mv.map_or((None, None), |(v, x)| (Some(v), Some(x)));
    lock(&app.screen).options.insert(sym.to_owned(), (Instant::now(), m));
}

#[tokio::main]
async fn main() {
    let args = Args::parse();
    let _ = rustls::crypto::ring::default_provider().install_default();
    let (key, secret) = match (secret("APCA_API_KEY_ID"), secret("APCA_API_SECRET_KEY")) {
        (Ok(k), Ok(s)) => (k, s),
        (Err(e), _) | (_, Err(e)) => {
            eprintln!("alpaca-data: {e}");
            std::process::exit(2);
        }
    };
    let observer = Observer::new("alpaca-data", env!("CARGO_PKG_VERSION"));
    observer.set_state(State::Ready, "waiting for requests");
    observer.describe(json!({
        "role": "Market data for any symbol on request: stock snapshots, bars and option chains with IV and greeks, from Alpaca.",
        "upstream": [{ "id": "alpaca-data-api", "label": "Alpaca", "sub": format!("{} stocks, {} options", args.feed, args.options_feed), "kind": "flow", "via": "https" }],
        "streams": { "bars": "/bars", "snapshots": "/snapshots", "chain": "/chain", "news_search": "/news", "option_bars": "/option_bars", "option_contracts": "/option_contracts", "screener": "/screener" },
    }));
    let app = Arc::new(App {
        http: reqwest::Client::builder()
            .timeout(Duration::from_secs(20))
            .build()
            .expect("an HTTP client"),
        key,
        secret,
        feed: args.feed,
        options_feed: args.options_feed,
        history_feed: args.history_feed,
        observer: observer.clone(),
        sent: Mutex::default(),
        cache: Mutex::default(),
        screen: Mutex::default(),
    });
    tokio::spawn(scan(app.clone()));
    {
        let a = app.clone();
        observer.set_metrics(move |w| {
            w.gauge(
                "tinytrader_alpaca_data_requests_last_minute",
                "Requests sent to Alpaca in the last minute.",
                &[],
                lock(&a.sent).len() as f64,
            );
            w.gauge(
                "tinytrader_alpaca_data_cached",
                "Answers kept.",
                &[],
                lock(&a.cache).len() as f64,
            );
        });
    }
    let router = Router::new()
        .route("/snapshots", get(snapshots))
        .route("/bars", get(bars))
        .route("/chain", get(option_chain))
        .route("/news", get(news))
        .route("/option_bars", get(option_bars))
        .route("/option_contracts", get(option_contracts))
        .route("/screener", get(screen))
        .with_state(app)
        .merge(observer.router());
    let listen = listen_address("ALPACA_DATA_LISTEN", "127.0.0.1:9123");
    let listener = match tokio::net::TcpListener::bind(&listen).await {
        Ok(l) => l,
        Err(e) => {
            eprintln!("alpaca-data: cannot listen on {listen}: {e}");
            std::process::exit(2);
        }
    };
    eprintln!("alpaca-data: serving http://{listen}");
    if let Err(e) = axum::serve(listener, router).await {
        eprintln!("alpaca-data: http server stopped: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_alpaca_times() {
        assert_eq!(unix("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(unix("2026-10-07T19:04:00Z"), Some(1_791_399_840));
        assert_eq!(unix("2026-10"), None);
    }

    #[test]
    fn older_history_ends_where_asked() {
        // Now: SIP history to the cutoff, IEX after.
        assert_eq!(
            feed_windows("sip", "iex", 1000, None),
            vec![("sip".into(), None, Some(1000)), ("iex".into(), Some(1000), None)]
        );
        // An older chunk: SIP alone, up to its end.
        assert_eq!(feed_windows("sip", "iex", 1000, Some(400)), vec![("sip".into(), None, Some(400))]);
        // A chunk ending after the cutoff: both, the second ending there.
        assert_eq!(
            feed_windows("sip", "iex", 1000, Some(1200)),
            vec![("sip".into(), None, Some(1000)), ("iex".into(), Some(1000), Some(1200))]
        );
        // One feed.
        assert_eq!(feed_windows("iex", "iex", 1000, Some(400)), vec![("iex".into(), None, Some(400))]);
    }

    #[test]
    fn keeps_the_last_trading_days() {
        // Two minute bars on each of three New York dates (13:30 UTC is 09:30 ET).
        let at = |d: i64, n: i64| json!([1_791_034_200 + d * 86_400 + n * 60, 1, 1, 1, 1, 1]);
        let bars: Vec<Value> = (0..3).flat_map(|d| (0..2).map(move |n| at(d, n))).collect();
        assert_eq!(last_days(&bars, 1, false), 2);
        assert_eq!(last_days(&bars, 2, false), 4);
        assert_eq!(last_days(&bars, 9, false), 6);
        assert_eq!(last_days(&bars, 2, true), 2);
    }

    #[test]
    fn writes_rfc3339_times() {
        assert_eq!(rfc3339(0), "1970-01-01T00:00:00Z");
        assert_eq!(rfc3339(1_791_399_840), "2026-10-07T19:04:00Z");
        assert_eq!(unix(&rfc3339(1_709_164_800)), Some(1_709_164_800)); // 29 February 2024
    }

    #[test]
    fn symbols_are_checked() {
        assert!(symbol_ok("BRK.B") && symbol_ok("XLE"));
        assert!(!symbol_ok("aapl") && !symbol_ok("A/B") && !symbol_ok(""));
    }
}

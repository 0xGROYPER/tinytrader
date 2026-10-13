//! Example service: reads Alpaca brokerage accounts (paper or live) over REST
//! and serves their balances and open positions. It only reads; it never
//! places, changes or cancels orders. One process reads any number of
//! accounts.
//!
//! Serves `/accounts`, one entry per account:
//!
//! ```json
//! { "accounts": [
//!   { "name": "paper-1", "label": "Paper 1", "environment": "paper", "updated_at": 1759700000.0,
//!     "account": { "status": "ACTIVE", "currency": "USD", "equity": 100000.0,
//!                  "last_equity": 99500.0, "cash": 50000.0, "buying_power": 200000.0, ... },
//!     "positions": [{ "symbol": "SPY", "qty": 10.0, "market_value": 6690.0, ... }],
//!     "orders": [{ "client_order_id": "tt-...", "status": "new", "order_class": "mleg",
//!                  "qty": 1.0, "limit_price": -0.54, "created_at": "...",
//!                  "legs": [{ "symbol": "SPY261012C00777000", "side": "sell", "ratio_qty": 1.0 }] }] } ] }
//! ```
//!
//! `orders` are the account's open orders, however they were placed (the
//! console, another client, Alpaca's app), replacements included; `null`
//! when they couldn't be read.
//!
//! Also serves `/calendar`, the US market clock and trading sessions from
//! a week ago to six weeks ahead (`{ "clock": { "is_open", "next_open",
//! "next_close", "timestamp" }, "days": [{ "date", "open", "close" }] }`,
//! times in New York), refreshed every ten minutes.
//!
//! Numeric fields are numbers, not Alpaca's strings. Account numbers and
//! Alpaca's ids are never served (an order's own client id is). Each account's equity is kept as a series at
//! `/recent` (keyed by its label), and position changes are a live tail at
//! `/events`.
//!
//! Credentials: with `--accounts`, each account `NAME` reads its key from
//! `<secrets>/NAME_key_id` and `<secrets>/NAME_secret_key`. Without it, one
//! account reads `APCA_API_KEY_ID` and `APCA_API_SECRET_KEY` (or the same
//! names with a `_FILE` suffix pointing at files containing them).

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tinytrader_observe::{lock, secret};

use axum::Router;
use axum::routing::get;
use clap::{Parser, ValueEnum};
use serde_json::{Map, Value, json};
mod live;

use tinytrader_observe::{Events, Observer, Series, State, listen_address, unix_seconds};

/// Account fields served, as Alpaca names them. Everything else, notably
/// the account number and id, is dropped.
const ACCOUNT_FIELDS: &[&str] = &[
    "status",
    "currency",
    "equity",
    "last_equity",
    "cash",
    "buying_power",
    "long_market_value",
    "short_market_value",
    "initial_margin",
    "maintenance_margin",
    "daytrade_count",
    "pattern_day_trader",
    "trading_blocked",
    "account_blocked",
];

const POSITION_FIELDS: &[&str] = &[
    "symbol",
    "asset_class",
    "side",
    "qty",
    "avg_entry_price",
    "current_price",
    "lastday_price",
    "market_value",
    "cost_basis",
    "unrealized_pl",
    "unrealized_plpc",
    "unrealized_intraday_pl",
    "unrealized_intraday_plpc",
    "change_today",
];

const ORDER_FIELDS: &[&str] = &[
    "client_order_id",
    "created_at",
    "updated_at",
    "symbol",
    "side",
    "qty",
    "filled_qty",
    "type",
    "order_class",
    "time_in_force",
    "limit_price",
    "stop_price",
    "status",
];

const LEG_FIELDS: &[&str] = &["symbol", "side", "ratio_qty", "status"];

#[derive(Clone, Copy, Debug, ValueEnum)]
enum Environment {
    Paper,
    Live,
}

impl Environment {
    fn base(self) -> &'static str {
        match self {
            Self::Paper => "https://paper-api.alpaca.markets",
            Self::Live => "https://api.alpaca.markets",
        }
    }
    fn stream(self) -> &'static str {
        match self {
            Self::Paper => "wss://paper-api.alpaca.markets/stream",
            Self::Live => "wss://api.alpaca.markets/stream",
        }
    }
    fn name(self) -> &'static str {
        match self {
            Self::Paper => "paper",
            Self::Live => "live",
        }
    }
}

/// Reads Alpaca accounts' balances and positions and serves them.
#[derive(Parser, Debug)]
#[command(version, about)]
struct Args {
    /// Accounts to read, as `NAME=LABEL` separated by `;`, e.g.
    /// `live=Live;paper-1=Paper 1`. Without it, one account is read from
    /// `APCA_API_KEY_ID` and `APCA_API_SECRET_KEY`.
    #[arg(long, env = "ALPACA_ACCOUNTS")]
    accounts: Option<String>,

    /// Where each account's key files are (`NAME_key_id`, `NAME_secret_key`).
    #[arg(long, env = "ALPACA_ACCOUNT_SECRETS", default_value = "/run/secrets")]
    secrets: String,

    /// Name shown for the single account read from the environment.
    #[arg(long, env = "ALPACA_ACCOUNT_LABEL")]
    label: Option<String>,

    /// Active hours in New York time, e.g. `07:55-17:05`, relative to the
    /// regular session on trading days (from Alpaca's calendar). Outside them
    /// the service reports itself idle and calls nothing; it reads every
    /// account once when it starts. Always active when unset.
    #[arg(long, env = "ALPACA_ACCOUNT_HOURS")]
    hours: Option<tinytrader_schedule::Schedule>,

    /// Relays that price positions, as `stocks=ADDR` and `ROOT=ADDR` entries
    /// separated by `;`, e.g. `stocks=relay-iex:7610;XSP=relay-opra-xsp:7602`.
    /// Positions they don't cover keep Alpaca's prices.
    #[arg(long, env = "ALPACA_ACCOUNT_RELAYS", default_value = "")]
    relays: String,

    /// Seconds between full re-reads of every account while active; order
    /// events trigger a re-read of their account at once.
    #[arg(long, env = "ALPACA_ACCOUNT_REFRESH", default_value_t = 600)]
    refresh: u64,
}

/// One account and its latest reading.
struct Account {
    name: String,
    label: String,
    env: Environment,
    key: String,
    secret: String,
    doc: Mutex<Value>,
    polls: AtomicU64,
    errors: AtomicU64,
    /// Whether the equity history has been loaded since start.
    history: std::sync::atomic::AtomicBool,
}

impl Account {
    /// Paper or live, told apart by the key: Alpaca's paper keys start with `PK`.
    fn new(name: &str, label: &str, key: String, secret: String) -> Self {
        let env = if key.starts_with("PK") {
            Environment::Paper
        } else {
            Environment::Live
        };
        Self {
            name: name.to_owned(),
            label: label.to_owned(),
            env,
            doc: Mutex::new(json!({ "name": name, "label": label, "environment": env.name() })),
            key,
            secret,
            polls: AtomicU64::new(0),
            errors: AtomicU64::new(0),
            history: std::sync::atomic::AtomicBool::new(false),
        }
    }
}

struct Shared {
    accounts: Vec<Account>,
    calendar: Mutex<Value>,
    events: Events,
    /// Each account's equity, one point a minute: the latest session, loaded
    /// from Alpaca's portfolio history on start, then kept live.
    series: Series,
    /// When the current session's window opened (ns), once one has.
    session: Mutex<Option<u64>>,
}

impl Default for Shared {
    fn default() -> Self {
        Self {
            accounts: Vec::new(),
            calendar: Mutex::default(),
            events: Events::default(),
            series: Series::new(1500, 60_000, 16),
            session: Mutex::new(None),
        }
    }
}

/// The accounts to read, from `--accounts` and the secrets directory, or the
/// single account in the environment.
fn load_accounts(args: &Args) -> Result<Vec<Account>, String> {
    let Some(list) = args.accounts.as_deref().filter(|l| !l.trim().is_empty()) else {
        let label = args.label.clone().unwrap_or_else(|| "Alpaca".to_owned());
        return Ok(vec![Account::new(
            "account",
            &label,
            secret("APCA_API_KEY_ID")?,
            secret("APCA_API_SECRET_KEY")?,
        )]);
    };
    let read = |file: String| {
        std::fs::read_to_string(&file)
            .map(|s| s.trim().to_owned())
            .map_err(|e| format!("cannot read {file}: {e}"))
    };
    list.split(';')
        .map(str::trim)
        .filter(|e| !e.is_empty())
        .map(|entry| {
            let (name, label) = entry.split_once('=').unwrap_or((entry, entry));
            let (name, label) = (name.trim(), label.trim());
            Ok(Account::new(
                name,
                label,
                read(format!("{}/{name}_key_id", args.secrets))?,
                read(format!("{}/{name}_secret_key", args.secrets))?,
            ))
        })
        .collect()
}

#[tokio::main]
async fn main() {
    // One TLS crypto provider for the trade stream and HTTP clients.
    let _ = rustls::crypto::ring::default_provider().install_default();
    let args = Args::parse();
    let accounts = match load_accounts(&args) {
        Ok(a) if !a.is_empty() => a,
        Ok(_) => {
            eprintln!("alpaca-account: no accounts given");
            std::process::exit(2);
        }
        Err(e) => {
            eprintln!("alpaca-account: {e}");
            std::process::exit(2);
        }
    };
    let listen = listen_address("ALPACA_ACCOUNT_LISTEN", "127.0.0.1:9107");
    let observer = Observer::new("alpaca-account", env!("CARGO_PKG_VERSION"));
    let names: Vec<String> = accounts
        .iter()
        .map(|a| format!("{} ({})", a.label, a.env.name()))
        .collect();
    let shared = Arc::new(Shared {
        accounts,
        ..Shared::default()
    });
    observer.describe(json!({
        "role": format!("Balances and positions of Alpaca accounts: {}. Read only.", names.join(", ")),
        "upstream": [{ "id": "alpaca-trading", "label": "alpaca", "sub": "trading API", "kind": "flow", "via": "REST, read only" }],
        "streams": { "events": "/events", "series": "/recent", "accounts": "/accounts", "calendar": "/calendar" },
        "schedule": [
            match args.hours {
                Some(h) => json!({ "name": "Account session: order events and relay prices", "kind": "window", "hours": h.to_string(), "when": format!("{h} ET on trading days") }),
                None => json!({ "name": "Account session: order events and relay prices", "kind": "always", "when": "always" }),
            },
            { "name": "Full read of every account", "kind": "interval", "every_seconds": args.refresh, "when": format!("every {} min during the session", args.refresh / 60) },
            { "name": "Market calendar", "kind": "event", "when": "read at start, a year ahead" },
        ],
    }));
    {
        let s = shared.clone();
        observer.set_metrics(move |w| {
            for a in &s.accounts {
                let l = [("account", a.name.as_str()), ("environment", a.env.name())];
                w.counter(
                    "tinytrader_account_polls_total",
                    "Polls of the account.",
                    &l,
                    a.polls.load(Ordering::Relaxed) as f64,
                );
                w.counter(
                    "tinytrader_account_errors_total",
                    "Failed polls.",
                    &l,
                    a.errors.load(Ordering::Relaxed) as f64,
                );
                let doc = lock(&a.doc);
                if let Some(at) = doc["updated_at"].as_f64() {
                    w.gauge(
                        "tinytrader_account_last_poll_age_seconds",
                        "Seconds since the last successful poll.",
                        &l,
                        unix_seconds() - at,
                    );
                }
                if let Some(n) = doc["positions"].as_array().map(Vec::len) {
                    w.gauge(
                        "tinytrader_account_positions",
                        "Open positions.",
                        &l,
                        n as f64,
                    );
                }
            }
        });
    }

    let app = Router::new()
        .route(
            "/accounts",
            get({
                let s = shared.clone();
                move || async move {
                    let docs: Vec<Value> =
                        s.accounts.iter().map(|a| lock(&a.doc).clone()).collect();
                    axum::Json(json!({ "accounts": docs }))
                }
            }),
        )
        .route(
            "/calendar",
            get({
                let s = shared.clone();
                move || async move { axum::Json(lock(&s.calendar).clone()) }
            }),
        )
        .merge(observer.router())
        .merge(shared.events.router())
        .merge(shared.series.router());
    let listener = match tokio::net::TcpListener::bind(&listen).await {
        Ok(l) => l,
        Err(e) => {
            eprintln!("alpaca-account: cannot listen on {listen}: {e}");
            std::process::exit(2);
        }
    };
    eprintln!("alpaca-account: {} on http://{listen}", names.join(", "));
    tokio::spawn(async move {
        if let Err(e) = axum::serve(listener, app).await {
            eprintln!("alpaca-account: http server stopped: {e}");
        }
    });

    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        .build()
        .expect("http client");
    let relays = live::Relays::parse(&args.relays);
    let prices = Arc::new(live::Prices::default());
    let feeds = live::Feeds::new(prices.clone());
    let refresh = Duration::from_secs(args.refresh.max(30));
    let mut first = true;
    loop {
        // Outside active hours, call nothing: wait for the next window, using
        // the calendar already read. The first pass always reads, so the
        // accounts are known even when the service starts after hours.
        let mut end = u64::MAX;
        let mut opened = None;
        if let Some(hours) = args.hours {
            let now = (unix_seconds() * 1e9) as u64;
            let calendar: tinytrader_schedule::Calendar =
                serde_json::from_value(lock(&shared.calendar).clone()).unwrap_or_default();
            match hours.next(now, &calendar) {
                Some((start, e)) if start <= now => {
                    end = e;
                    opened = Some(start);
                }
                next if !first => {
                    let detail = match next {
                        Some((start, _)) => format!(
                            "off schedule until {}",
                            tinytrader_schedule::describe(start)
                        ),
                        None => "off schedule: no trading day in the next two weeks".to_owned(),
                    };
                    observer.set_state(State::Idle, &detail);
                    let wait = next.map_or(Duration::from_secs(3600), |(start, _)| {
                        Duration::from_nanos(start.saturating_sub(now))
                            .min(Duration::from_secs(300))
                    });
                    tokio::time::sleep(wait).await;
                    continue;
                }
                _ => end = 0, // starting after hours: read once, then idle
            }
        }
        first = false;
        // The calendar is the same for every account; the first one reads it.
        let a = &shared.accounts[0];
        match read_calendar(&http, a.env, &a.key, &a.secret).await {
            Ok(c) => *lock(&shared.calendar) = c,
            Err(e) => eprintln!("alpaca-account: calendar: {e}"),
        }
        // A new session starts each account's curve afresh, from that day.
        if opened.is_some() && opened != *lock(&shared.session) {
            *lock(&shared.session) = opened;
            for a in &shared.accounts {
                shared.series.clear(&a.label);
                a.history.store(false, Ordering::Relaxed);
            }
        }
        read_all(&shared, &http, &observer).await;
        if end == 0 {
            continue;
        }
        // Active: order events re-read their account; relay prices value the
        // positions every second.
        let (tx, mut rx) = tokio::sync::mpsc::channel(64);
        let listeners: Vec<_> = shared
            .accounts
            .iter()
            .enumerate()
            .map(|(i, a)| {
                tokio::spawn(live::trade_updates(
                    a.env.stream().to_owned(),
                    a.key.clone(),
                    a.secret.clone(),
                    i,
                    tx.clone(),
                ))
            })
            .collect();
        feeds.want(&live::wanted(&docs(&shared), &relays));
        let mut tick = tokio::time::interval(Duration::from_secs(1));
        let mut last_full = std::time::Instant::now();
        loop {
            let now = (unix_seconds() * 1e9) as u64;
            if now >= end {
                break;
            }
            tokio::select! {
                Some(i) = rx.recv() => {
                    let a = &shared.accounts[i];
                    read_one(&shared, a, &http, &observer).await;
                    feeds.want(&live::wanted(&docs(&shared), &relays));
                }
                _ = tick.tick() => {
                    if last_full.elapsed() >= refresh {
                        read_all(&shared, &http, &observer).await;
                        feeds.want(&live::wanted(&docs(&shared), &relays));
                        last_full = std::time::Instant::now();
                    }
                    for a in &shared.accounts {
                        let mut doc = lock(&a.doc);
                        live::revalue(&mut doc, &relays, &prices);
                        if let Some(e) = doc["account"]["equity"].as_f64() {
                            shared.series.push(&a.label, e);
                        }
                    }
                }
            }
        }
        for l in listeners {
            l.abort();
        }
        feeds.want(&std::collections::HashSet::new());
    }
}

/// Every account's current document.
fn docs(shared: &Shared) -> Vec<Value> {
    shared
        .accounts
        .iter()
        .map(|a| lock(&a.doc).clone())
        .collect()
}

/// Read one account's balances and positions from Alpaca.
async fn read_one(
    shared: &Shared,
    a: &Account,
    http: &reqwest::Client,
    observer: &Observer,
) -> bool {
    let read = async {
        let account = get_json(http, a.env, &a.key, &a.secret, "/v2/account").await?;
        let positions = get_json(http, a.env, &a.key, &a.secret, "/v2/positions").await?;
        Ok::<_, String>((account, positions))
    };
    match read.await {
        Ok((account, positions)) => {
            a.polls.fetch_add(1, Ordering::Relaxed);
            update(shared, a, &account, &positions);
            // Open orders, however placed; a failure here leaves them unknown, not empty.
            let orders = get_json(
                http,
                a.env,
                &a.key,
                &a.secret,
                "/v2/orders?status=open&nested=true&limit=100",
            )
            .await;
            if let Err(e) = &orders {
                eprintln!("alpaca-account: {}: {e}", a.label);
            }
            lock(&a.doc)["orders"] = orders.map_or(Value::Null, |o| open_orders(&o));
            if !a.history.load(Ordering::Relaxed) {
                load_history(shared, a, http).await;
            }
            if let Some(e) = lock(&a.doc)["account"]["equity"].as_f64() {
                shared.series.push(&a.label, e);
            }
            true
        }
        Err(e) => {
            a.errors.fetch_add(1, Ordering::Relaxed);
            eprintln!("alpaca-account: {}: {e}", a.label);
            observer.record_error(&format!("{}: {e}", a.label));
            false
        }
    }
}

/// Load the account's equity over its latest session (extended hours, one
/// point a minute) from Alpaca's portfolio history, so a restart doesn't lose
/// the curve. Alpaca's history uses its own marks; live points use ours.
async fn load_history(shared: &Shared, a: &Account, http: &reqwest::Client) {
    // In a session, from four hours before its window opened (04:00 New York
    // time for a 07:55 window, never the day before); otherwise the latest
    // session Alpaca has.
    let path = match *lock(&shared.session) {
        Some(start) => format!(
            "/v2/account/portfolio/history?start={}&timeframe=1Min&intraday_reporting=extended_hours",
            rfc3339((start / 1_000_000_000).saturating_sub(4 * 3600))
        ),
        None => "/v2/account/portfolio/history?period=1D&timeframe=1Min&intraday_reporting=extended_hours".to_owned(),
    };
    match get_json(http, a.env, &a.key, &a.secret, &path).await {
        Ok(h) => {
            let times = h["timestamp"].as_array().cloned().unwrap_or_default();
            let equity = h["equity"].as_array().cloned().unwrap_or_default();
            let points: Vec<(f64, f64)> = times
                .iter()
                .zip(&equity)
                .filter_map(|(t, e)| Some((t.as_f64()? * 1000.0, e.as_f64()?)))
                .filter(|(_, e)| *e > 0.0)
                .collect();
            shared.series.seed(&a.label, &points);
            a.history.store(true, Ordering::Relaxed);
        }
        Err(e) => eprintln!("alpaca-account: {}: no equity history: {e}", a.label),
    }
}

/// Seconds since the epoch as `YYYY-MM-DDTHH:MM:SSZ`.
fn rfc3339(secs: u64) -> String {
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    format!(
        "{}T{:02}:{:02}:{:02}Z",
        civil_date(days),
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}

/// Read every account, and report how that went.
async fn read_all(shared: &Shared, http: &reqwest::Client, observer: &Observer) {
    let mut failing = Vec::new();
    for a in &shared.accounts {
        if !read_one(shared, a, http, observer).await {
            failing.push(a.label.clone());
        }
    }
    let n = shared.accounts.len();
    if failing.is_empty() {
        observer.set_state(
            State::Ready,
            &format!("{n} account{}", if n == 1 { "" } else { "s" }),
        );
    } else {
        observer.set_state(
            State::Degraded,
            &format!("unreachable: {}; retrying", failing.join(", ")),
        );
    }
}

async fn get_json(
    http: &reqwest::Client,
    env: Environment,
    key: &str,
    secret: &str,
    path: &str,
) -> Result<Value, String> {
    let r = http
        .get(format!("{}{path}", env.base()))
        .header("APCA-API-KEY-ID", key)
        .header("APCA-API-SECRET-KEY", secret)
        .send()
        .await
        .map_err(|e| format!("{path}: request failed: {e}"))?;
    let status = r.status();
    if !status.is_success() {
        return Err(format!("{path}: Alpaca answered {status}"));
    }
    r.json()
        .await
        .map_err(|e| format!("{path}: unreadable response: {e}"))
}

/// The market clock, and trading sessions from a week ago to six weeks ahead.
async fn read_calendar(
    http: &reqwest::Client,
    env: Environment,
    key: &str,
    secret: &str,
) -> Result<Value, String> {
    let clock = get_json(http, env, key, secret, "/v2/clock").await?;
    let today = (unix_seconds() / 86_400.0) as i64;
    let path = format!(
        "/v2/calendar?start={}&end={}",
        civil_date(today - 7),
        civil_date(today + 400)
    );
    let days = get_json(http, env, key, secret, &path).await?;
    let days: Vec<Value> = days
        .as_array()
        .into_iter()
        .flatten()
        .map(|d| json!({ "date": d["date"], "open": d["open"], "close": d["close"] }))
        .collect();
    Ok(json!({
        "updated_at": unix_seconds(),
        "clock": {
            "is_open": clock["is_open"],
            "timestamp": clock["timestamp"],
            "next_open": clock["next_open"],
            "next_close": clock["next_close"],
        },
        "days": days,
    }))
}

/// `YYYY-MM-DD` for a count of days since 1970-01-01 (Howard Hinnant's
/// civil-from-days).
fn civil_date(days: i64) -> String {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}")
}

/// Keep the allowed fields, as numbers where Alpaca sends numeric strings.
fn pick(v: &Value, fields: &[&str]) -> Value {
    let mut out = Map::new();
    for f in fields {
        let x = &v[*f];
        let x = match x.as_str().map(str::parse::<f64>) {
            Some(Ok(n)) if !matches!(*f, "symbol" | "status" | "currency" | "client_order_id") => {
                json!(n)
            }
            _ => x.clone(),
        };
        if !x.is_null() {
            out.insert((*f).to_owned(), x);
        }
    }
    Value::Object(out)
}

/// Open orders, oldest first, each with its legs (a multi-leg order's).
fn open_orders(orders: &Value) -> Value {
    let mut out: Vec<Value> = orders
        .as_array()
        .into_iter()
        .flatten()
        .map(|o| {
            let mut v = pick(o, ORDER_FIELDS);
            if let Some(legs) = o["legs"].as_array().filter(|l| !l.is_empty()) {
                v["legs"] = json!(legs.iter().map(|l| pick(l, LEG_FIELDS)).collect::<Vec<_>>());
            }
            v
        })
        .collect();
    out.sort_by(|a, b| a["created_at"].as_str().cmp(&b["created_at"].as_str()));
    json!(out)
}

/// Store a fresh reading, record equity, and announce position changes.
fn update(shared: &Shared, acct: &Account, account: &Value, positions: &Value) {
    let account = pick(account, ACCOUNT_FIELDS);
    let mut positions: Vec<Value> = positions
        .as_array()
        .into_iter()
        .flatten()
        .map(|p| pick(p, POSITION_FIELDS))
        .collect();
    positions.sort_by(|a, b| a["symbol"].as_str().cmp(&b["symbol"].as_str()));
    let mut doc = lock(&acct.doc);
    if let Some(before) = doc["positions"].as_array() {
        for line in changes(before, &positions) {
            let line = format!("{}: {line}", acct.label);
            shared.events.push(line.clone(), line);
        }
    }
    doc["account"] = account;
    doc["positions"] = json!(positions);
    doc["updated_at"] = json!(unix_seconds());
}

/// One line per position opened, closed or resized.
fn changes(before: &[Value], after: &[Value]) -> Vec<String> {
    let qty = |list: &[Value], sym: &str| {
        list.iter()
            .find(|p| p["symbol"] == sym)
            .and_then(|p| p["qty"].as_f64())
    };
    let mut out = Vec::new();
    for p in after {
        let sym = p["symbol"].as_str().unwrap_or("?");
        let now = p["qty"].as_f64().unwrap_or(0.0);
        match qty(before, sym) {
            None => out.push(format!("opened {sym}: {now}")),
            Some(was) if (was - now).abs() > f64::EPSILON => {
                out.push(format!("{sym}: {was} → {now}"));
            }
            Some(_) => {}
        }
    }
    for p in before {
        let sym = p["symbol"].as_str().unwrap_or("?");
        if qty(after, sym).is_none() {
            out.push(format!("closed {sym}"));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drops_identifiers_and_parses_numbers() {
        let a = pick(
            &json!({ "id": "x", "account_number": "PA1", "status": "ACTIVE", "equity": "1000.5", "pattern_day_trader": false }),
            ACCOUNT_FIELDS,
        );
        assert_eq!(
            a,
            json!({ "status": "ACTIVE", "equity": 1000.5, "pattern_day_trader": false })
        );
    }

    #[test]
    fn civil_dates() {
        assert_eq!(civil_date(0), "1970-01-01");
        assert_eq!(civil_date(20_366), "2025-10-05");
        assert_eq!(civil_date(19_782), "2024-02-29");
    }

    #[test]
    fn reports_position_changes() {
        let before = [
            json!({ "symbol": "AAPL", "qty": 5.0 }),
            json!({ "symbol": "SPY", "qty": 10.0 }),
        ];
        let after = [
            json!({ "symbol": "QQQ", "qty": 1.0 }),
            json!({ "symbol": "SPY", "qty": 4.0 }),
        ];
        assert_eq!(
            changes(&before, &after),
            ["opened QQQ: 1", "SPY: 10 → 4", "closed AAPL"]
        );
    }

    #[test]
    fn open_orders_keep_legs_and_drop_alpaca_ids() {
        let o = open_orders(&json!([
            { "id": "b", "client_order_id": "c2", "created_at": "2026-10-12T17:00:00Z", "symbol": "SPY",
              "side": "buy", "qty": "1", "limit_price": "0.5", "status": "new", "legs": null },
            { "id": "a", "client_order_id": "tt-1", "created_at": "2026-10-12T16:00:00Z", "symbol": "",
              "order_class": "mleg", "qty": "2", "limit_price": "-0.54", "status": "new",
              "legs": [{ "id": "l", "symbol": "SPY261012C00777000", "side": "sell", "ratio_qty": "1" }] }
        ]));
        assert_eq!(o[0]["client_order_id"], "tt-1");
        assert_eq!(o[0]["limit_price"], -0.54);
        assert_eq!(
            o[0]["legs"],
            json!([{ "symbol": "SPY261012C00777000", "side": "sell", "ratio_qty": 1.0 }])
        );
        assert!(o[0].get("id").is_none() && o[1].get("legs").is_none());
        assert_eq!(o[1]["qty"], 1.0);
    }
}

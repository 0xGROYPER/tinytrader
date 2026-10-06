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
//!     "positions": [{ "symbol": "SPY", "qty": 10.0, "market_value": 6690.0, ... }] } ] }
//! ```
//!
//! Also serves `/calendar`, the US market clock and trading sessions from
//! a week ago to six weeks ahead (`{ "clock": { "is_open", "next_open",
//! "next_close", "timestamp" }, "days": [{ "date", "open", "close" }] }`,
//! times in New York), refreshed every ten minutes.
//!
//! Numeric fields are numbers, not Alpaca's strings. Account numbers and
//! ids are never served. Each account's equity is kept as a series at
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

use axum::Router;
use axum::routing::get;
use clap::{Parser, ValueEnum};
use serde_json::{Map, Value, json};
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

    /// Seconds between polls of each account.
    #[arg(long, env = "ALPACA_ACCOUNT_INTERVAL", default_value_t = 15)]
    interval: u64,

    /// Active hours in New York time, e.g. `07:55-17:05`, relative to the
    /// regular session on trading days (from Alpaca's calendar). Outside them
    /// the service reports itself idle and calls nothing; it reads every
    /// account once when it starts. Always active when unset.
    #[arg(long, env = "ALPACA_ACCOUNT_HOURS")]
    hours: Option<tinytrader_schedule::Schedule>,
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
        }
    }
}

#[derive(Default)]
struct Shared {
    accounts: Vec<Account>,
    calendar: Mutex<Value>,
    events: Events,
    series: Series,
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
    let every = Duration::from_secs(args.interval.max(5));
    let mut calendar_due: Option<std::time::Instant> = None;
    let mut first = true;
    loop {
        // Outside active hours, call nothing: wait for the next window, using
        // the calendar already read. The first pass always reads, so the
        // accounts are known even when the service starts after hours.
        if let Some(hours) = args.hours
            && !first
        {
            let now = (unix_seconds() * 1e9) as u64;
            let calendar: tinytrader_schedule::Calendar =
                serde_json::from_value(lock(&shared.calendar).clone()).unwrap_or_default();
            match hours.next(now, &calendar) {
                Some((start, _)) if start > now => {
                    observer.set_state(
                        State::Idle,
                        &format!(
                            "off schedule until {}",
                            tinytrader_schedule::describe(start)
                        ),
                    );
                    // Read the calendar afresh when the window opens.
                    calendar_due = None;
                    tokio::time::sleep(
                        Duration::from_nanos(start - now).min(Duration::from_secs(300)),
                    )
                    .await;
                    continue;
                }
                None => {
                    observer.set_state(
                        State::Idle,
                        "off schedule: no trading day in the next two weeks",
                    );
                    tokio::time::sleep(Duration::from_secs(3600)).await;
                    continue;
                }
                Some(_) => {}
            }
        }
        first = false;
        // The calendar is the same for every account; the first one reads it.
        if calendar_due.is_none_or(|t| t.elapsed() > Duration::from_secs(600)) {
            let a = &shared.accounts[0];
            match read_calendar(&http, a.env, &a.key, &a.secret).await {
                Ok(c) => {
                    *lock(&shared.calendar) = c;
                    calendar_due = Some(std::time::Instant::now());
                }
                Err(e) => eprintln!("alpaca-account: calendar: {e}"),
            }
        }
        let mut failing = Vec::new();
        for a in &shared.accounts {
            let read = async {
                let account = get_json(&http, a.env, &a.key, &a.secret, "/v2/account").await?;
                let positions = get_json(&http, a.env, &a.key, &a.secret, "/v2/positions").await?;
                Ok::<_, String>((account, positions))
            };
            match read.await {
                Ok((account, positions)) => {
                    a.polls.fetch_add(1, Ordering::Relaxed);
                    update(&shared, a, &account, &positions);
                }
                Err(e) => {
                    a.errors.fetch_add(1, Ordering::Relaxed);
                    eprintln!("alpaca-account: {}: {e}", a.label);
                    observer.record_error(&format!("{}: {e}", a.label));
                    failing.push(a.label.clone());
                }
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
        tokio::time::sleep(every).await;
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
        civil_date(today + 45)
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
            Some(Ok(n)) if !matches!(*f, "symbol" | "status" | "currency") => json!(n),
            _ => x.clone(),
        };
        if !x.is_null() {
            out.insert((*f).to_owned(), x);
        }
    }
    Value::Object(out)
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
    if let Some(e) = account["equity"].as_f64() {
        shared.series.push(&acct.label, e);
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
}

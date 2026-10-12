//! Economic calendar: scheduled US economic releases and Fed decisions, from
//! the agencies' own published schedules.
//!
//! - BLS (CPI, the jobs report, PPI, JOLTS, ...): its iCalendar feed. BLS
//!   only answers requests whose User-Agent carries a contact address,
//!   given as `ECON_CONTACT`; without one, BLS is skipped and reported.
//! - BEA (GDP, PCE and personal income, trade): its iCalendar feed.
//! - The Federal Reserve: FOMC decisions from its meeting calendar page.
//! - Weekly jobless claims (Labor Department): Thursdays at 08:30 ET, by rule.
//! - Oil and gas (category "oil", `oil.rs`): API and EIA weekly inventories,
//!   EIA gas storage and the Baker Hughes rig count, by their weekly rules.
//!   Every other event is category "econ".
//!
//! - Earnings report dates (`earnings.rs`): Alpha Vantage, Finnhub and
//!   Nasdaq.com compared, each used when configured (its key set; Nasdaq
//!   needs none). Served at `/earnings` (`?from=&to=&min=`, by default a week
//!   back to three months ahead, medium and high: a company of $10 billion
//!   or more is medium, $200 billion or more high, and `EARNINGS_WATCH`
//!   symbols always high). Only BLS is sent the contact address.
//! - Past earnings per company (`history.rs`): `/history?symbol=`, report
//!   dates and times from SEC EDGAR (with `SEC_CONTACT`, the address the SEC
//!   requires in requests; sent to sec.gov only), EPS and estimates from
//!   Alpha Vantage, all kept on disk (`ECON_CACHE`) and fetched only when
//!   missing or a new report is due.
//!
//! Each source is read once a day; when a read fails, its last good copy
//! is kept and the failure reported. Serves `/events` (`?from=YYYY-MM-DD&
//! to=YYYY-MM-DD&min=low|medium|high`; by default a month back to six months
//! ahead, medium and high), and the usual `/health`, `/metrics` and `/describe`.

mod alphavantage;
mod earnings;
mod history;
mod oil;
mod sources;

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use axum::extract::{Query, State as AxState};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use clap::Parser;
use serde::Deserialize;
use serde_json::{Value, json};
use sources::{Event, Importance};
use tinytrader_observe::{Observer, State, listen_address, unix_seconds};
use tinytrader_schedule::ny_date_time;

const BLS_URL: &str = "https://www.bls.gov/schedule/news_release/bls.ics";
const BEA_URL: &str = "https://www.bea.gov/news/schedule/ics/online-calendar-subscription.ics";
const FED_URL: &str = "https://www.federalreserve.gov/monetarypolicy/fomccalendars.htm";

/// Serves scheduled US economic releases and Fed decisions.
#[derive(Parser, Debug)]
#[command(version, about)]
struct Args {
    /// A contact address for BLS, which refuses requests without one in the
    /// User-Agent. Without it, BLS releases are left out.
    #[arg(long, env = "ECON_CONTACT")]
    contact: Option<String>,

    /// Hours between reads of each source.
    #[arg(long, env = "ECON_REFRESH_HOURS", default_value_t = 24)]
    refresh_hours: u64,

    /// Companies whose earnings always count as high importance
    /// (comma-separated symbols).
    #[arg(long, env = "EARNINGS_WATCH", value_delimiter = ',')]
    watch: Vec<String>,

    /// How many days ahead to read Finnhub and Nasdaq (one request per day
    /// each); Alpha Vantage always gives three months.
    #[arg(long, env = "EARNINGS_DAYS", default_value_t = 42)]
    earnings_days: i64,

    /// A directory to keep companies' earnings histories in across restarts.
    #[arg(long, env = "ECON_CACHE")]
    cache: Option<std::path::PathBuf>,
}

/// A source's last good events, and how its last read went.
struct Source<T = Event> {
    events: Vec<T>,
    error: Option<String>,
    read_at: f64,
}

impl<T> Default for Source<T> {
    fn default() -> Self {
        Self {
            events: Vec::new(),
            error: None,
            read_at: 0.0,
        }
    }
}

type Sources = HashMap<&'static str, Source>;

/// Earnings: each source's last good rows and how its last read went, and
/// the reports merged from them.
#[derive(Default)]
struct Earnings {
    sources: HashMap<&'static str, Source<earnings::Row>>,
    reports: Vec<earnings::Report>,
    watch: Vec<String>,
}

#[derive(Default)]
struct Shared {
    sources: Mutex<Sources>,
    earnings: Mutex<Earnings>,
}

impl Shared {
    fn lock(&self) -> std::sync::MutexGuard<'_, Sources> {
        self.sources.lock().unwrap_or_else(PoisonError::into_inner)
    }
    fn earnings(&self) -> std::sync::MutexGuard<'_, Earnings> {
        self.earnings.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

#[tokio::main]
async fn main() {
    let args = Args::parse();
    let _ = rustls::crypto::ring::default_provider().install_default();
    let shared = Arc::new(Shared::default());
    shared.earnings().watch = args.watch.iter().map(|w| w.trim().to_uppercase()).collect();
    let keys = Keys {
        alpha_vantage: tinytrader_observe::secret("ALPHAVANTAGE_API_KEY").ok(),
        finnhub: tinytrader_observe::secret("FINNHUB_API_KEY").ok(),
    };
    let budget = Arc::new(alphavantage::Budget::new(args.cache.as_deref()));
    let mut upstream = vec![
        json!({ "id": "bls", "label": "BLS", "sub": "release schedule", "kind": "flow", "via": "https" }),
        json!({ "id": "bea", "label": "BEA", "sub": "release schedule", "kind": "flow", "via": "https" }),
        json!({ "id": "fed", "label": "Federal Reserve", "sub": "FOMC calendar", "kind": "flow", "via": "https" }),
        json!({ "id": "nasdaq", "label": "Nasdaq.com", "sub": "earnings calendar", "kind": "flow", "via": "https" }),
    ];
    if keys.alpha_vantage.is_some() {
        upstream.push(json!({ "id": "alphavantage", "label": "Alpha Vantage", "sub": "earnings calendar", "kind": "flow", "via": "https" }));
    }
    if keys.finnhub.is_some() {
        upstream.push(json!({ "id": "finnhub", "label": "Finnhub", "sub": "earnings calendar", "kind": "flow", "via": "https" }));
    }
    let observer = Observer::new("econ-calendar", env!("CARGO_PKG_VERSION"));
    observer.set_state(State::Starting, "reading the agencies' schedules");
    observer.describe(json!({
        "role": "Scheduled US economic releases (BLS, BEA, jobless claims) and FOMC decisions, from the agencies' own schedules, and earnings dates compared across calendars.",
        "upstream": upstream,
        "streams": { "econ": "/events", "earnings": "/earnings", "earnings_history": "/history" },
        "schedule": [{ "name": "Read the agencies' schedules and the earnings calendars", "kind": "interval", "every_seconds": args.refresh_hours * 3600, "when": format!("every {} h", args.refresh_hours) }],
    }));
    {
        let s = shared.clone();
        observer.set_metrics(move |w| {
            for (name, Source { events, error, .. }) in s.lock().iter() {
                let labels = [("source", *name)];
                w.gauge(
                    "tinytrader_econ_events",
                    "Events held from a source.",
                    &labels,
                    events.len() as f64,
                );
                w.gauge(
                    "tinytrader_econ_source_failing",
                    "1 when a source's last read failed.",
                    &labels,
                    f64::from(u8::from(error.is_some())),
                );
            }
        });
    }
    let app = Router::new()
        .route("/events", get(events))
        .route("/earnings", get(earnings_list))
        .route(
            "/history",
            get({
                let h = Arc::new(history::History::new(
                    keys.alpha_vantage.clone(),
                    tinytrader_observe::secret("SEC_CONTACT").ok(),
                    args.cache.clone(),
                    budget.clone(),
                ));
                move |Query(ask): Query<HistoryAsk>| async move {
                    match h.get(&ask.symbol).await {
                        Ok(v) => Json(v).into_response(),
                        Err((code, e)) => (code, Json(json!({ "error": e }))).into_response(),
                    }
                }
            }),
        )
        .with_state(shared.clone())
        .merge(observer.router());
    let listen = listen_address("ECON_LISTEN", "127.0.0.1:9122");
    let listener = match tokio::net::TcpListener::bind(&listen).await {
        Ok(l) => l,
        Err(e) => {
            eprintln!("econ-calendar: cannot listen on {listen}: {e}");
            std::process::exit(2);
        }
    };
    eprintln!("econ-calendar: serving http://{listen}");
    tokio::spawn(async move {
        if let Err(e) = axum::serve(listener, app).await {
            eprintln!("econ-calendar: http server stopped: {e}");
        }
    });

    let agent = match &args.contact {
        Some(c) => format!(
            "tinytrader-econ-calendar/{} ({c})",
            env!("CARGO_PKG_VERSION")
        ),
        None => format!("tinytrader-econ-calendar/{}", env!("CARGO_PKG_VERSION")),
    };
    let http = reqwest::Client::builder()
        .user_agent(agent)
        .timeout(Duration::from_secs(30))
        .build()
        .expect("an HTTP client");
    // The earnings calendars get no contact address: only BLS asks for one.
    // Nasdaq.com answers only browser-like agents.
    let plain = reqwest::Client::builder()
        .user_agent(format!(
            "Mozilla/5.0 (compatible; tinytrader-econ-calendar/{})",
            env!("CARGO_PKG_VERSION")
        ))
        .timeout(Duration::from_secs(30))
        .build()
        .expect("an HTTP client");
    loop {
        read_all(&http, &shared, args.contact.is_some()).await;
        if shared.earnings().sources.is_empty() {
            observer.set_state(State::Ready, "reading the earnings calendars");
        }
        read_earnings(&plain, &shared, &keys, &budget, args.cache.as_deref(), args.earnings_days).await;
        let now = unix_seconds();
        let mut notes = Vec::new();
        let (mut failing, held) = {
            let sources = shared.lock();
            let failing: Vec<String> = sources
                .iter()
                .filter_map(|(n, src)| sort_failure(n, src, now, &mut notes))
                .collect();
            (
                failing,
                sources.values().map(|src| src.events.len()).sum::<usize>(),
            )
        };
        let reports = {
            let e = shared.earnings();
            failing.extend(
                e.sources
                    .iter()
                    .filter_map(|(n, src)| sort_failure(n, src, now, &mut notes)),
            );
            e.reports.len()
        };
        let mut held = format!("{held} events, {reports} earnings reports");
        if !notes.is_empty() {
            held = format!("{held}; {}", notes.join("; "));
        }
        if failing.is_empty() {
            observer.set_state(State::Ready, &held);
        } else {
            observer.set_state(State::Degraded, &format!("{held}; {}", failing.join("; ")));
        }
        tokio::time::sleep(Duration::from_secs(args.refresh_hours.max(1) * 3600)).await;
    }
}

/// How long a source that fails may serve its last good rows before the
/// service counts as degraded.
const STALE_AFTER: f64 = 3.0 * 86_400.0;

/// A source whose last read failed: a failure, or only a note while it still
/// holds rows read within [`STALE_AFTER`] (a daily limit, a brief outage).
fn sort_failure<T>(name: &str, src: &Source<T>, now: f64, notes: &mut Vec<String>) -> Option<String> {
    let err = src.error.as_ref()?;
    if !src.events.is_empty() && now - src.read_at < STALE_AFTER {
        let hours = (now - src.read_at) / 3600.0;
        notes.push(format!("{name}: {err} (serving rows from {hours:.0} h ago)"));
        None
    } else {
        Some(format!("{name}: {err}"))
    }
}

/// Read every source; a source that fails keeps its last good events.
async fn read_all(http: &reqwest::Client, shared: &Shared, bls: bool) {
    let get = |url: &'static str| async move {
        let r = http
            .get(url)
            .send()
            .await
            .map_err(|e| e.without_url().to_string())?;
        if !r.status().is_success() {
            return Err(format!("answered {}", r.status()));
        }
        r.text().await.map_err(|e| e.to_string())
    };
    let now = unix_seconds();
    let set = |name: &'static str, result: Result<Vec<Event>, String>| {
        let mut sources = shared.lock();
        let entry = sources.entry(name).or_default();
        match result {
            Ok(events) if !events.is_empty() => {
                *entry = Source {
                    events,
                    error: None,
                    read_at: now,
                }
            }
            Ok(_) => entry.error = Some("no events found (has the page changed?)".into()),
            Err(e) => entry.error = Some(e),
        }
    };
    if bls {
        set("BLS", get(BLS_URL).await.map(|t| sources::ics(&t, "BLS")));
    } else {
        set(
            "BLS",
            Err("skipped: BLS needs a contact address (ECON_CONTACT)".into()),
        );
    }
    set("BEA", get(BEA_URL).await.map(|t| sources::ics(&t, "BEA")));
    set("Fed", get(FED_URL).await.map(|t| sources::fomc(&t)));
    let t = now as i64;
    set(
        "DOL",
        Ok(sources::claims(t - 40 * 86_400, t + 200 * 86_400)),
    );
    set("Oil", Ok(oil::events(t - 40 * 86_400, t + 200 * 86_400)));
}

#[derive(Deserialize)]
struct HistoryAsk {
    symbol: String,
}

#[derive(Deserialize)]
struct Ask {
    from: Option<String>,
    to: Option<String>,
    min: Option<String>,
}

/// Before the first read, a 503 rather than an empty list, so a caller that
/// keeps what it gets (the console keeps it for minutes) asks again.
fn not_yet() -> Response {
    (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(json!({ "error": "still reading the sources" })),
    )
        .into_response()
}

async fn events(AxState(shared): AxState<Arc<Shared>>, Query(ask): Query<Ask>) -> Response {
    let now = unix_seconds() as i64;
    let day = |s: i64| ny_date_time(s).0;
    let from = ask.from.unwrap_or_else(|| day(now - 31 * 86_400));
    let to = ask.to.unwrap_or_else(|| day(now + 183 * 86_400));
    let min = ask
        .min
        .as_deref()
        .and_then(Importance::parse)
        .unwrap_or(Importance::Medium);
    let sources = shared.lock();
    if sources.is_empty() {
        return not_yet();
    }
    let mut list: Vec<&Event> = sources
        .values()
        .flat_map(|src| &src.events)
        .filter(|e| e.importance >= min)
        .collect();
    list.sort_by_key(|e| e.at);
    let events: Vec<Value> = list
        .into_iter()
        .filter_map(|e| {
            let (date, time, _) = ny_date_time(e.at);
            (date >= from && date <= to).then(|| json!({ "at": e.at, "date": date, "time": time, "title": e.title, "source": e.source, "importance": e.importance.as_str(), "category": e.category }))
        })
        .collect();
    let status: Value = sources
        .iter()
        .map(
            |(
                n,
                Source {
                    events: e,
                    error: err,
                    read_at: at,
                },
            )| {
                (
                    n.to_string(),
                    json!({ "events": e.len(), "error": err, "read_at": at }),
                )
            },
        )
        .collect::<serde_json::Map<_, _>>()
        .into();
    Json(
        json!({ "events": events, "sources": status, "from": from, "to": to, "min": min.as_str() }),
    )
    .into_response()
}

/// Keys for the earnings calendars that need one.
struct Keys {
    alpha_vantage: Option<String>,
    finnhub: Option<String>,
}

/// Read the earnings calendars and merge them; a source that fails keeps
/// its last good rows. Requests are a second apart, gently.
async fn read_earnings(
    http: &reqwest::Client,
    shared: &Shared,
    keys: &Keys,
    budget: &alphavantage::Budget,
    cache: Option<&std::path::Path>,
    days: i64,
) {
    async fn get(r: reqwest::RequestBuilder) -> Result<String, String> {
        // Errors without the URL, which may carry a key.
        let r = r.send().await.map_err(|e| e.without_url().to_string())?;
        if !r.status().is_success() {
            return Err(format!("answered {}", r.status()));
        }
        r.text().await.map_err(|e| e.without_url().to_string())
    }
    let json = |t: String| serde_json::from_str::<Value>(&t).map_err(|e| e.to_string());
    let pause = || tokio::time::sleep(Duration::from_millis(1100));
    let now = unix_seconds() as i64;
    let day = |d: i64| ny_date_time(now + d * 86_400).0;
    let mut got: Vec<(&'static str, Result<Vec<earnings::Row>, String>)> = Vec::new();

    if let Some(key) = &keys.alpha_vantage {
        // Its calendar covers three months, so a day-old copy (kept on disk
        // across restarts) is good enough; each read spends a request.
        let file = cache.map(|d| d.join("alphavantage").join("calendar.csv"));
        let kept = file.as_ref().and_then(|f| {
            let age = f.metadata().ok()?.modified().ok()?.elapsed().ok()?;
            (age < Duration::from_secs(20 * 3600)).then(|| std::fs::read_to_string(f).ok())?
        });
        let r = match kept {
            Some(t) => earnings::alpha_vantage(&t),
            None if !budget.take(alphavantage::LIMIT, now as f64) => {
                Err("Alpha Vantage's requests for the last 24 hours are used up".to_owned())
            }
            None => {
                let r = get(http.get("https://www.alphavantage.co/query").query(&[
                    ("function", "EARNINGS_CALENDAR"),
                    ("horizon", "3month"),
                    ("apikey", key),
                ]))
                .await;
                r.and_then(|t| {
                    let rows = earnings::alpha_vantage(&t)?;
                    if let Some(f) = file.as_ref().filter(|_| !rows.is_empty()) {
                        let _ = f.parent().map(std::fs::create_dir_all);
                        let tmp = f.with_extension("csv.tmp");
                        if std::fs::write(&tmp, &t).is_ok() {
                            let _ = std::fs::rename(&tmp, f);
                        }
                    }
                    Ok(rows)
                })
            }
        };
        got.push((earnings::ALPHA_VANTAGE, r));
    }
    if let Some(key) = &keys.finnhub {
        // A week at a time (each answer holds at most 1,500), from a week back
        // so recent reports carry their actual EPS.
        let mut rows = Ok(Vec::new());
        for start in (-7..days).step_by(7) {
            pause().await;
            let r = get(http
                .get("https://finnhub.io/api/v1/calendar/earnings")
                .header("X-Finnhub-Token", key)
                .query(&[("from", day(start)), ("to", day((start + 6).min(days)))]))
            .await
            .and_then(json)
            .and_then(|v| earnings::finnhub(&v));
            match (&mut rows, r) {
                (Ok(all), Ok(mut more)) => all.append(&mut more),
                (Ok(_), Err(e)) => rows = Err(e),
                _ => {}
            }
        }
        got.push((earnings::FINNHUB, rows));
    }
    let mut rows = Ok(Vec::new());
    for d in 0..days {
        let (date, _, weekday) = ny_date_time(now + d * 86_400);
        // No reports on weekends.
        if weekday == 0 || weekday == 6 {
            continue;
        }
        pause().await;
        let r = get(http
            .get("https://api.nasdaq.com/api/calendar/earnings")
            .header("Accept", "application/json")
            .query(&[("date", &date)]))
        .await
        .and_then(json)
        .and_then(|v| earnings::nasdaq(&v, &date));
        match (&mut rows, r) {
            (Ok(all), Ok(mut more)) => all.append(&mut more),
            (Ok(_), Err(e)) => rows = Err(format!("{date}: {e}")),
            _ => {}
        }
    }
    got.push((earnings::NASDAQ, rows));

    let mut e = shared.earnings();
    let read_at = unix_seconds();
    for (name, result) in got {
        let entry = e.sources.entry(name).or_default();
        match result {
            Ok(rows) if !rows.is_empty() => {
                *entry = Source {
                    events: rows,
                    error: None,
                    read_at,
                }
            }
            Ok(_) => entry.error = Some("no reports found (has the format changed?)".into()),
            Err(err) => entry.error = Some(err),
        }
    }
    let all: Vec<earnings::Row> = e.sources.values().flat_map(|s| s.events.clone()).collect();
    e.reports = earnings::merge(all);
}

/// How much a report matters: by company size, or always high when watched.
fn report_importance(r: &earnings::Report, watch: &[String]) -> Importance {
    match r.market_cap {
        _ if watch.contains(&r.symbol) => Importance::High,
        Some(c) if c >= 200e9 => Importance::High,
        Some(c) if c >= 10e9 => Importance::Medium,
        _ => Importance::Low,
    }
}

async fn earnings_list(AxState(shared): AxState<Arc<Shared>>, Query(ask): Query<Ask>) -> Response {
    let now = unix_seconds() as i64;
    let day = |s: i64| ny_date_time(s).0;
    let from = ask.from.unwrap_or_else(|| day(now - 7 * 86_400));
    let to = ask.to.unwrap_or_else(|| day(now + 92 * 86_400));
    let min = ask
        .min
        .as_deref()
        .and_then(Importance::parse)
        .unwrap_or(Importance::Medium);
    let e = shared.earnings();
    if e.sources.is_empty() {
        return not_yet();
    }
    let reports: Vec<Value> = e
        .reports
        .iter()
        .filter(|r| r.date >= from && r.date <= to)
        .filter_map(|r| {
            let importance = report_importance(r, &e.watch);
            (importance >= min).then(|| {
                json!({
                    "symbol": r.symbol, "name": r.name, "date": r.date, "when": r.when.as_str(),
                    "eps_estimate": r.eps_estimate, "eps_actual": r.eps_actual, "market_cap": r.market_cap,
                    "dates": r.dates, "disputed": r.disputed, "importance": importance.as_str(),
                })
            })
        })
        .collect();
    let sources: serde_json::Map<String, Value> = e
        .sources
        .iter()
        .map(|(n, s)| {
            (
                (*n).to_owned(),
                json!({ "rows": s.events.len(), "error": s.error, "read_at": s.read_at }),
            )
        })
        .collect();
    Json(
        json!({ "reports": reports, "sources": sources, "from": from, "to": to, "min": min.as_str() }),
    )
    .into_response()
}

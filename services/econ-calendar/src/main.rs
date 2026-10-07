//! Economic calendar: scheduled US economic releases and Fed decisions, from
//! the agencies' own published schedules.
//!
//! - BLS (CPI, the jobs report, PPI, JOLTS, ...): its iCalendar feed. BLS
//!   only answers requests whose User-Agent carries a contact address,
//!   given as `ECON_CONTACT`; without one, BLS is skipped and reported.
//! - BEA (GDP, PCE and personal income, trade): its iCalendar feed.
//! - The Federal Reserve: FOMC decisions from its meeting calendar page.
//! - Weekly jobless claims (Labor Department): Thursdays at 08:30 ET, by rule.
//!
//! Each source is read every few hours; when a read fails, its last good copy
//! is kept and the failure reported. Serves `/events` (`?from=YYYY-MM-DD&
//! to=YYYY-MM-DD&min=low|medium|high`; by default a month back to six months
//! ahead, medium and high), and the usual `/health`, `/metrics` and `/describe`.

mod sources;

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use axum::extract::{Query, State as AxState};
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
    #[arg(long, env = "ECON_REFRESH_HOURS", default_value_t = 6)]
    refresh_hours: u64,
}

/// A source's last good events, and how its last read went.
#[derive(Default)]
struct Source {
    events: Vec<Event>,
    error: Option<String>,
    read_at: f64,
}

type Sources = HashMap<&'static str, Source>;

#[derive(Default)]
struct Shared {
    sources: Mutex<Sources>,
}

impl Shared {
    fn lock(&self) -> std::sync::MutexGuard<'_, Sources> {
        self.sources.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

#[tokio::main]
async fn main() {
    let args = Args::parse();
    let _ = rustls::crypto::ring::default_provider().install_default();
    let shared = Arc::new(Shared::default());
    let observer = Observer::new("econ-calendar", env!("CARGO_PKG_VERSION"));
    observer.set_state(State::Starting, "reading the agencies' schedules");
    observer.describe(json!({
        "role": "Scheduled US economic releases (BLS, BEA, jobless claims) and FOMC decisions, from the agencies' own schedules.",
        "upstream": [
            { "id": "bls", "label": "BLS", "sub": "release schedule", "kind": "flow", "via": "https" },
            { "id": "bea", "label": "BEA", "sub": "release schedule", "kind": "flow", "via": "https" },
            { "id": "fed", "label": "Federal Reserve", "sub": "FOMC calendar", "kind": "flow", "via": "https" },
        ],
        "streams": { "econ": "/events" },
        "schedule": [{ "name": "Read the agencies' schedules", "kind": "interval", "every_seconds": args.refresh_hours * 3600, "when": format!("every {} h", args.refresh_hours) }],
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
    loop {
        read_all(&http, &shared, args.contact.is_some()).await;
        let (failing, held) = {
            let sources = shared.lock();
            let failing: Vec<String> = sources
                .iter()
                .filter_map(|(n, src)| src.error.as_ref().map(|e| format!("{n}: {e}")))
                .collect();
            (
                failing,
                sources.values().map(|src| src.events.len()).sum::<usize>(),
            )
        };
        if failing.is_empty() {
            observer.set_state(State::Ready, &format!("{held} events"));
        } else {
            observer.set_state(
                State::Degraded,
                &format!("{held} events; {}", failing.join("; ")),
            );
        }
        tokio::time::sleep(Duration::from_secs(args.refresh_hours.max(1) * 3600)).await;
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
}

#[derive(Deserialize)]
struct Ask {
    from: Option<String>,
    to: Option<String>,
    min: Option<String>,
}

async fn events(AxState(shared): AxState<Arc<Shared>>, Query(ask): Query<Ask>) -> Json<Value> {
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
            (date >= from && date <= to).then(|| json!({ "at": e.at, "date": date, "time": time, "title": e.title, "source": e.source, "importance": e.importance.as_str() }))
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
}

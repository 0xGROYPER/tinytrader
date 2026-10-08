//! The job queue (decision 0012): work that should run later, or when the
//! machine can spare it, from templates the deployment defines.
//!
//! `queue` serves the queue and runs its docker jobs as one-off containers;
//! `queue host-runner` runs its host jobs on the machine it runs on.
//!
//! Endpoints: `GET /jobs`, `GET /templates`, `POST /jobs`, `GET /jobs/{id}`,
//! `PATCH /jobs/{id}`, `POST /jobs/{id}/cancel`, `POST /jobs/{id}/run-now`,
//! `GET /jobs/{id}/log`; for the host runner `POST /claim`,
//! `POST /jobs/{id}/log`, `POST /jobs/{id}/finish`; and the
//! `tinytrader.observe/1` endpoints.

mod docker;
mod host;
mod job;
mod queue;
mod template;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::Router;
use axum::routing::{get, post};
use clap::{Parser, Subcommand};
use serde_json::json;
use tinytrader_observe::{Events, Observer, State, listen_address, lock};

#[derive(Parser, Debug)]
#[command(version, about)]
struct Args {
    #[command(subcommand)]
    mode: Option<Mode>,

    /// The job templates (YAML, Kubernetes-shaped; decision 0012).
    #[arg(long, env = "QUEUE_TEMPLATES")]
    templates: Option<PathBuf>,

    /// Where the journal and the jobs' logs are kept.
    #[arg(long, env = "QUEUE_STATE")]
    state: Option<PathBuf>,

    /// The Docker Engine API (a proxy allowing containers only), for docker
    /// jobs. Without it, docker jobs wait.
    #[arg(long, env = "QUEUE_DOCKER")]
    docker: Option<String>,

    /// The Docker network docker jobs join.
    #[arg(long, env = "QUEUE_NETWORK")]
    network: Option<String>,

    /// Secrets this service was given (`/run/secrets/<name>`) to pass to
    /// templates that mount them, separated by commas.
    #[arg(long, env = "QUEUE_SECRETS", value_delimiter = ',')]
    secrets: Vec<String>,

    /// Where to place those secrets (a folder of a Docker volume mounted
    /// here) ...
    #[arg(long, env = "QUEUE_SECRETS_DIR")]
    secrets_dir: Option<PathBuf>,

    /// ... and that folder as the containers that mount it see it:
    /// `VOLUME:SUBPATH`, e.g. `tinytrader-ops_queue-data:secrets`.
    #[arg(long, env = "QUEUE_SECRETS_VOLUME")]
    secrets_volume: Option<String>,

    /// Docker jobs running at once.
    #[arg(long, env = "QUEUE_MAX_RUNNING", default_value_t = 2)]
    max_running: usize,

    /// Capture services to yield to (decision 0011), as NAME=URL of each
    /// one's observe endpoint, separated by `;`.
    #[arg(long, env = "QUEUE_YIELD_TO", value_delimiter = ';')]
    yield_to: Vec<String>,

    /// Market calendar (`{ "days": [...] }`), for `after-close`; without it,
    /// weekdays close at 16:00 New York time.
    #[arg(long, env = "QUEUE_CALENDAR")]
    calendar: Option<String>,
}

#[derive(Subcommand, Debug)]
enum Mode {
    /// Serve the queue (the default).
    Serve,
    /// Run the queue's host jobs on this machine.
    HostRunner {
        /// The queue's address.
        #[arg(long, env = "QUEUE_URL", default_value = "http://127.0.0.1:9123")]
        queue: String,
        /// This runner's name in the queue (default: the host's name).
        #[arg(long, env = "QUEUE_RUNNER_NAME")]
        name: Option<String>,
    },
}

/// Copy the secrets this service was given into the volume jobs mount, each
/// as a file readable by the job's user, which isn't this service's.
fn place_secrets(names: &[String], dir: &std::path::Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt as _;
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    for name in names.iter().filter(|n| !n.is_empty()) {
        let from = PathBuf::from("/run/secrets").join(name);
        let value = std::fs::read(&from).map_err(|e| format!("secret {name}: {e}"))?;
        let to = dir.join(name);
        std::fs::write(&to, value).map_err(|e| format!("secret {name}: {e}"))?;
        std::fs::set_permissions(&to, std::fs::Permissions::from_mode(0o444))
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

fn fail(msg: String) -> ! {
    eprintln!("queue: {msg}");
    std::process::exit(2);
}

#[tokio::main]
async fn main() {
    let args = Args::parse();
    let _ = rustls::crypto::ring::default_provider().install_default();
    if let Some(Mode::HostRunner { queue, name }) = &args.mode {
        let name = name.clone().unwrap_or_else(|| {
            std::process::Command::new("hostname")
                .output()
                .ok()
                .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned())
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| "host".into())
        });
        host::run(queue, &name).await;
    }
    let path = args
        .templates
        .clone()
        .unwrap_or_else(|| fail("set QUEUE_TEMPLATES".into()));
    let text =
        std::fs::read_to_string(&path).unwrap_or_else(|e| fail(format!("{}: {e}", path.display())));
    let templates = template::load(&text).unwrap_or_else(|e| fail(format!("templates: {e}")));
    let state = args
        .state
        .clone()
        .unwrap_or_else(|| fail("set QUEUE_STATE".into()));
    let (journal, jobs) = job::Journal::open(&state).unwrap_or_else(|e| fail(e));
    if let Some(dir) = &args.secrets_dir {
        place_secrets(&args.secrets, dir).unwrap_or_else(|e| fail(e));
    }
    let q = Arc::new(queue::Queue {
        templates,
        jobs: Mutex::new(jobs),
        journal,
        docker: args
            .docker
            .as_deref()
            .map(|d| docker::Docker::new(d, args.network.clone(), args.secrets_volume.clone())),
        max_running: args.max_running.max(1),
        yield_to: args
            .yield_to
            .iter()
            .filter_map(|y| {
                y.split_once('=').map(|(n, u)| {
                    (
                        n.trim().to_owned(),
                        u.trim().trim_end_matches('/').to_owned(),
                    )
                })
            })
            .collect(),
        calendar_url: args.calendar.clone(),
        calendar: Mutex::default(),
        http: reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(5))
            .build()
            .expect("an HTTP client"),
        events: Events::default(),
        host_seen: Mutex::new(HashMap::new()),
        wake: tokio::sync::Notify::new(),
    });

    let observer = Observer::new("queue", env!("CARGO_PKG_VERSION"));
    observer.set_state(State::Starting, "reading the journal");
    let names: Vec<&str> = q.yield_to.iter().map(|(n, _)| n.as_str()).collect();
    observer.describe(json!({
        "role": "The job queue: work from the deployment's templates, started when its conditions hold, as one-off containers or on the host.",
        "upstream": [],
        "streams": { "queue": "/jobs", "templates": "/templates", "events": "/events" },
        "schedule": [
            { "name": "Queued jobs", "kind": "event", "class": "background", "when": "when each job's conditions hold" },
            { "name": "Yields to live capture", "kind": "yield", "class": "background", "yields_to": names, "when": "jobs marked background-may-run wait while capture is live" },
        ],
    }));
    {
        let q = q.clone();
        observer.set_metrics(move |w| {
            let jobs = lock(&q.jobs);
            for s in [
                job::State::Waiting,
                job::State::Pending,
                job::State::Running,
                job::State::Succeeded,
                job::State::Failed,
                job::State::Cancelled,
            ] {
                let n = jobs.values().filter(|j| j.state == s).count();
                w.gauge(
                    "tinytrader_queue_jobs",
                    "Jobs by state.",
                    &[("state", s.as_str())],
                    n as f64,
                );
            }
        });
    }

    let app = Router::new()
        .route("/jobs", get(queue::list).post(queue::add))
        .route("/templates", get(queue::templates))
        .route("/jobs/{id}", get(queue::get_job).patch(queue::edit))
        .route("/jobs/{id}/cancel", post(queue::cancel))
        .route("/jobs/{id}/run-now", post(queue::run_now))
        .route("/jobs/{id}/log", get(queue::log).post(queue::append_log))
        .route("/jobs/{id}/finish", post(queue::finish))
        .route("/claim", post(queue::claim))
        .layer(axum::middleware::from_fn(
            tinytrader_observe::guard::same_origin_writes,
        ))
        .with_state(q.clone())
        .merge(observer.router())
        .merge(q.events.router());
    let listen = listen_address("QUEUE_LISTEN", "127.0.0.1:9123");
    let listener = tokio::net::TcpListener::bind(&listen)
        .await
        .unwrap_or_else(|e| fail(format!("cannot listen on {listen}: {e}")));
    eprintln!(
        "queue: {} templates; serving http://{listen}",
        q.templates.len()
    );
    tokio::spawn(async move {
        if let Err(e) = axum::serve(listener, app).await {
            eprintln!("queue: http server stopped: {e}");
        }
    });

    queue::recover(&q).await;
    let mut calendar_at = None;
    loop {
        queue::refresh_calendar(&q, &mut calendar_at).await;
        queue::tick(&q).await;
        queue::report(&q, &observer);
        tokio::select! {
            () = q.wake.notified() => {}
            () = tokio::time::sleep(Duration::from_secs(5)) => {}
        }
    }
}

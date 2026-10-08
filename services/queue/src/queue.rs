//! The queue: jobs, the conditions that hold them back, the scheduler that
//! starts them on their runner, and the HTTP API the console, `tt` and the
//! host runner use.

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::Json;
use axum::extract::{Path, Query, State as AxState};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use serde_json::{Value, json};
use tinytrader_observe::{Events, Observer, State as Health, lock, unix_seconds};
use tinytrader_schedule::{Calendar, ny_date_time};

use crate::docker::{Docker, Status};
use crate::job::{Job, Journal, State, When, next_id};
use crate::template::{self, Runner, Template};

/// A host runner not heard from for this long has lost its job.
const HOST_SILENCE: Duration = Duration::from_secs(300);
/// Finished jobs listed (the journal keeps all).
const LISTED_FINISHED: usize = 100;

pub struct Queue {
    pub templates: Vec<Template>,
    pub jobs: Mutex<BTreeMap<String, Job>>,
    pub journal: Journal,
    pub docker: Option<Docker>,
    /// At most this many docker jobs run at once.
    pub max_running: usize,
    /// Capture services to yield to (decision 0011), as (name, observe URL).
    pub yield_to: Vec<(String, String)>,
    pub calendar_url: Option<String>,
    pub calendar: Mutex<Calendar>,
    pub http: reqwest::Client,
    pub events: Events,
    /// When each host runner was last heard from, by job.
    pub host_seen: Mutex<HashMap<String, Instant>>,
    pub wake: tokio::sync::Notify,
}

pub type Shared = Arc<Queue>;

impl Queue {
    fn template(&self, name: &str) -> Option<&Template> {
        self.templates.iter().find(|t| t.metadata.name == name)
    }

    /// Record a job's new state (journal first), and keep it.
    fn save(&self, job: Job) -> Result<Job, String> {
        self.journal.record(&job)?;
        let summary = format!(
            "{} {}{}",
            job.id,
            job.state.as_str(),
            if job.reason.is_empty() {
                String::new()
            } else {
                format!(": {}", job.reason)
            }
        );
        self.events.push(summary.clone(), summary);
        lock(&self.jobs).insert(job.id.clone(), job.clone());
        Ok(job)
    }

    fn update(&self, id: &str, f: impl FnOnce(&mut Job)) -> Option<Job> {
        let mut j = lock(&self.jobs).get(id)?.clone();
        f(&mut j);
        match self.save(j) {
            Ok(j) => Some(j),
            Err(e) => {
                eprintln!("queue: cannot record {id}: {e}");
                None
            }
        }
    }
}

// ---------- conditions ----------

/// Whether today's session has closed (or there is none today).
fn after_close(now: f64, cal: &Calendar) -> bool {
    let (date, hhmm, weekday) = ny_date_time(now as i64);
    if let Some(d) = cal.days.iter().find(|d| d.date == date) {
        return hhmm >= d.close;
    }
    let covered = matches!((cal.days.first(), cal.days.last()), (Some(a), Some(b)) if a.date <= date && date <= b.date);
    // Inside the calendar's range, a missing day is a holiday; outside it,
    // weekdays close at 16:00.
    covered || !(1..=5).contains(&weekday) || hhmm.as_str() >= "16:00"
}

/// The first capture service that is live (its health is anything but
/// idle); one that doesn't answer isn't running, so isn't capturing.
async fn live_capture(q: &Queue) -> Option<String> {
    for (name, url) in &q.yield_to {
        let Ok(r) = q
            .http
            .get(format!("{url}/health"))
            .timeout(Duration::from_secs(3))
            .send()
            .await
        else {
            continue;
        };
        let state = r
            .json::<Value>()
            .await
            .ok()
            .and_then(|v| v["state"].as_str().map(str::to_owned));
        if state.as_deref() != Some("idle") {
            return Some(name.clone());
        }
    }
    None
}

/// What a waiting job still waits for, or `None` when it may start.
fn waits_for(
    job: &Job,
    now: f64,
    cal: &Calendar,
    capture: Option<&str>,
    jobs: &BTreeMap<String, Job>,
) -> Option<String> {
    for dep in &job.after {
        match jobs.get(dep).map(|d| d.state) {
            Some(State::Succeeded) => {}
            Some(s) if s.finished() => {
                return Some(format!("!{dep} {}", s.as_str().to_lowercase()));
            }
            Some(_) => return Some(format!("after {dep}")),
            None => return Some(format!("!{dep} doesn't exist")),
        }
    }
    if let Some(t) = job.not_before
        && now < t
    {
        let (d, hm, _) = ny_date_time(t as i64);
        return Some(format!("not before {d} {hm} ET"));
    }
    match job.when {
        When::Anytime => None,
        When::AfterClose => (!after_close(now, cal)).then(|| "after the close".to_owned()),
        When::BackgroundMayRun => capture.map(|c| format!("yielding: {c} is live")),
    }
}

// ---------- the scheduler ----------

/// Move every job along: start what may start, follow what runs, finish what
/// has ended. Runs every few seconds and whenever something changes.
pub async fn tick(q: &Queue) {
    let now = unix_seconds();
    let jobs = lock(&q.jobs).clone();
    let wants_capture = jobs
        .values()
        .any(|j| j.state == State::Waiting && j.when == When::BackgroundMayRun);
    let capture = if wants_capture {
        live_capture(q).await
    } else {
        None
    };
    let cal = lock(&q.calendar).clone();

    for job in jobs.values().filter(|j| j.state == State::Waiting) {
        match waits_for(job, now, &cal, capture.as_deref(), &jobs) {
            Some(why) if why.starts_with('!') => {
                let why = why[1..].to_owned();
                q.update(&job.id, |j| {
                    j.state = State::Cancelled;
                    j.reason = format!("after {why}");
                    j.finished = Some(now);
                });
            }
            Some(why) => {
                if why != job.reason {
                    // Waiting reasons change often; keep them in memory only.
                    if let Some(j) = lock(&q.jobs).get_mut(&job.id) {
                        j.reason = why;
                    }
                }
            }
            None => {
                q.update(&job.id, |j| {
                    j.state = State::Pending;
                    j.reason = match j.runner {
                        Runner::Docker => "starting".into(),
                        Runner::Host => "waiting for the host runner".into(),
                    };
                });
            }
        }
    }

    let jobs = lock(&q.jobs).clone();
    if let Some(docker) = &q.docker {
        docker_jobs(q, docker, &jobs, now).await;
    }
    for job in jobs
        .values()
        .filter(|j| j.runner == Runner::Host && j.state == State::Running)
    {
        let silent = lock(&q.host_seen)
            .get(&job.id)
            .is_none_or(|t| t.elapsed() > HOST_SILENCE);
        if silent {
            q.update(&job.id, |j| {
                j.state = State::Failed;
                j.reason = "the host runner stopped reporting".into();
                j.finished = Some(now);
            });
        } else if past_deadline(q, job, now) && !job.cancel_requested {
            q.update(&job.id, |j| {
                j.cancel_requested = true;
                j.reason = "past its deadline".into();
            });
        }
    }
}

fn past_deadline(q: &Queue, job: &Job, now: f64) -> bool {
    let limit = q
        .template(&job.template)
        .and_then(|t| t.spec.active_deadline_seconds);
    matches!((limit, job.started), (Some(l), Some(s)) if now - s > l as f64)
}

async fn docker_jobs(q: &Queue, docker: &Docker, jobs: &BTreeMap<String, Job>, now: f64) {
    let mut running = jobs
        .values()
        .filter(|j| j.runner == Runner::Docker && j.state == State::Running)
        .count();
    // Follow running containers.
    for job in jobs
        .values()
        .filter(|j| j.runner == Runner::Docker && j.state == State::Running)
    {
        let status = match docker.status(job).await {
            Ok(s) => s,
            Err(e) => {
                q.events.push(format!("{}: {e}", job.id), e);
                continue;
            }
        };
        match status {
            Status::Running if job.cancel_requested || past_deadline(q, job, now) => {
                // Stopped now; the next look sees it exited and finishes it.
                docker.stop(job).await;
                if !job.cancel_requested {
                    q.update(&job.id, |j| {
                        j.cancel_requested = true;
                        j.reason = "past its deadline".into();
                    });
                }
            }
            Status::Running => {}
            Status::Exited(code) => {
                if let Ok(log) = docker.logs(job, None).await {
                    q.journal.write_log(&job.id, &log);
                }
                docker.remove(job).await;
                running = running.saturating_sub(1);
                let deadline = job.reason == "past its deadline";
                q.update(&job.id, |j| {
                    j.exit_code = Some(code);
                    j.finished = Some(unix_seconds());
                    (j.state, j.reason) = if deadline {
                        (State::Failed, "stopped: past its deadline".into())
                    } else if j.cancel_requested {
                        (State::Cancelled, "stopped on request".into())
                    } else if code == 0 {
                        (State::Succeeded, String::new())
                    } else {
                        (State::Failed, format!("exit code {code}"))
                    };
                });
            }
            Status::Gone => {
                running = running.saturating_sub(1);
                q.update(&job.id, |j| {
                    j.state = State::Failed;
                    j.reason = "its container disappeared".into();
                    j.finished = Some(unix_seconds());
                });
            }
        }
    }
    // Start pending jobs while there's room; recorded as running before the
    // container is created, so a crash can never start one twice.
    let mut pending: Vec<&Job> = jobs
        .values()
        .filter(|j| j.runner == Runner::Docker && j.state == State::Pending)
        .collect();
    pending.sort_by(|a, b| a.created.total_cmp(&b.created));
    for job in pending {
        if running >= q.max_running {
            if job.reason != "waiting for a free slot"
                && let Some(j) = lock(&q.jobs).get_mut(&job.id)
            {
                j.reason = "waiting for a free slot".into();
            }
            continue;
        }
        let Some(t) = q.template(&job.template) else {
            continue;
        };
        let Some(job) = q.update(&job.id, |j| {
            j.state = State::Running;
            j.started = Some(unix_seconds());
            j.worker = Some(Docker::name(j));
            j.reason = String::new();
        }) else {
            continue;
        };
        running += 1;
        if let Err(e) = docker.start(t, &job).await {
            docker.remove(&job).await;
            running -= 1;
            q.update(&job.id, |j| {
                j.state = State::Failed;
                j.reason = e;
                j.finished = Some(unix_seconds());
            });
        }
    }
}

/// After a restart: a docker job recorded as running whose container is gone
/// failed while the queue was down; it is never started again.
pub async fn recover(q: &Queue) {
    let Some(docker) = &q.docker else { return };
    let jobs = lock(&q.jobs).clone();
    for job in jobs
        .values()
        .filter(|j| j.runner == Runner::Docker && j.state == State::Running)
    {
        if let Ok(Status::Gone) = docker.status(job).await {
            q.update(&job.id, |j| {
                j.state = State::Failed;
                j.reason = "lost: its container was gone after the queue restarted".into();
                j.finished = Some(unix_seconds());
            });
        }
    }
    // Host jobs get the usual silence allowance from now.
    let mut seen = lock(&q.host_seen);
    for job in jobs
        .values()
        .filter(|j| j.runner == Runner::Host && j.state == State::Running)
    {
        seen.insert(job.id.clone(), Instant::now());
    }
}

/// Fetch the market calendar again, at most hourly.
pub async fn refresh_calendar(q: &Queue, last: &mut Option<Instant>) {
    let Some(url) = &q.calendar_url else { return };
    if last.is_some_and(|t| t.elapsed() < Duration::from_secs(3600)) {
        return;
    }
    if let Ok(r) = q.http.get(url).send().await
        && let Ok(c) = r.json::<Calendar>().await
        && !c.days.is_empty()
    {
        *lock(&q.calendar) = c;
        *last = Some(Instant::now());
    }
}

/// The service's health: what waits and what runs.
pub fn report(q: &Queue, observer: &Observer) {
    let jobs = lock(&q.jobs);
    let count = |s: State| jobs.values().filter(|j| j.state == s).count();
    let detail = format!(
        "{} waiting, {} pending, {} running",
        count(State::Waiting),
        count(State::Pending),
        count(State::Running)
    );
    observer.set_state(Health::Ready, &detail);
}

// ---------- the API ----------

fn err(code: StatusCode, msg: impl Into<String>) -> Response {
    (code, Json(json!({ "error": msg.into() }))).into_response()
}

pub async fn list(AxState(q): AxState<Shared>) -> Json<Value> {
    let jobs = lock(&q.jobs);
    let mut open: Vec<&Job> = jobs.values().filter(|j| !j.state.finished()).collect();
    open.sort_by(|a, b| a.created.total_cmp(&b.created));
    let mut done: Vec<&Job> = jobs.values().filter(|j| j.state.finished()).collect();
    done.sort_by(|a, b| {
        b.finished
            .unwrap_or(0.0)
            .total_cmp(&a.finished.unwrap_or(0.0))
    });
    done.truncate(LISTED_FINISHED);
    Json(
        json!({ "jobs": open.into_iter().chain(done).collect::<Vec<_>>(), "updated_at": unix_seconds() }),
    )
}

pub async fn templates(AxState(q): AxState<Shared>) -> Json<Value> {
    let list: Vec<Value> = q
        .templates
        .iter()
        .map(|t| {
            let c = &t.spec.template.spec.containers[0];
            json!({
                "name": t.metadata.name,
                "description": t.metadata.description,
                "runner": t.spec.runner,
                "class": t.spec.class,
                "parameters": t.spec.parameters,
                "command": c.command.iter().chain(&c.args).cloned().collect::<Vec<_>>(),
                "image": (t.spec.runner == Runner::Docker).then(|| c.image.clone()),
                "workingDir": c.working_dir,
                "activeDeadlineSeconds": t.spec.active_deadline_seconds,
            })
        })
        .collect();
    Json(json!({ "templates": list }))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Add {
    template: String,
    #[serde(default)]
    params: BTreeMap<String, String>,
    #[serde(default)]
    not_before: Option<f64>,
    #[serde(default)]
    when: When,
    #[serde(default)]
    after: Vec<String>,
    #[serde(default)]
    added_by: Option<String>,
}

pub async fn add(AxState(q): AxState<Shared>, Json(a): Json<Add>) -> Response {
    let Some(t) = q.template(&a.template) else {
        return err(StatusCode::NOT_FOUND, format!("no template {}", a.template));
    };
    let (command, args) = match template::resolve(t, &a.params) {
        Ok(c) => c,
        Err(e) => return err(StatusCode::UNPROCESSABLE_ENTITY, e),
    };
    let id = {
        let jobs = lock(&q.jobs);
        if let Some(missing) = a.after.iter().find(|d| !jobs.contains_key(*d)) {
            return err(
                StatusCode::UNPROCESSABLE_ENTITY,
                format!("no job {missing}"),
            );
        }
        next_id(&a.template, &jobs)
    };
    let job = Job {
        id,
        template: a.template,
        runner: t.spec.runner,
        params: a.params,
        command,
        args,
        not_before: a.not_before,
        when: a.when,
        after: a.after,
        state: State::Waiting,
        reason: String::new(),
        created: unix_seconds(),
        started: None,
        finished: None,
        exit_code: None,
        worker: None,
        cancel_requested: false,
        added_by: a.added_by.unwrap_or_default().chars().take(40).collect(),
    };
    match q.save(job) {
        Ok(j) => {
            q.wake.notify_one();
            (StatusCode::CREATED, Json(json!(j))).into_response()
        }
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e),
    }
}

pub async fn get_job(AxState(q): AxState<Shared>, Path(id): Path<String>) -> Response {
    // A host runner asking about its job is alive.
    if let Some(t) = lock(&q.host_seen).get_mut(&id) {
        *t = Instant::now();
    }
    match lock(&q.jobs).get(&id) {
        Some(j) => Json(json!(j)).into_response(),
        None => err(StatusCode::NOT_FOUND, format!("no job {id}")),
    }
}

pub async fn cancel(AxState(q): AxState<Shared>, Path(id): Path<String>) -> Response {
    let Some(state) = lock(&q.jobs).get(&id).map(|j| j.state) else {
        return err(StatusCode::NOT_FOUND, format!("no job {id}"));
    };
    let job = match state {
        s if s.finished() => {
            return err(StatusCode::CONFLICT, format!("{id} has already finished"));
        }
        State::Running => q.update(&id, |j| {
            j.cancel_requested = true;
            j.reason = "stopping on request".into();
        }),
        _ => q.update(&id, |j| {
            j.state = State::Cancelled;
            j.reason = "cancelled before it started".into();
            j.finished = Some(unix_seconds());
        }),
    };
    q.wake.notify_one();
    Json(json!(job)).into_response()
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Edit {
    /// A time, or `null` to clear it.
    #[serde(default, deserialize_with = "serde_with_null::deserialize")]
    not_before: Option<Option<f64>>,
    #[serde(default)]
    when: Option<When>,
    #[serde(default)]
    after: Option<Vec<String>>,
}

/// `Option<Option<T>>` from JSON: absent is `None`, `null` is `Some(None)`.
mod serde_with_null {
    use serde::{Deserialize, Deserializer};
    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Option<f64>>, D::Error> {
        Option::<f64>::deserialize(d).map(Some)
    }
}

pub async fn edit(
    AxState(q): AxState<Shared>,
    Path(id): Path<String>,
    Json(e): Json<Edit>,
) -> Response {
    let Some(at) = lock(&q.jobs).get(&id).map(|j| (j.runner, j.state)) else {
        return err(StatusCode::NOT_FOUND, format!("no job {id}"));
    };
    // A waiting job, or a host job no runner has claimed yet.
    if !matches!(at, (_, State::Waiting) | (Runner::Host, State::Pending)) {
        return err(
            StatusCode::CONFLICT,
            format!("{id} has started; only a waiting job can be changed"),
        );
    }
    if let Some(after) = &e.after {
        let jobs = lock(&q.jobs);
        if let Some(missing) = after.iter().find(|d| !jobs.contains_key(*d) || **d == id) {
            return err(
                StatusCode::UNPROCESSABLE_ENTITY,
                format!("can't wait for {missing}"),
            );
        }
    }
    let job = q.update(&id, |j| {
        if let Some(nb) = e.not_before {
            j.not_before = nb;
        }
        if let Some(w) = e.when {
            j.when = w;
        }
        if let Some(a) = e.after {
            j.after = a;
        }
        j.state = State::Waiting;
        j.reason = String::new();
    });
    q.wake.notify_one();
    Json(json!(job)).into_response()
}

pub async fn run_now(AxState(q): AxState<Shared>, Path(id): Path<String>) -> Response {
    let Some(state) = lock(&q.jobs).get(&id).map(|j| j.state) else {
        return err(StatusCode::NOT_FOUND, format!("no job {id}"));
    };
    if state != State::Waiting {
        return err(StatusCode::CONFLICT, format!("{id} isn't waiting"));
    }
    let job = q.update(&id, |j| {
        j.not_before = None;
        j.when = When::Anytime;
        j.after.clear();
        j.reason = "run now".into();
    });
    q.wake.notify_one();
    Json(json!(job)).into_response()
}

#[derive(Deserialize)]
pub struct LogAsk {
    #[serde(default)]
    tail: Option<u32>,
}

pub async fn log(
    AxState(q): AxState<Shared>,
    Path(id): Path<String>,
    Query(a): Query<LogAsk>,
) -> Response {
    let Some(job) = lock(&q.jobs).get(&id).cloned() else {
        return err(StatusCode::NOT_FOUND, format!("no job {id}"));
    };
    let text = match (&q.docker, job.runner, job.state) {
        (Some(d), Runner::Docker, State::Running) => d
            .logs(&job, Some(a.tail.unwrap_or(500)))
            .await
            .map(|b| String::from_utf8_lossy(&b).into_owned())
            .unwrap_or_else(|e| e),
        _ => q.journal.read_log(&id, 256 * 1024),
    };
    Json(json!({ "id": id, "state": job.state, "log": text })).into_response()
}

// ---------- the host runner's side ----------

#[derive(Deserialize)]
pub struct Claim {
    runner: String,
}

/// Hand the oldest pending host job to a host runner.
pub async fn claim(AxState(q): AxState<Shared>, Json(c): Json<Claim>) -> Response {
    let next = {
        let jobs = lock(&q.jobs);
        let mut pending: Vec<&Job> = jobs
            .values()
            .filter(|j| j.runner == Runner::Host && j.state == State::Pending)
            .collect();
        pending.sort_by(|a, b| a.created.total_cmp(&b.created));
        pending.first().map(|j| j.id.clone())
    };
    let Some(id) = next else {
        return StatusCode::NO_CONTENT.into_response();
    };
    let Some(job) = q.update(&id, |j| {
        j.state = State::Running;
        j.started = Some(unix_seconds());
        j.worker = Some(c.runner.chars().take(60).collect());
        j.reason = String::new();
    }) else {
        return StatusCode::NO_CONTENT.into_response();
    };
    lock(&q.host_seen).insert(id, Instant::now());
    let t = q.template(&job.template).expect("a job's template exists");
    let c = &t.spec.template.spec.containers[0];
    Json(json!({
        "job": job,
        "workingDir": c.working_dir,
        "env": c.env,
        "activeDeadlineSeconds": t.spec.active_deadline_seconds,
    }))
    .into_response()
}

pub async fn append_log(
    AxState(q): AxState<Shared>,
    Path(id): Path<String>,
    body: axum::body::Bytes,
) -> Response {
    if !lock(&q.jobs)
        .get(&id)
        .is_some_and(|j| j.runner == Runner::Host && j.state == State::Running)
    {
        return err(
            StatusCode::CONFLICT,
            format!("{id} isn't a running host job"),
        );
    }
    if let Some(t) = lock(&q.host_seen).get_mut(&id) {
        *t = Instant::now();
    }
    q.journal.append_log(&id, &body);
    StatusCode::NO_CONTENT.into_response()
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Finish {
    exit_code: i64,
}

pub async fn finish(
    AxState(q): AxState<Shared>,
    Path(id): Path<String>,
    Json(f): Json<Finish>,
) -> Response {
    if !lock(&q.jobs)
        .get(&id)
        .is_some_and(|j| j.runner == Runner::Host && j.state == State::Running)
    {
        return err(
            StatusCode::CONFLICT,
            format!("{id} isn't a running host job"),
        );
    }
    lock(&q.host_seen).remove(&id);
    let job = q.update(&id, |j| {
        j.exit_code = Some(f.exit_code);
        j.finished = Some(unix_seconds());
        (j.state, j.reason) = if j.reason == "past its deadline" {
            (State::Failed, "stopped: past its deadline".into())
        } else if j.cancel_requested {
            (State::Cancelled, "stopped on request".into())
        } else if f.exit_code == 0 {
            (State::Succeeded, String::new())
        } else {
            (State::Failed, format!("exit code {}", f.exit_code))
        };
    });
    q.wake.notify_one();
    Json(json!(job)).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tinytrader_schedule::Day;

    fn cal() -> Calendar {
        Calendar {
            days: vec![
                Day {
                    date: "2026-10-08".into(),
                    open: "09:30".into(),
                    close: "16:00".into(),
                },
                Day {
                    date: "2026-11-27".into(),
                    open: "09:30".into(),
                    close: "13:00".into(),
                },
            ],
        }
    }

    #[test]
    fn after_the_close() {
        let at = |y, m, d, h, mi| tinytrader_schedule::ny_to_unix(y, m, d, h, mi) as f64;
        assert!(!after_close(at(2026, 10, 8, 15, 59), &cal()));
        assert!(after_close(at(2026, 10, 8, 16, 0), &cal()));
        assert!(after_close(at(2026, 11, 27, 13, 5), &cal()), "early close");
        assert!(after_close(at(2026, 10, 10, 11, 0), &cal()), "a Saturday");
        assert!(
            after_close(at(2026, 10, 9, 11, 0), &cal()),
            "a day the calendar's range leaves out is a holiday"
        );
        assert!(
            !after_close(at(2026, 12, 1, 11, 0), &cal()),
            "past the calendar: a weekday's session"
        );
    }
}

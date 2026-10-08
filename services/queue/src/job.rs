//! Jobs and their journal. A job is a template plus parameter values and
//! when it may start. Every change appends the job's whole record to an
//! append-only JSON-lines file, flushed before anything acts on it; on start
//! the last record of each job is its state. Nothing is ever started twice:
//! a job recorded as started is never started again, only looked up.

use std::collections::BTreeMap;
use std::io::Write as _;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Kubernetes' job phases, plus `Waiting` (its conditions don't hold yet).
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub enum State {
    Waiting,
    Pending,
    Running,
    Succeeded,
    Failed,
    Cancelled,
}

impl State {
    #[must_use]
    pub fn finished(self) -> bool {
        matches!(self, State::Succeeded | State::Failed | State::Cancelled)
    }
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            State::Waiting => "Waiting",
            State::Pending => "Pending",
            State::Running => "Running",
            State::Succeeded => "Succeeded",
            State::Failed => "Failed",
            State::Cancelled => "Cancelled",
        }
    }
}

/// When a job may start, besides `notBefore` and `after`.
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum When {
    #[default]
    Anytime,
    /// After the market's close on a trading day; any time on other days.
    AfterClose,
    /// Decision 0011: while no capture service is live.
    BackgroundMayRun,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Job {
    pub id: String,
    pub template: String,
    pub runner: crate::template::Runner,
    pub params: BTreeMap<String, String>,
    /// The resolved command and arguments.
    pub command: Vec<String>,
    pub args: Vec<String>,
    /// Unix seconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub not_before: Option<f64>,
    #[serde(default)]
    pub when: When,
    #[serde(default)]
    pub after: Vec<String>,
    pub state: State,
    /// Why it is in its state (what it waits for, why it failed).
    #[serde(default)]
    pub reason: String,
    pub created: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finished: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i64>,
    /// The docker runner's container, or the host runner that claimed it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worker: Option<String>,
    /// Set when cancel was asked while it ran; the runner stops it.
    #[serde(default)]
    pub cancel_requested: bool,
    /// Who added it (`console`, `tt`, ...), as the client said.
    #[serde(default)]
    pub added_by: String,
}

pub struct Journal {
    path: PathBuf,
    logs: PathBuf,
}

impl Journal {
    /// Open (creating) the journal under `dir`, and the jobs it holds.
    ///
    /// # Errors
    /// The directory can't be created or the journal can't be read. A torn
    /// last line (a crash mid-write) is skipped.
    pub fn open(dir: &Path) -> Result<(Self, BTreeMap<String, Job>), String> {
        let logs = dir.join("logs");
        std::fs::create_dir_all(&logs).map_err(|e| format!("{}: {e}", logs.display()))?;
        let path = dir.join("jobs.jsonl");
        let mut jobs = BTreeMap::new();
        if let Ok(text) = std::fs::read_to_string(&path) {
            for line in text.lines().filter(|l| !l.trim().is_empty()) {
                if let Ok(j) = serde_json::from_str::<Job>(line) {
                    jobs.insert(j.id.clone(), j);
                }
            }
            // A torn last line: end it, so the next record starts clean.
            if !text.is_empty() && !text.ends_with('\n') {
                let mut f = std::fs::OpenOptions::new()
                    .append(true)
                    .open(&path)
                    .map_err(|e| e.to_string())?;
                f.write_all(b"\n").map_err(|e| e.to_string())?;
            }
        }
        Ok((Self { path, logs }, jobs))
    }

    /// Append a job's record, flushed to disk before returning.
    ///
    /// # Errors
    /// The write fails.
    pub fn record(&self, job: &Job) -> Result<(), String> {
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
            .map_err(|e| e.to_string())?;
        let line = serde_json::to_string(job).map_err(|e| e.to_string())? + "\n";
        f.write_all(line.as_bytes()).map_err(|e| e.to_string())?;
        f.sync_data().map_err(|e| e.to_string())
    }

    #[must_use]
    pub fn log_path(&self, id: &str) -> PathBuf {
        self.logs.join(format!("{id}.log"))
    }

    /// Append to a job's log.
    pub fn append_log(&self, id: &str, text: &[u8]) {
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.log_path(id))
        {
            let _ = f.write_all(text);
        }
    }

    /// Replace a job's log (the docker runner keeps the container's whole log).
    pub fn write_log(&self, id: &str, text: &[u8]) {
        let _ = std::fs::write(self.log_path(id), text);
    }

    /// The last `max` bytes of a job's log.
    #[must_use]
    pub fn read_log(&self, id: &str, max: usize) -> String {
        let bytes = std::fs::read(self.log_path(id)).unwrap_or_default();
        let from = bytes.len().saturating_sub(max);
        String::from_utf8_lossy(&bytes[from..]).into_owned()
    }
}

/// A new job id: the template's name and a number, e.g. `research-collector-7`.
#[must_use]
pub fn next_id(template: &str, jobs: &BTreeMap<String, Job>) -> String {
    let n = jobs
        .keys()
        .filter_map(|k| {
            k.strip_prefix(template)?
                .strip_prefix('-')?
                .parse::<u64>()
                .ok()
        })
        .max()
        .unwrap_or(0);
    format!("{template}-{}", n + 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn job(id: &str, state: State) -> Job {
        Job {
            id: id.into(),
            template: "t".into(),
            runner: crate::template::Runner::Docker,
            params: BTreeMap::new(),
            command: vec!["true".into()],
            args: vec![],
            not_before: None,
            when: When::Anytime,
            after: vec![],
            state,
            reason: String::new(),
            created: 1.0,
            started: None,
            finished: None,
            exit_code: None,
            worker: None,
            cancel_requested: false,
            added_by: "test".into(),
        }
    }

    #[test]
    fn the_last_record_of_each_job_wins_and_a_torn_line_is_skipped() {
        let dir =
            std::env::temp_dir().join(format!("tinytrader-queue-journal-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let (j, jobs) = Journal::open(&dir).unwrap();
        assert!(jobs.is_empty());
        j.record(&job("t-1", State::Waiting)).unwrap();
        j.record(&job("t-2", State::Waiting)).unwrap();
        j.record(&job("t-1", State::Running)).unwrap();
        let mut f = std::fs::OpenOptions::new()
            .append(true)
            .open(dir.join("jobs.jsonl"))
            .unwrap();
        f.write_all(b"{\"id\":\"t-2\",\"sta").unwrap();
        let (j, jobs) = Journal::open(&dir).unwrap();
        assert_eq!(jobs["t-1"].state, State::Running);
        assert_eq!(jobs["t-2"].state, State::Waiting);
        assert_eq!(next_id("t", &jobs), "t-3");
        // A record after the torn line still reads.
        j.record(&job("t-2", State::Cancelled)).unwrap();
        let (_, jobs) = Journal::open(&dir).unwrap();
        assert_eq!(jobs["t-2"].state, State::Cancelled);
        let _ = std::fs::remove_dir_all(&dir);
    }
}

//! The submissions ledger: every submission to Databento is written down,
//! and flushed to disk, before it is sent, so a purchase is never forgotten.
//!
//! One JSON line per step, appended, never rewritten:
//!
//! - `sending`: about to submit this exact request (written first);
//! - `submitted`: Databento answered with this job ID;
//! - `uncertain`: the submission failed or its outcome is unknown;
//! - `not-submitted`: reconciliation found no job for an uncertain one.
//!
//! A request whose latest step is `sending` or `uncertain` may already have
//! been bought, so it can't be submitted again until reconciled against
//! Databento's job list: matched to the job it created, or, after
//! [`SETTLE_SECONDS`] with no such job, recorded as not submitted.

use std::collections::HashSet;
use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// How long a submission may take to appear in Databento's job list before
/// an uncertain one is taken as never created.
pub const SETTLE_SECONDS: f64 = 600.0;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Entry {
    /// Unix seconds.
    pub at: f64,
    /// The request's fingerprint (see [`fingerprint`]).
    pub request: String,
    pub state: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub job: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_usd: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// What identifies a request across a submission and Databento's job list:
/// dataset, schema, symbols (sorted) and the days it covers.
#[must_use]
pub fn fingerprint(
    dataset: &str,
    schema: &str,
    symbols: &[String],
    start: &str,
    end: &str,
) -> String {
    let mut s = symbols.to_vec();
    s.sort();
    format!("{dataset}|{schema}|{}|{start}|{end}", s.join(","))
}

pub struct Ledger {
    path: PathBuf,
    entries: Vec<Entry>,
}

impl Ledger {
    /// Open (or start) the ledger at `path`.
    ///
    /// # Errors
    /// If the file exists but can't be read, or a line isn't an entry.
    pub fn open(path: &Path) -> Result<Self, String> {
        let entries = match fs::read_to_string(path) {
            Ok(text) => text
                .lines()
                .filter(|l| !l.trim().is_empty())
                .map(|l| serde_json::from_str(l).map_err(|e| format!("{}: {e}", path.display())))
                .collect::<Result<_, _>>()?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(e) => return Err(format!("{}: {e}", path.display())),
        };
        Ok(Self {
            path: path.to_owned(),
            entries,
        })
    }

    /// Append a step, flushed to disk before returning.
    ///
    /// # Errors
    /// If it can't be written; the caller must then not submit.
    pub fn append(&mut self, e: Entry) -> Result<(), String> {
        if let Some(dir) = self.path.parent() {
            fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        }
        let mut f = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
            .map_err(|e| format!("{}: {e}", self.path.display()))?;
        let line = serde_json::to_string(&e).map_err(|e| e.to_string())?;
        writeln!(f, "{line}").map_err(|e| format!("{}: {e}", self.path.display()))?;
        f.sync_all()
            .map_err(|e| format!("{}: {e}", self.path.display()))?;
        self.entries.push(e);
        Ok(())
    }

    fn latest(&self, request: &str) -> Option<&Entry> {
        self.entries.iter().rev().find(|e| e.request == request)
    }

    /// The step that blocks submitting `request` again, if any.
    #[must_use]
    pub fn blocking(&self, request: &str) -> Option<&Entry> {
        self.latest(request)
            .filter(|e| e.state == "sending" || e.state == "uncertain")
    }

    /// Every request whose latest step is unresolved.
    #[must_use]
    pub fn unresolved(&self) -> Vec<Entry> {
        let mut seen = HashSet::new();
        self.entries
            .iter()
            .rev()
            .filter(|e| seen.insert(e.request.clone()))
            .filter(|e| e.state == "sending" || e.state == "uncertain")
            .cloned()
            .collect()
    }

    /// Every job ID submitted here.
    #[must_use]
    pub fn submitted(&self) -> HashSet<String> {
        self.entries.iter().filter_map(|e| e.job.clone()).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(request: &str, state: &str, job: Option<&str>) -> Entry {
        Entry {
            at: 1.0,
            request: request.into(),
            state: state.into(),
            job: job.map(Into::into),
            cost_usd: None,
            note: None,
        }
    }

    #[test]
    fn an_unresolved_submission_blocks_the_same_request_until_reconciled() {
        let d = std::env::temp_dir().join(format!("tinytrader-ledger-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        let path = d.join("submissions.jsonl");
        let r = fingerprint(
            "OPRA.PILLAR",
            "cmbp-1",
            &["SPY.OPT".into()],
            "2026-10-01",
            "2026-10-02",
        );
        {
            let mut l = Ledger::open(&path).unwrap();
            l.append(entry(&r, "sending", None)).unwrap();
            assert!(l.blocking(&r).is_some());
            l.append(entry(&r, "uncertain", None)).unwrap();
        }
        // Reopened (a restart): still blocked, and listed for reconciliation.
        let mut l = Ledger::open(&path).unwrap();
        assert_eq!(l.blocking(&r).map(|e| e.state.as_str()), Some("uncertain"));
        assert_eq!(l.unresolved().len(), 1);
        l.append(entry(&r, "submitted", Some("OPRA-1"))).unwrap();
        assert!(l.blocking(&r).is_none());
        assert!(l.unresolved().is_empty());
        assert!(l.submitted().contains("OPRA-1"));
        // A different request is never blocked by this one.
        assert!(
            l.blocking(&fingerprint(
                "OPRA.PILLAR",
                "cmbp-1",
                &["QQQ.OPT".into()],
                "2026-10-01",
                "2026-10-02"
            ))
            .is_none()
        );
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn symbols_are_compared_in_any_order() {
        assert_eq!(
            fingerprint("D", "s", &["B".into(), "A".into()], "x", "y"),
            fingerprint("D", "s", &["A".into(), "B".into()], "x", "y")
        );
    }
}

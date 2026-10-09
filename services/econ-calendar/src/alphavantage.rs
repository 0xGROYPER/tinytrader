//! Alpha Vantage's free plan allows 25 requests a day, counted on Alpha
//! Vantage's own clock. Every request this service sends (the earnings
//! calendar and companies' histories) is counted here over the last 24
//! hours, kept on disk with `ECON_CACHE` so a restart doesn't forget them.
//! Histories stop at [`HISTORY_MAX`], leaving the rest for the calendar, and
//! two of the 25 stay spare for people using the same key.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use tinytrader_observe::lock;

/// Requests in any 24 hours, everything together.
pub const LIMIT: usize = 23;
/// Of those, at most this many for companies' histories.
pub const HISTORY_MAX: usize = 20;
const DAY: f64 = 86_400.0;

pub struct Budget {
    file: Option<PathBuf>,
    /// When each request in the last 24 hours was sent (unix seconds).
    sent: Mutex<Vec<f64>>,
}

impl Budget {
    #[must_use]
    pub fn new(dir: Option<&Path>) -> Self {
        let file = dir.map(|d| d.join("alphavantage").join("requests.json"));
        let sent = file
            .as_ref()
            .and_then(|f| std::fs::read_to_string(f).ok())
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default();
        Self {
            file,
            sent: Mutex::new(sent),
        }
    }

    /// Count a request sent at `now`, if fewer than `max` were sent in the 24
    /// hours before it.
    pub fn take(&self, max: usize, now: f64) -> bool {
        let mut sent = lock(&self.sent);
        sent.retain(|t| now - t < DAY);
        if sent.len() >= max {
            return false;
        }
        sent.push(now);
        // Best effort, atomically: a lost write only forgets requests.
        if let Some(f) = &self.file {
            let _ = f.parent().map(std::fs::create_dir_all);
            let tmp = f.with_extension("json.tmp");
            if std::fs::write(&tmp, serde_json::to_string(&*sent).unwrap_or_default()).is_ok() {
                let _ = std::fs::rename(&tmp, f);
            }
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_count_over_24_hours_and_across_restarts() {
        let dir = std::env::temp_dir().join(format!("tinytrader-av-budget-{}", std::process::id()));
        let b = Budget::new(Some(&dir));
        assert!(b.take(2, 1000.0));
        assert!(b.take(2, 2000.0));
        assert!(!b.take(2, 3000.0), "two in 24 hours already");
        // A restart remembers them.
        let b = Budget::new(Some(&dir));
        assert!(!b.take(2, 4000.0));
        assert!(b.take(3, 4000.0), "a higher limit leaves room");
        // A day after the first, it no longer counts.
        assert!(b.take(3, 1000.0 + DAY));
        let _ = std::fs::remove_dir_all(&dir);
    }
}

//! Active hours: keep the provider connection only during the relay's daily
//! window on trading days, and report the relay idle outside it.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use relay::schedule::{self, Calendar, Schedule};
use tinytrader_observe::{Observer, State};

use crate::hub::{Hub, lock};
use crate::protocol;

/// How often the market calendar is fetched again.
const CALENDAR_TTL: Duration = Duration::from_secs(3600);

pub struct Hours {
    pub schedule: Option<Schedule>,
    /// Where to read the market calendar (`{ "days": [...] }`); without it,
    /// every weekday is a trading day.
    pub calendar_url: Option<String>,
    pub http: reqwest::Client,
    cache: Mutex<Option<(Calendar, Instant)>>,
}

impl Hours {
    pub fn new(
        schedule: Option<Schedule>,
        calendar_url: Option<String>,
        http: reqwest::Client,
    ) -> Self {
        Self {
            schedule,
            calendar_url,
            http,
            cache: Mutex::new(None),
        }
    }

    async fn calendar(&self) -> Calendar {
        if let Some((c, at)) = lock(&self.cache).as_ref()
            && at.elapsed() < CALENDAR_TTL
        {
            return c.clone();
        }
        let Some(url) = &self.calendar_url else {
            return Calendar::default();
        };
        let fetched = async {
            let r = self.http.get(url).send().await.ok()?;
            r.json::<Calendar>()
                .await
                .ok()
                .filter(|c| !c.days.is_empty())
        };
        match fetched.await {
            Some(c) => {
                *lock(&self.cache) = Some((c.clone(), Instant::now()));
                c
            }
            // Keep the last calendar we had, or fall back to weekdays.
            None => lock(&self.cache)
                .as_ref()
                .map(|(c, _)| c.clone())
                .unwrap_or_default(),
        }
    }

    /// Wait until the relay should be active, reporting it idle meanwhile, and
    /// return when the current window ends (ns since the epoch). Without a
    /// schedule the relay is always active.
    pub async fn wait_until_active(&self, hub: &Hub, observer: &Observer) -> u64 {
        let Some(s) = self.schedule else {
            return u64::MAX;
        };
        loop {
            let now = protocol::now_ns();
            match s.next(now, &self.calendar().await) {
                Some((start, end)) if start <= now => return end,
                Some((start, _)) => {
                    hub.off_schedule(start);
                    observer.set_state(
                        State::Idle,
                        &format!("off schedule until {}", schedule::describe(start)),
                    );
                    // Wake at the start, or every few minutes to pick up calendar changes.
                    let wait = Duration::from_nanos(start - now).min(Duration::from_secs(300));
                    tokio::time::sleep(wait).await;
                }
                None => {
                    hub.off_schedule(u64::MAX);
                    observer.set_state(
                        State::Idle,
                        "off schedule: no trading day in the next two weeks",
                    );
                    tokio::time::sleep(Duration::from_secs(3600)).await;
                }
            }
        }
    }
}

/// Sleep until a time in ns since the epoch.
pub async fn until(end: u64) {
    if end == u64::MAX {
        return std::future::pending().await;
    }
    let now = protocol::now_ns();
    if end > now {
        tokio::time::sleep(Duration::from_nanos(end - now)).await;
    }
}

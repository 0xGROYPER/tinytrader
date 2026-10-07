//! One interface over the two ways a front end can see a deployment: a
//! poller it owns, or a console that is already running.

use std::sync::Arc;
use std::time::Duration;

use crate::{EventsPage, Poller, Snapshot, Topology};

/// Where a front end reads the deployment from.
pub enum Source {
    /// Poll the deployment directly. Call [`Source::refresh`] periodically.
    Direct(Arc<Poller>),
    /// Read a running console's API at this base URL, e.g. `http://localhost:8080`.
    Console { base: String, http: reqwest::Client },
}

impl Source {
    /// A source reading the console at `base`.
    #[must_use]
    pub fn console(base: &str) -> Self {
        Self::Console {
            base: base.trim_end_matches('/').to_owned(),
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(3))
                .build()
                .expect("http client"),
        }
    }

    /// Poll once when direct; a running console polls on its own.
    pub async fn refresh(&self) -> Option<String> {
        match self {
            Self::Direct(p) => p.poll().await,
            Self::Console { .. } => None,
        }
    }

    pub async fn topology(&self) -> Result<Topology, String> {
        match self {
            Self::Direct(p) => Ok(p.topology()),
            Self::Console { base, http } => get(http, &format!("{base}/api/topology")).await,
        }
    }

    pub async fn snapshot(&self) -> Result<Snapshot, String> {
        match self {
            Self::Direct(p) => Ok(p.snapshot()),
            Self::Console { base, http } => get(http, &format!("{base}/api/state")).await,
        }
    }

    /// A relay's one-minute bars for `symbol` over the last `days`
    /// (`{ "bars": [[t, o, h, l, c, v], ...] }`).
    pub async fn bars(
        &self,
        service: &str,
        symbol: &str,
        days: u32,
    ) -> Result<serde_json::Value, String> {
        let query = format!("symbol={symbol}&days={days}");
        match self {
            Self::Direct(p) => match p.bars_json(service, &query).await {
                Ok((200, v)) => Ok(v),
                Ok((code, v)) => Err(v["error"]
                    .as_str()
                    .map_or_else(|| format!("{code}"), str::to_owned)),
                Err(e) => Err(e.to_string()),
            },
            Self::Console { base, http } => {
                get(http, &format!("{base}/api/bars/{service}?{query}")).await
            }
        }
    }

    /// Events from `service` newer than `after`, at most `limit`.
    pub async fn events(
        &self,
        service: &str,
        after: u64,
        limit: u64,
    ) -> Result<EventsPage, String> {
        match self {
            Self::Direct(p) => p
                .events_json(service, after, limit)
                .await
                .map_err(|e| e.to_string())
                .and_then(|v| serde_json::from_value(v).map_err(|e| e.to_string())),
            Self::Console { base, http } => {
                get(
                    http,
                    &format!("{base}/api/events/{service}?after={after}&limit={limit}"),
                )
                .await
            }
        }
    }
}

async fn get<T: serde::de::DeserializeOwned>(
    http: &reqwest::Client,
    url: &str,
) -> Result<T, String> {
    let r = http.get(url).send().await.map_err(|e| e.to_string())?;
    if !r.status().is_success() {
        return Err(format!("{} from {url}", r.status()));
    }
    r.json::<T>().await.map_err(|e| e.to_string())
}

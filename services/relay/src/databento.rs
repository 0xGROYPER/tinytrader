//! Databento source: one live session feeds the hub. It replays the day's
//! reference data (instrument definitions and statistics, by default) when it
//! connects, so the hub holds them for joining clients even after a restart,
//! and then continues live.
//!
//! Market data is not replayed: Databento refuses intraday replay for
//! `cmbp-1`, and missed quotes come from the next day's historical data.

use std::sync::Arc;
use std::time::{Duration, Instant};

use databento::LiveClient;
use databento::dbn::{SType, Schema, VersionUpgradePolicy};
use databento::live::Subscription;
use time::OffsetDateTime;
use tinytrader_observe::{Observer, State};

use crate::hub::Hub;

/// Records are passed to clients in chunks of about this many bytes.
const CHUNK: usize = 64 * 1024;

pub struct Databento {
    pub key: String,
    pub dataset: String,
    pub symbols: Vec<String>,
    pub schema: Schema,
    /// Reference schemas sent live alongside `schema` and replayed for
    /// clients catching up, such as `definition` and `statistics`.
    pub reference: Vec<Schema>,
    pub stype_in: SType,
}

impl Databento {
    /// A started session for the configured symbols: the schema live, and the
    /// reference schemas replayed from as early as Databento has them (up to
    /// 24 hours) and then live.
    async fn session(&self, heartbeat: Duration) -> Result<LiveClient, String> {
        let mut client = LiveClient::builder()
            .key(&self.key)
            .map_err(|e| format!("invalid API key: {e}"))?
            .dataset(&self.dataset)
            .upgrade_policy(VersionUpgradePolicy::UpgradeToV3)
            .heartbeat_interval(time::Duration::seconds(
                i64::try_from(heartbeat.as_secs()).unwrap_or(10),
            ))
            .build()
            .await
            .map_err(|e| format!("cannot connect to Databento: {e}"))?;
        let replay = Some(OffsetDateTime::UNIX_EPOCH);
        let subs =
            std::iter::once((self.schema, None)).chain(self.reference.iter().map(|s| (*s, replay)));
        for (schema, from) in subs {
            let sub = Subscription::builder()
                .symbols(self.symbols.clone())
                .schema(schema)
                .stype_in(self.stype_in);
            let sub = match from {
                Some(t) => sub.start(t).build(),
                None => sub.build(),
            };
            client
                .subscribe(sub)
                .await
                .map_err(|e| format!("cannot subscribe to {schema}: {e}"))?;
        }
        client
            .start()
            .await
            .map_err(|e| format!("cannot start the session: {e}"))?;
        Ok(client)
    }

    /// Keep the standing session up, publishing its records to the hub.
    pub async fn run(self: Arc<Self>, hub: Arc<Hub>, observer: Observer) {
        let mut backoff = Duration::from_secs(1);
        loop {
            let began = Instant::now();
            let outcome = self.stream(&hub, &observer).await;
            hub.upstream_down();
            let why = match outcome {
                Ok(()) => "Databento closed the session".to_owned(),
                Err(e) => e,
            };
            eprintln!("relay: {why}; reconnecting in {}s", backoff.as_secs());
            observer.record_error(&why);
            observer.set_state(State::Degraded, &format!("{why}; reconnecting"));
            if began.elapsed() > Duration::from_secs(60) {
                backoff = Duration::from_secs(1);
            }
            tokio::time::sleep(backoff).await;
            backoff = (backoff * 2).min(Duration::from_secs(30));
        }
    }

    async fn stream(&self, hub: &Hub, observer: &Observer) -> Result<(), String> {
        let mut client = self.session(hub.heartbeat()).await?;
        hub.upstream_up();
        observer.set_state(State::Ready, "live");
        eprintln!(
            "relay: Databento session live for {}",
            self.symbols.join(",")
        );
        loop {
            let mut chunk = Vec::with_capacity(CHUNK);
            let mut n = 0;
            match client.next_record().await {
                Ok(Some(r)) => chunk.extend_from_slice(r.as_ref()),
                Ok(None) => return Ok(()),
                Err(e) => return Err(format!("Databento session failed: {e}")),
            }
            n += 1;
            // Take whatever else is already buffered, up to a chunk.
            while chunk.len() < CHUNK {
                match client.try_next_record() {
                    Ok(Some(r)) => {
                        chunk.extend_from_slice(r.as_ref());
                        n += 1;
                    }
                    Ok(None) => break,
                    Err(e) => return Err(format!("Databento session failed: {e}")),
                }
            }
            hub.publish(chunk, n);
        }
    }
}

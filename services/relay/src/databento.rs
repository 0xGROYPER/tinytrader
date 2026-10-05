//! Databento source: one standing live session feeds the hub. A client that
//! asks to catch up gets its own session, which replays the day's reference
//! data (instrument definitions and statistics, by default) and then
//! continues live.
//!
//! Quotes are not replayed: Databento refuses intraday replay for `cmbp-1`,
//! and missed quotes come from the next day's historical data.

use std::sync::Arc;
use std::time::{Duration, Instant};

use databento::LiveClient;
use databento::dbn::{ErrorCode, Metadata, SType, Schema, VersionUpgradePolicy};
use databento::live::Subscription;
use glacier_observe::{Observer, State};
use time::OffsetDateTime;
use tokio::io::AsyncWriteExt as _;
use tokio::net::tcp::OwnedWriteHalf;
use tokio::sync::Semaphore;

use crate::hub::{ClientGuard, Hub};
use crate::protocol;

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
    /// Limits concurrent replay sessions, which count against Databento's
    /// per-dataset session limit.
    pub replays: Semaphore,
}

impl Databento {
    /// A started session for the configured symbols: the schema, live, plus
    /// the reference schemas. With `catch_up`, the reference schemas are
    /// replayed from as early as Databento has them (up to 24 hours), so the
    /// client knows every instrument and the day's statistics.
    async fn session(
        &self,
        catch_up: bool,
        heartbeat: Duration,
    ) -> Result<(LiveClient, Metadata), String> {
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
        let replay = catch_up.then_some(OffsetDateTime::UNIX_EPOCH);
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
        let metadata = client
            .start()
            .await
            .map_err(|e| format!("cannot start the session: {e}"))?;
        Ok((client, metadata))
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
        let (mut client, _) = self.session(false, hub.heartbeat()).await?;
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

    /// Serve one client from its own catch-up session until either side ends.
    /// `start` sets the header; the reference schemas replay from as early as
    /// Databento has them, whatever it is.
    pub async fn replay(&self, start: OffsetDateTime, mut out: OwnedWriteHalf, hub: &Hub) {
        let Ok(_permit) = self.replays.try_acquire() else {
            hub.refuse(
                out,
                ErrorCode::ConnectionLimitExceeded,
                "too many clients are catching up; try again shortly",
            )
            .await;
            return;
        };
        let _client = ClientGuard::new(&hub.clients);
        hub.events.push(
            format!("catch-up requested from {start}"),
            format!("catch-up requested from {start}"),
        );
        let (mut session, _) = match self.session(true, hub.heartbeat()).await {
            Ok(s) => s,
            Err(e) => {
                hub.refuse(out, ErrorCode::InternalError, &e).await;
                return;
            }
        };
        // The relay's own header, not the session's, so every client sees
        // the same dataset, symbols and symbology.
        let start = u64::try_from(start.unix_timestamp_nanos()).unwrap_or(0);
        let Ok(header) = protocol::header(&hub.metadata(start)).await else {
            return;
        };
        if out.write_all(&header).await.is_err() {
            return;
        }
        loop {
            let mut chunk = Vec::with_capacity(CHUNK);
            match session.next_record().await {
                Ok(Some(r)) => chunk.extend_from_slice(r.as_ref()),
                Ok(None) => break,
                Err(e) => {
                    let msg = format!("Databento session failed: {e}");
                    let _ = out
                        .write_all(&protocol::error(ErrorCode::InternalError, &msg))
                        .await;
                    break;
                }
            }
            while chunk.len() < CHUNK {
                match session.try_next_record() {
                    Ok(Some(r)) => chunk.extend_from_slice(r.as_ref()),
                    _ => break,
                }
            }
            if out.write_all(&chunk).await.is_err() {
                break;
            }
        }
        let _ = out.shutdown().await;
        let _ = session.close().await;
    }
}

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
use databento::dbn::{
    ErrorCode, ErrorMsg, RecordRef, SType, Schema, SystemCode, SystemMsg, VersionUpgradePolicy,
};
use databento::live::Subscription;
use futures_util::future::BoxFuture;
use time::OffsetDateTime;
use tinytrader_observe::{Observer, State};

use crate::hours::{self, Hours};
use crate::hub::Hub;
use crate::source::{Feed, Source};

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

    async fn stream(&self, hub: &Hub, observer: &Observer) -> Result<(), String> {
        let mut client = self.session(hub.heartbeat()).await?;
        hub.upstream_up();
        observer.set_state(State::Ready, "live");
        eprintln!(
            "relay: Databento session live for {}",
            self.symbols.join(",")
        );
        let mut gaps = Gaps::default();
        loop {
            let mut chunk = Vec::with_capacity(CHUNK);
            let mut n = 0;
            match client.next_record().await {
                Ok(Some(r)) => gaps.take(hub, r, &mut chunk, &mut n),
                Ok(None) => return Ok(()),
                Err(e) => return Err(format!("Databento session failed: {e}")),
            }
            // Take whatever else is already buffered, up to a chunk.
            while chunk.len() < CHUNK {
                match client.try_next_record() {
                    Ok(Some(r)) => gaps.take(hub, r, &mut chunk, &mut n),
                    Ok(None) => break,
                    Err(e) => return Err(format!("Databento session failed: {e}")),
                }
            }
            if !chunk.is_empty() {
                hub.publish(chunk, n);
            }
        }
    }
}

impl Source for Databento {
    fn feed(&self) -> Feed {
        Feed {
            dataset: self.dataset.clone(),
            schema: self.schema.to_string(),
            stype_in: self.stype_in,
            provider: "databento",
            name: "Databento",
            via: "live API",
        }
    }

    /// Keep the standing session up, publishing its records to the hub.
    fn run(
        self: Arc<Self>,
        hub: Arc<Hub>,
        observer: Observer,
        hours: Arc<Hours>,
    ) -> BoxFuture<'static, ()> {
        Box::pin(async move {
            let mut backoff = Duration::from_secs(1);
            loop {
                let end = hours.wait_until_active(&hub, &observer).await;
                let began = Instant::now();
                // At the end of the active window the session is dropped, which
                // disconnects it.
                let outcome = tokio::select! {
                    r = self.stream(&hub, &observer) => Some(r),
                    () = hours::until(end) => None,
                };
                let Some(outcome) = outcome else {
                    continue;
                };
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
        })
    }
}

/// Databento's own slow-reader messages are about the relay, and use the
/// codes the relay sends its clients when it drops them; they are turned into
/// the relay's markers instead of being passed on. A skip becomes
/// `upstream-gap gap_from=<ns> gap_to=<ns>`, sent before the first record
/// after it: from the last record before the skip to that record.
#[derive(Default)]
struct Gaps {
    last_ts: u64,
    skipped_since: Option<u64>,
}

impl Gaps {
    fn take(&mut self, hub: &Hub, r: RecordRef, chunk: &mut Vec<u8>, n: &mut u64) {
        if let Some(e) = r.get::<ErrorMsg>()
            && e.code().ok() == Some(ErrorCode::SkippedRecordsAfterSlowReading)
        {
            self.skipped_since.get_or_insert(self.last_ts);
            return;
        }
        if let Some(m) = r.get::<SystemMsg>()
            && m.code().ok() == Some(SystemCode::SlowReaderWarning)
        {
            hub.upstream_slow();
            return;
        }
        let ts = r.header().ts_event;
        if let Some(from) = self.skipped_since.take() {
            chunk.extend(hub.upstream_gap(from, ts));
        }
        if r.get::<ErrorMsg>().is_none() && r.get::<SystemMsg>().is_none() {
            self.last_ts = ts;
        }
        chunk.extend_from_slice(r.as_ref());
        *n += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use databento::dbn::{Cmbp1Msg, Metadata};

    fn quote(ts: u64) -> Cmbp1Msg {
        let mut q = Cmbp1Msg::default_for_schema(Schema::Cmbp1);
        q.hd.instrument_id = 7;
        q.hd.ts_event = ts;
        q
    }

    #[test]
    fn a_provider_skip_becomes_a_gap_marker() {
        let hub = Hub::new(
            Metadata::builder()
                .dataset("OPRA.PILLAR".to_owned())
                .schema(None)
                .start(0)
                .stype_in(None)
                .stype_out(SType::InstrumentId)
                .build(),
            8,
            Duration::from_secs(10),
        );
        let mut gaps = Gaps::default();
        let (mut chunk, mut n) = (Vec::new(), 0);
        let before = quote(100);
        gaps.take(&hub, RecordRef::from(&before), &mut chunk, &mut n);
        let skip = ErrorMsg::new(
            150,
            Some(ErrorCode::SkippedRecordsAfterSlowReading),
            "Slow client detected for cmbp-1. Skipped records",
            true,
        );
        gaps.take(&hub, RecordRef::from(&skip), &mut chunk, &mut n);
        let after = quote(200);
        gaps.take(&hub, RecordRef::from(&after), &mut chunk, &mut n);
        assert_eq!(n, 2, "the provider's error isn't passed on");
        let text = String::from_utf8_lossy(&chunk);
        assert!(
            text.contains("upstream-gap gap_from=100 gap_to=200"),
            "{text}"
        );
        assert!(!text.contains("Slow client detected"));
        assert_eq!(
            hub.upstream_skips
                .load(std::sync::atomic::Ordering::Relaxed),
            1
        );
    }
}

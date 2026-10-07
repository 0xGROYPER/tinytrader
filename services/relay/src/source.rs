//! What the relay needs from a provider. A source holds the one provider
//! connection and publishes its records into the hub; anything else it can do
//! (catch-up, trade conditions, bar history) is optional, and a client asking
//! a source for something it can't do is refused or served without it.

use std::sync::Arc;

use databento::dbn::SType;
use futures_util::future::BoxFuture;
use tinytrader_observe::Observer;
use tokio::sync::mpsc;

use crate::bars::Bar;
use crate::hours::Hours;
use crate::hub::Hub;

/// Chunks of records sent to a client catching up, before live data.
pub type History = mpsc::Receiver<Result<Vec<u8>, String>>;

/// How a source describes itself in stream headers, `/describe` and logs.
pub struct Feed {
    /// Dataset in stream headers, such as `OPRA.PILLAR`.
    pub dataset: String,
    /// Schema relayed live, such as `cmbp-1`.
    pub schema: String,
    /// Symbology of the relay's symbols.
    pub stype_in: SType,
    /// Provider id, such as `databento`.
    pub provider: &'static str,
    /// Provider name for people, such as `Databento`.
    pub name: &'static str,
    /// How the relay reaches the provider, such as `live API`.
    pub via: &'static str,
}

pub trait Source: Send + Sync {
    fn feed(&self) -> Feed;

    /// Keep the provider connection up during active hours, publishing its
    /// records to the hub and marking the upstream up, down and gaps.
    fn run(
        self: Arc<Self>,
        hub: Arc<Hub>,
        observer: Observer,
        hours: Arc<Hours>,
    ) -> BoxFuture<'static, ()>;

    /// Records from `start` to `cut` (ns since the epoch), encoded as live,
    /// for a client catching up. `None`: the client gets live data only.
    fn catch_up(self: Arc<Self>, _start: u64, _cut: u64, _conditions: bool) -> Option<History> {
        None
    }

    /// Whether clients may ask for trade conditions.
    fn conditions(&self) -> bool {
        false
    }

    /// One-minute bars for a symbol from `start` to `end` (ns since the
    /// epoch), oldest first. `None`: the source keeps no bar history.
    fn bars(
        &self,
        _symbol: &str,
        _start: u64,
        _end: u64,
    ) -> Option<BoxFuture<'_, Result<Vec<Bar>, String>>> {
        None
    }
}

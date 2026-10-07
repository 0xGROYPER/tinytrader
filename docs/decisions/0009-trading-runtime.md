# 0009: The trading runtime

- **Status:** Proposed
- **Date:** 2026-10-06

## Context

tinytrader is an integrated platform. It covers:

- storage on drives and a NAS;
- data acquisition and relays;
- jobs and scheduling;
- supervision across hosts;
- the console.

All of it ships as one package with the deployment integration built in.
Relays fan DBN v3 streams out to local clients
([0006](0006-market-data-relay.md)), and the store service keeps provider
data in Databento's layout and serves it over the network
([0002](0002-data-store.md), [0005](0005-store-service-and-supervision.md)).

Nothing in tinytrader yet acts on that data: there is no backtest loop, no
simulated venue and no live order path. Whatever acts on it is **one optional
entry point into the platform**, a client like the console, not the
platform's centre. A deployment may use it, or run its own engine in any
language against the same relays, store and jobs.

Whatever acts on the data must meet these requirements:

- **The same code in backtest and live.** Only the data sources and the
  venue differ between the two.
- **Reproducible.** The same inputs (pinned by hash), the same code and the
  same seed give the same results.
- **DBN-native.** Consumers read records as they were filed: `ts_event`,
  `ts_recv`, `flags`, `publisher_id`, CMBP-1's `bid_pb` and `ask_pb`, and our
  extension records such as `tinytrader.trade-conditions/1`.
- **Unimposing.** tinytrader is a platform
  ([0001](0001-platform-and-deployment.md)). How decisions get made belongs to
  each deployment: forecasts, inference, control, learned policies or rules.
  A Kalman filter, a message-passing model, a PyTorch network and a
  hand-written rule must all fit without one of them being privileged.

Two earlier private prototypes shaped this decision.

- **The first** built a thread-per-actor graph with a compiler, typestate
  wiring and a large model layer, all before any decision loop had paid for
  its costs. It stalled on scope, not on mechanics. Its fill model never saw
  market data.
- **The second** reached paper trading. It had a single state owner that
  handled one event at a time and did no I/O. Around it were the patterns that
  made live trading safe:
  - an order outbox written before submission;
  - reconciliation with the broker before each decision;
  - freshness guards;
  - pre-open checks that fail closed;
  - a decision trace that replays against recorded broker responses.

## Decision

tinytrader gets a **small actor runtime tailored for trading**:

- deterministic message passing in virtual time;
- DBN sources;
- a venue boundary;
- run provenance.

It takes **no position on how decisions are made**. Users write actors and
define their own message types.

The runtime is **optional**. The rest of the platform never depends on it,
and it has no private hooks into the platform. It reaches the platform
through the same documented interfaces as any other client:

- the relay protocol;
- the store service;
- the jobs service;
- `tinytrader.observe/1`;
- the topology format.

An engine written outside tinytrader therefore gets everything the runtime
gets.

## Design rules

1. **DBN is the observation format.** The runtime delivers DBN records. It
   defines no tick, quote or bar types of its own. Anything DBN lacks gets an
   extension record (an unused rtype in DBN framing), as trade conditions
   already does.
2. **No opinions on decision-making.** There is no strategy, estimator,
   policy, signal or indicator trait. Patterns such as
   estimator → policy → execution belong in examples, not in contracts.
3. **Deterministic semantics, configurable placement.** Message order,
   virtual time and mailbox policies are fixed by these rules. Which thread
   runs which actor is configuration. The single-threaded executor is the
   reference.
4. **Actors do no I/O.** They read their messages and their context. They
   send messages, set timers, issue venue commands and spawn declared kinds.
   Live I/O happens only at the edges, in sources, venues and ports.
5. **Missing is missing.** Gaps, upstream outages, silence and dropped
   messages are themselves messages. Nothing is forward-filled or zero-filled
   on an actor's behalf.
6. **Bounded.** Every mailbox, every spawnable kind and every queue has a
   fixed capacity, as CONTRIBUTING requires.
7. **Everything else is optional.** Book and chain state, options settlement,
   fee models, broker venues, feature libraries and Python bindings are
   separate crates, examples or deployment code.

## Messages

Everything that moves through the runtime is a message: one envelope (a
virtual timestamp, a delivery sequence and a sender) around a payload. The
payloads fall into kinds, but all share the same delivery path.

- **Facts:** something happened. Market records, gaps, fills, timers firing.
- **Commands:** a request that something happen. Submit, cancel, replace.
- **Entities** are not messages. An order is an entity with a state machine.
  The venue owns its authoritative state: what the broker says, and what
  reconciliation checks. Commands act on an order, and the venue's reports
  move it from state to state.

Each run has **one message type**: the user's enum. It wraps whichever
runtime messages (market, venue reports, timers, drops) its actors use,
through `From`. Everything is therefore monomorphised, with no type erasure
per message.

DBN records stay borrowed (`RecordRef`, pointing into the decoder's buffer)
for as long as possible:

- **Delivered directly** when the target actor is idle and its mailbox is
  empty: no copy and no enqueue. This is the common case.
- **Owned only when a record must wait** in a mailbox. It is then copied
  into a `dbn::RecordBuf<CAP>`, a fixed-size stack buffer. `CAP` is sized to
  the largest record the run routes as market data, not to `dbn`'s default
  `MAX_RECORD_LEN`, which is the size of an instrument definition. A record
  larger than `CAP` is a configuration error, not a silent truncation.
- **Instrument definitions travel on their own path.** They are reference
  data that actors keep anyway, so each is held once and shared by reference
  rather than copied into mailboxes.

Actors subscribe to instruments when the topology is built, so a record is
routed only to the actors that asked for it.

## Time

- `Ts` is `u64` nanoseconds since the UNIX epoch, the convention DBN and
  Databento use (see [epoch.to](https://epoch.to/)).
  - It cannot hold DBN's `UNDEF_TIMESTAMP` sentinel. A missing timestamp is
    `Option<Ts>` at the decode boundary.
  - It converts to `dbn::pretty::Ts` for display, so the runtime uses one
    formatting convention, Databento's.
- **Clocks:**
  - A virtual clock in backtests, advanced by the merged order of deliveries.
  - A live clock in live runs, which stamps each arrival.
  - Timers are messages in the same queue.
- **Service time.** In virtual time a handler takes no time, so each actor
  declares a modelled processing time. The first version supports a constant
  per actor; zero is the default.
  - An actor handling a message at `t` with service time `s` is busy until
    `t + s`.
  - What it sends is delivered at `t + s`.
  - Messages that arrive while it is busy wait in its mailbox.

## Ordering

Every delivery follows one total order, with the key `(ts, class, seq)`.

- **`ts`** for a market record is its **index timestamp**, an approximation
  of when it arrived:
  - `ts_recv`, normally;
  - `ts_event` plus the source's **declared latency offset**, where the record
    carries `F_BAD_TS_RECV`. Examples are replayed IEX history and filed IEX
    days, which have no true receive time.

  The offset is recorded in the run manifest. Without it, merging such a
  source with one indexed by `ts_recv` would compare exchange times against
  arrival times and skew ordering across assets by the feed latency.
- **`class`** orders messages that share a timestamp:
  1. market data;
  2. venue reports;
  3. timers;
  4. actor messages.
- **`seq`** is the order of creation. For market data, it is the source's
  rank in the run manifest, then the record's position within its source.

How the order is carried out:

- **Market data is merged lazily.** Each source is already in time order,
  so the runtime keeps a small heap with one entry per source and takes
  records from the source with the lowest key.
- **The priority queue holds only what the runtime generates:** timers,
  venue reports and actor messages.
- The two meet at the same key. A day of options data therefore costs one
  comparison per record, not one queue insertion and removal per record.

In live runs, the order is arrival order at the runtime, and the audit log
records every arrival.

## Mailboxes

Each actor's mailbox policy is defined in **virtual time**. It applies to
whatever collects while the actor is busy, so the same policy gives the same
result in every replay.

| Policy | Behaviour |
|---|---|
| `Lossless { capacity }` | Keeps everything. Overflow fails the run. |
| `LatestPer { key, capacity }` | Keeps only the latest message per key, e.g. per `instrument_id`. |
| `Coalesce { key, merge, capacity }` | Merges messages per key with a user function. |

Whatever a policy drops or merges is recorded in the audit log. It reaches
the actor as a `Dropped` message that says **what** was lost: the key (for
example the instrument), how many messages, and the time span. An actor can
then tell which contract went stale. Live runs use the same policies,
triggered by actual busyness rather than modelled busyness.

## Spawning

Actors may spawn other actors, within limits:

- **Only declared kinds.** Factories are registered at build time and listed
  in the run manifest. A run cannot create anything undeclared.
- **Bounded.** Each kind has a `max_live`. At capacity, `spawn` returns
  `SpawnError::AtCapacity`. That is a normal outcome for the spawner to
  handle, not a run failure. The cap doubles as a risk limit, for example "at
  most N orders being worked".
- **Deterministic.** A new actor's id is `(parent, spawn counter)`. The spawn
  takes effect in the queue like any other message.
- **Depth one.** Spawned actors cannot spawn.
- **Addressed.** The spawner receives an `Addr`. The venue routes the reports
  for an order to the actor that submitted it.
- **Ends on stop.** A spawned actor stops itself. Its mailbox is drained and
  it is removed, and its id is never reused.

Spawning suits things with a **lifecycle**, such as a working order, a
multi-leg execution or an episode. Things that form a **cross-section**, such
as the contracts in a chain, belong in one actor holding a map. Fitting a
smile or choosing strikes needs every contract at once, and a `LatestPer`
mailbox already gives that actor per-contract semantics.

## Place in the platform

A run is an ordinary platform process. The platform owns everything outside
the run, and the run uses only interfaces any client could use.

| Concern | Owned by | How a run meets it |
|---|---|---|
| Starting, stopping, restarts, resource limits | Supervision (`tinytrader.services`, [0005](0005-store-service-and-supervision.md)) | A live run is a declared service. A backtest or sweep is a job. |
| When to run | `schedule` and the jobs service | Active hours from the market calendar. A live run reports "off schedule" outside them, not a fault. |
| Historical inputs | The store service | Read over the network by dataset, symbol, schema and day. The run pins the SHA-256 the store recorded for each day. It never takes a data path. |
| Live inputs | Relays | Joined like any other client (`tinytrader.relay/1`). |
| Health and metrics | `tinytrader.observe/1` | `/health` and metrics with low-cardinality labels: never order ids. |
| Visibility | The topology format and the console | A run appears like any other service. Its results and audit hash are listed per run. |
| Results | The store service | Run manifests, audit logs and results are written through the store, immutable and pinned like data. |
| Other processes (training, sweeps, models) | The jobs service | Only requested, never spawned (below). |

- **An actor error fails the run.** The run fails closed, and the supervisor
  decides what happens next. Restarting an actor that has lost its state
  during a session is how positions drift.
- **Runs request; they never spawn processes.** An actor writes a request
  (for example "retrain on today's data") to an outbound port connected to
  the jobs service, and the platform decides whether to act on it. The
  request is a recorded output. Any answer comes back as a recorded input.
- **Live venues reconcile before any actor runs.** If the venue's state and
  the broker's do not match, the run does not start.

## Outside the process

Models in other languages, such as Python with PyTorch, join the relays like
any other client. They send results back through an **inbound port** as
framed messages, each carrying an `as_of`: the order key of the last input
the model saw.

- **Live:** the runtime records each arrival.
- **Backtest:** the model runs over the same pinned days. Its outputs are
  filed and pinned as inputs, and each one is delivered at its `as_of` plus a
  modelled latency.
- **Payload:** opaque bytes to the runtime. Its schema belongs to the
  deployment.

`as_of` makes lookahead and training/serving skew checkable. Online learning
inside the runtime avoids skew by construction, because labels arrive as
messages when they become known.

## Executors

| Executor | Use |
|---|---|
| `Deterministic` | One thread. The reference semantics. Always used for backtests. |
| `Threaded(placement)` | Live only. Actors are pinned to threads by configuration. Deferred until a live actor needs its own core. |

Every executor writes an audit log that the deterministic executor can
replay exactly. Throughput for backtests comes from running many runs in
parallel, not from threads within one run.

## Provenance

- **The run manifest** pins:
  - every input, by SHA-256;
  - the code revision;
  - the seed;
  - the configuration;
  - the topology;
  - the declared spawnable kinds.
- **The audit log** records in full everything actors do and everything
  the runtime generates:
  - sends and timers;
  - clock reads;
  - venue commands and reports;
  - spawns, stops and drops;
  - port messages.
- **Market deliveries are summarised in backtests, not logged.** The pinned
  inputs and the ordering rules fully determine them. Logging them would
  make the log larger than the data, so the log records how far each source
  was read.
- **Live runs record arrival order.** Interleaving there is not determined
  by any input. The log keeps a compact `(source, position, arrival ts)` for
  each record, which is enough to replay the session exactly against the
  relay stream as recorded.
- **The audit hash** is a running hash, updated as the log is written. It
  never needs the whole log in memory or on disk.
- Two runs with the same manifest must produce the same audit hash. This is
  a test in `nix flake check`.
- Randomness comes only from the run seed. Each actor gets its own stream,
  derived from the seed and the actor's id.

## Sketch

```rust
pub struct Ts(u64);                        // ns since the UNIX epoch; never UNDEF_TIMESTAMP
pub struct Seq(u64);
pub struct ActorId(u32);
pub struct Addr<M> { id: ActorId, _m: PhantomData<M> }

pub struct Envelope<'a, M> { pub ts: Ts, pub seq: Seq, pub from: ActorId, pub msg: &'a M }

pub enum Market<'a, const CAP: usize> {
    Record(Rec<'a, CAP>),                  // borrowed when delivered directly, owned when queued
    Definition(Arc<dbn::InstrumentDefMsg>),
    Gap { source: SourceId, from: Ts, to: Option<Ts> },
    Upstream { source: SourceId, up: bool },
    ReplayCompleted { source: SourceId },
}
pub enum Rec<'a, const CAP: usize> { Borrowed(dbn::RecordRef<'a>), Owned(dbn::RecordBuf<CAP>) }

pub enum VenueCommand { Submit(OrderRequest), Cancel(OrderId), Replace(OrderId, OrderChange) }
pub enum VenueReport {
    Accepted(OrderId), Rejected(OrderId, Reason), Fill(Fill), Canceled(OrderId),
    Expired(PositionId), Settled { position: PositionId, cash: Money },   // cash-settled index options
    Exercised(PositionId), Assigned(PositionId),                          // physically settled options
    Halted(Reason),                                                       // risk gate kill switch
    Reconciled(VenueSnapshot),
}
pub struct Timer(pub TimerId);
pub struct Dropped { pub key: u64, pub count: u32, pub from: Ts, pub to: Ts }

pub trait Actor<M>: 'static {
    fn handle(&mut self, env: Envelope<'_, M>, cx: &mut Cx<'_, M>) -> Result<(), ActorError>;
}

impl<M> Cx<'_, M> {
    pub fn now(&self) -> Ts;
    pub fn send<T: Into<M>>(&mut self, to: ActorId, msg: T);
    pub fn timer(&mut self, at: Ts, id: TimerId);
    pub fn venue(&mut self, v: VenueId, cmd: VenueCommand) -> OrderId;
    pub fn spawn(&mut self, kind: KindId, init: M) -> Result<ActorId, SpawnError>;
    pub fn stop(&mut self);
    pub fn request(&mut self, port: PortId, bytes: &[u8]);
    pub fn rng(&mut self) -> &mut ActorRng;
}

pub enum Mailbox<M> {
    Lossless { capacity: usize },
    LatestPer { key: fn(&M) -> u64, capacity: usize },
    Coalesce { key: fn(&M) -> u64, merge: fn(&mut M, M), capacity: usize },
}
pub enum ServiceTime { Zero, Constant(Duration) }

pub trait Source {                         // store service, relay, direct provider, inbound port
    fn latency_offset(&self) -> Duration;  // declared; applied where F_BAD_TS_RECV is set
    fn peek_ts(&self) -> Option<Ts>;       // index timestamp of the next record, for the lazy merge
    fn next(&mut self) -> Result<Option<dbn::RecordRef<'_>>, SourceError>;
}
pub trait Venue {                          // simulated or broker; owns order state
    fn command(&mut self, now: Ts, cmd: VenueCommand, out: &mut Reports);
    fn market(&mut self, now: Ts, rec: dbn::RecordRef<'_>, out: &mut Reports);
    fn reconcile(&mut self, out: &mut Reports) -> Result<(), ReconcileError>;
}

let rt = Runtime::<Msg>::builder(manifest)
    .source("opra", StoreSource::days(store_addr, dataset, symbol, schema, days)?)   // over the network; SHA-256s pinned
    .venue("sim", RiskGate::new(SimVenue::new(fill_model, latency_model), limits))   // same gate live
    .actor("chain", Chain::default(), Mailbox::LatestPer { key: instrument, capacity: 4096 }, ServiceTime::Zero)
    .actor("decide", Decide::new(cfg), Mailbox::Lossless { capacity: 1024 }, ServiceTime::Constant(ms(2)))
    .kind("work-order", WorkOrder::new,
          Spawn { max_live: 16, mailbox: Mailbox::Lossless { capacity: 64 }, service: ServiceTime::Zero })
    .route("opra", ["chain", "sim"])
    .route("chain", ["decide"])
    .build()?;

let result: RunResult = rt.run(Executor::Deterministic)?;   // includes the audit hash
```

The topology is a builder in Rust or an equivalent configuration file. There
is no graph compiler or intermediate representation.

## Crates

| Crate | Contents |
|---|---|
| `tinytrader-runtime` | Time, merge and queue, mailboxes, spawning, executors, audit, and the `RiskGate<V: Venue>` wrapper: position and notional limits plus a kill switch, around any venue. Budget: 3–5k lines for the first version. |
| `tinytrader-sources` | Store service reads (filed days, by pinned SHA-256), the relay client, inbound ports. |
| `tinytrader-venue-sim` | A simulated venue. Fill and latency models are traits; the default fill model is risk-averse (a resting order fills only when the opposite side trades through its price). |
| `tinytrader-venue-alpaca` | Alpaca. It has: an outbox written before submission, the trade-updates stream, reconciliation at start, and freshness checks. |

Out of scope for this decision:

- book, chain and instrument state;
- options settlement and margin;
- feature libraries;
- Python bindings;
- distributed runs.

Each can become its own crate later, or stay in deployments. Strategies,
models and their message schemas always live in deployments.

The **sim venue emits expiry and settlement reports** (expired, settled,
exercised, assigned) from instrument definitions and a settlement model.
Options positions change at expiry without any order, and 0DTE makes that
matter from the first day. The settlement *models* themselves (cash at
intrinsic, AM versus PM, physical delivery) are out of scope above. The
*reports* are part of the venue contract.

**The risk gate wraps every venue.** Spawn caps limit how many orders are
being worked at once, not exposure. `RiskGate` limits exposure and wraps the
simulated venue as well as live ones, so backtests run under the same limits
as live trading.

**These crates are public** and published. They follow semver. Until 1.0, a
minor version may break the API, and every break is listed in the changelog.
Deployments pin exact versions.

## Plan

1. **Runtime and store-service sources.** Ordering, mailboxes, service
   time, spawning and the audit hash. Reading ranges back from the store
   service comes first: 0005 lists it as later work.
   - The determinism test goes into `nix flake check`.
   - It runs on **synthetic fixtures generated by code** and kept in the
     repository: small, and free to publish. Provider data is never
     committed.
   - The fixtures cover the extension record, gaps, `F_BAD_TS_RECV` rows,
     ties in timestamps and an expiry.
2. **Simulated venue**, with fill, latency and fee models.
3. **Alpaca paper venue**, carrying over the second prototype's live
   patterns.
4. **One decision loop paper-trading end to end.** Then calibrate the fill
   model against the recorded paper fills.

Before this record is accepted, a time-boxed prototype checks four things:

- Two runs over five filed days give the same audit hash.
- Throughput over those days, in records per second.
- Lines of code, the runtime's and the user's.
- A day's recording of the relay's live stream matches that day's filed
  file. Databento's intraday replay cannot do this check, because it refuses
  `cmbp-1` ([0006](0006-market-data-relay.md)).

## When to reverse

- The runtime is not running one decision loop on a paper venue within
  about 12 weeks.
- The first version outgrows its 3–5k line budget before then.
- Determinism cannot be held: equal manifests give different audit hashes,
  and the cause is the design rather than a bug.

## Consequences

- tinytrader gains libraries that run decision logic, but no decision logic
  of its own. Deployments supply it, as they supply topology and inventory.
- The hard parts remain:
  - fill models for options without queue data;
  - reconciliation across restarts and partial fills;
  - time handling: DST, half days, OPRA expirations stamped at midnight UTC,
    and AM versus PM settlement.
- `dbn` is pinned exactly. It is pre-1.0 and breaks its API between minor
  versions.
- Extension rtypes work with the `dbn` crate, but Databento does not
  document them as a contract. Databento's Python and CSV tools will not read
  them.

## Settled in review

- **Service time is per actor** (a constant) in the first version. A version
  that depends on the message can be added later without breaking anything.
- **No threaded executor for the first live deployment.** One decision loop
  on one thread is enough for paper trading.
- **Further spawnable kinds,** such as episodes in online learning, wait
  until a real use needs them.

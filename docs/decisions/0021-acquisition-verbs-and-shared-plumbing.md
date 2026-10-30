# 0021: Producers, verbs and addresses for acquisition, on shared plumbing

- **Status:** Accepted for the first deliverable, steps 0–3 (2026-10-26); the rest Proposed, each step accepted on its own
- **Date:** 2026-10-26
- **Builds on:** [0003](0003-front-ends.md) (one data layer, any number of
  front ends), [0014](0014-history-feeds.md) (history feeds, and why Hyphae's
  feed wasn't ported), [0019](0019-purchase-guards.md) (a person approves
  every charge), and the [relay](../api/relay.md), [store](../api/store.md)
  and [databento-jobs](../api/databento-jobs.md) APIs

## Context

Getting market data into the store is one job, data acquisition, done today
by several services that each carry their own plumbing:

- **Historical:** `databento-jobs` (paid, priced and approved) and
  `alpaca-history` (free, paced). Both upload a batch to the store and then
  file it (`PUT /incoming`, then `POST /incorporate`), and the code doing it
  is written twice.
- **Live:** the relay's `Source` trait (Alpaca, Databento) is the only shared
  feed interface. Every other connection (ticker, alpaca-news,
  alpaca-account, order-gateway) opens its own websocket.
- **Reconnects and retries** are hand-written at least seven times, each a
  doubling wait capped at 30 or 60 s, most without a bound. HTTP clients are
  set up in about fifteen places. DBN encoding is in five places, not
  `libs/dbn`.
- **Operation is web-only for writes.** Everything can be read with curl, but
  the one approval path is the console's button, and quoting, approving and
  watching a purchase from a terminal means hand-written curl and the
  approval token's file. A 60-minute quote approved late is lost
  without notice (2026-10-25: two in a row).
- **No replay from the store.** The relay's `start` re-fetches from the
  provider where it can (Alpaca) and can't for OPRA `cmbp-1`. A strategy
  can't be run over held days through the same interface it runs live on.

Hyphae's feed (about 30k lines) got two things right that tinytrader lacks:
one operation that a command, a recurring job and a service all invoke
(`fetch`, `sync`, `listen`; `poll` was planned), and the same DBN bytes going
to live clients, the recorder and a generic replay. 0014's reason for not
porting it still holds; this takes those two properties, not its code.

## Decision

This takes Hyphae's acquisition and distribution model (its `docs/TODO.org`
architecture, `docs/src/{architecture,concepts,cli,reactive-acquisition}.md`,
`transport-v1` and `dbn-publication-v1`), mapped onto tinytrader's services.
Where Hyphae specified something it never built, that is said.

### 1. Roles: producers, distribution, subscribers

| Role | What it is | tinytrader today |
|---|---|---|
| **Producer** | Turns a source into DBN records. Two kinds, siblings with one output contract: a **provider** producer (a vendor's API) and the **store** producer (held days). | relay sources, `alpaca-history`, `databento-jobs`; no store producer |
| **Distribution** | Fans a producer's stream out to any number of subscribers. Never parses or judges the records. | the relay's hub |
| **Subscriber** | Anything that reads a stream: a strategy, the console, the recorder. | relay clients |
| **Recorder** | An ordinary subscriber that files what it reads in the store. | none (the relay keeps no recording, 0006; 0013 proposes capture) |

**Zero-consumer correctness.** A producer publishes with no subscribers at
all, and nothing a subscriber does (including the recorder: its capacity,
its commits, its failures) enters the producer's progress. A slow live
subscriber is cut off with an explicit error, never allowed to slow the
provider connection or the others (the relay does this today). The store
producer defaults to lossless: it waits for its reader.

**Gaps say whose they are.** A source gap (the provider skipped, the
connection dropped) is published to everyone. A subscriber's own loss (it
fell behind and was cut off, its disk filled) is that subscriber's, and is
never published as a source gap.

**One output contract for both producers.** The store producer sends the
stored DBN unchanged, so a subscriber can't tell a replay from a live
session except by the metadata that says which it is.

**Lossless and fanned out are different paths.** A lossless replay is a
direct stream from the store producer to its one subscriber, paced by that
subscriber's reads; it does not go through the hub. Through the hub (a
replay several subscribers share), the live rule applies: a subscriber
that falls too far behind is cut off with an explicit error, and the
others go on. The request says which it wants; there is no case where both
rules apply.

### 2. Verbs: how a producer asks its source

| Verb | Transport | What it does |
|---|---|---|
| `fetch` | request/response (REST) | A finite range, all pages, with bounded retries and an explicit outcome. Free goes ahead; paid is priced and waits for a person's yes. |
| `listen` | stream (websocket) | A provider's live feed, with reconnects and gaps marked. From the store, the held days as a stream: that is replay. |

`replay` is not a verb: it is `listen` (or `fetch`) with `store` as the
source. Which source is a designation, the first argument; the verbs mean
the same for both.

**`--every`: the one trigger keyword.** A poll is a `fetch` that repeats:
the same provider operation, pages, retries and output, with only the range
and the trigger as inputs. The trigger is either a time or an event that
arrives on a stream:

- `--every 5s`, `--every 1m`: clock-aligned.
- `--every close+15m`: a market-calendar point (sessions only, early closes
  included). Named points come from the calendar, never from parsing a name.
- `--every bar`: an event from a `listen` of the same provider. The REST
  fetch is triggered by the websocket.

Hyphae called the event case "reactive acquisition" and kept it to one
provider and one feed lifecycle: "Cross-feed or cross-provider triggers are
orchestration concerns above `feed`". The same holds here: a trigger and
its fetch share a provider (and its credentials and rate limit); triggering
one provider from another's stream is two commands composed, not a flag.

**Rules every repeating fetch keeps** (Hyphae's poll card): one fetch in
flight; a trigger that fires during a fetch is coalesced into one next
fetch, never queued without bound; skipped triggers are counted and
reported; failures back off on `libs/net`'s bounded retry, and a poll that
gives up says so and stops; a trigger the source can't serve (no endpoint,
faster than its rate allows) is refused at the start with the reason.
Fetched records go out on the same output as the stream that triggered
them, each with its own reception time: never one time stamped across a
whole chain.

**Batch schedules stay outside.** Intraday cadence is `--every`, in the
process doing the fetching. Tonight's top-off is the deployment's scheduler
(0020; Nomad today) running `tt fetch`, so tinytrader keeps no batch
scheduler. A paid repeating fetch still needs a person: under 0019 it
prices, leaves the quote waiting and notifies, and buys nothing until
someone says yes. A standing approval (per dataset, a daily dollar cap,
expiring) would be its own decision; this one doesn't propose it.

### 3. Addresses: one input, one output

Every command has exactly one input and one output, each an address with a
scheme (Hyphae's `docs/src/cli.md` conventions; Hyphae itself still uses
bare loopback `IP:PORT`):

| | Flag | Address |
|---|---|---|
| Output | `--publish ADDR` | `tcp://127.0.0.1:7610`, `unix://$TT_SOCKETS/spy.sock`: a stream any number of subscribers join (through distribution) |
| | `--output -` | stdout: protocol bytes only; logs go to stderr |
| | `--write STORE` | the store service (`http://…:9120`): filed through its API, never written to a disk directly (0005) |
| Input | `--connect ADDR` | a published stream |
| | `--input -` | stdin |

The provider's own address, when it must be given, is `--provider-endpoint`;
credentials are never arguments (environment or a file, as today).
Commands run in the foreground and exit truthfully (0, 1 failed, 2 usage,
130 signal), so Bash and the scheduler compose them without hidden logic.

### 4. `tt`, and what it looks like

`apps/tt`, one binary, front end only: each command is a call to the
service that owns it (the relay, `databento-jobs`, `alpaca-history`, the
store, the console).

- **`tt` never dials a vendor.** A provider `fetch` or `listen` is always a
  request to the service that owns that provider's connection and key: the
  relay for live streams, `databento-jobs` and `alpaca-history` for
  history. Only those services hold the keys (0013's rule for Databento),
  `tt` runs from any tailnet machine, and Alpaca allows one stream per
  account. `tt listen … --publish` or `--output` re-publishes a relay
  subscription; it opens no provider connection.
- **The owning service runs `--every`.** A repeating or event-triggered
  fetch is a request to the service that owns the fetch (and, for an event
  trigger, the stream): it runs the trigger loop beside its connection and
  publishes the results on its output. `tt` only asks for it and reads it.
- **Anything long-lived is a service job.** A `--every` or `--publish`
  that should outlive a terminal runs as a service job in the deployment's
  scheduler (a Nomad service job today), with its limits and restarts
  there. tinytrader ships no supervisor, as it ships no batch scheduler.
- **Socket addresses** default to a per-user directory, `$TT_SOCKETS`
  (`$XDG_RUNTIME_DIR/tt` on Linux, `$TMPDIR/tt` on macOS, where `/run` is
  root's), mode 0700. Examples, in the shape Hyphae's scripts use:

```sh
# Historical, finite (REST). Paid: prints the quote and asks y/N.
tt fetch databento --dataset OPRA.PILLAR --schema cbbo-1m --symbols SPY.OPT \
  --start 2022-09-01 --end 2025-10-07 --hours 09:30-16:01 --write store
tt fetch alpaca --feed sip --schema cmbp-1 --symbols SPY,QQQ \
  --start 2026-10-05 --end 2026-10-23 --write store

# Live (websocket): a subscription to the relay that holds the provider's
# connection, re-published here for any number of subscribers.
tt listen databento --dataset OPRA.PILLAR --schema cmbp-1 --symbols SPY.OPT \
  --publish tcp://127.0.0.1:7600

# A repeating fetch on a timer.
tt fetch alpaca --schema chain --symbols SPY --every 1m \
  --publish unix://$TT_SOCKETS/spy-chain.sock

# A fetch on each event of a stream: SPY's chain at every minute bar's close.
tt listen alpaca --schema ohlcv-1m --symbols SPY \
  --every bar --fetch chain \
  --publish unix://$TT_SOCKETS/spy-bars-chain.sock

# The recorder: an ordinary subscriber, filing through the store.
tt record --connect unix://$TT_SOCKETS/spy-bars-chain.sock --write store

# Replay: the store as the source, same output as live.
tt listen store --dataset OPRA.PILLAR --schema cmbp-1 --symbols SPY.OPT \
  --start 2026-09-01 --end 2026-09-05 --publish tcp://127.0.0.1:7700
tt fetch store --dataset ALPACA.SIP --schema cmbp-1 --symbols SPY \
  --start 2026-10-02 --end 2026-10-02 --output - | my-strategy --input -

# Purchases.
tt quotes                       # waiting, with cost and time left
tt approve QUOTE | tt decline QUOTE
tt jobs --follow                # ends with each job's filing result
```

Exact flags follow the generated contract (section 5); these fix the shape:
source first, then what, then when (`--every`), then where (`--publish`,
`--output`, `--write`).

**Approval goes through the console, not a key.** `tt approve` sends the
console's own `POST /api/job-quotes/{id}/approve` with `confirm_usd`; the
console's write gate takes the login from its authenticating proxy
(`tailscale serve` sets `Tailscale-User-Login`) and adds the approval token,
exactly as for the button. So `tt` works from any machine on the tailnet
signed in as an allowed login, holds no secret, and every approval records
who gave it: the approver is the login the proxy sets, never a value `tt`
sends.

- `tt` approves only through the proxy's address (`TT_CONSOLE`: the
  console's name under `tailscale serve`, say), even on the console's own
  host. A
  loopback address has no proxy login, so the write gate refuses it, and
  `tt` says why instead of retrying.
- With no authenticating proxy (no allowed logins configured), the console
  refuses every write, approvals included, and `tt approve` refuses with
  that message. There is no fallback to a token alone.
- Where the console asks for its order token on approvals (as for a live
  order, unless `CONSOLE_LIVE_NEEDS_TOKEN` is `false`), `tt` asks for it at
  the prompt and keeps it for that run only.

The y/N is the confirmation 0019 asks for: it shows the exact price and
sends that price.

**A quote that expires is said so.** Quotes carry `expires_at`; `tt` and the
console's Jobs widget both show the time left, and an approval after
expiry offers to price it again (free) and asks again, never buys at a new
price unasked.

`tt` replaces the queue's `tt` that 0020 removes; it shares no code with it.

### 5. One contract, generated

The endpoints these verbs use are described once, by an OpenAPI document
generated from the axum handlers' types, and CI fails on a breaking change
to it. `tt`, `libs/store-client`, the console's widgets and research's
Python client are built against that document, not kept by hand. This
covers only the endpoints the verbs touch at first: `databento-jobs`
`/price`, `/quotes`, `/submit`; the store's `/catalog`, `/coverage`,
`/files`; the console's `/api/job-quotes`. Other services follow as they're
touched.

### 6. Layered libraries, with a stated dependency direction

Hyphae's crates were its best-reused part: each owned one concern, the
dependency direction was written down, and no library held process
concerns. tinytrader's `libs` (`alpaca`, `dbn`, `fleet`, `observe`,
`schedule`) start that; most provider and network logic is still inside
services' `main.rs`. The layers:

| Layer | Libraries | Depends on |
|---|---|---|
| Foundation | `dbn` (DBN, re-exported, plus tinytrader's extension records), `schedule` (clock, market calendar), `instruments` (symbology: OCC contracts, option roots, a symbol's dataset), `observe` | upstream crates and std only |
| Plumbing | `net` (below), `market` (only when two services need it: book state, gaps) | foundation |
| Venues | `venues/alpaca` (today's `libs/alpaca`), `venues/databento`, later `venues/coinbase`, `venues/kraken`: each provider's wire format to DBN, its pagination and its rate rules, once | foundation, plumbing |
| Clients | `store-client` and the clients generated from the contract (section 5) | foundation, plumbing |
| Front ends and services | `apps/tt`, `apps/tui`, the console, every service | any library |

**Rules.**

- A library never depends on a service or an app, and a layer never on a
  layer above it. CI checks this from `cargo metadata`.
- Libraries hold no process concerns: no argument parsing, environment or
  secret lookup, signal handling, exits, or listener and runtime startup.
  A service's `main` reads its configuration and passes values in. So the
  same code runs behind HTTP, a socket, a pipe or a test, and `fetch`,
  `--every` and `listen` reuse one provider mapping.
- A venue library is the only place a provider's wire format is known. A
  service that talks to Alpaca or Databento does it through it.

**Crypto records.** `libs/dbn` takes Hyphae's `dbn-ext` crypto records as
they are (`TradeWide`, `MboWide`, `Mbp10Wide`: wider fields for crypto's
fractional sizes; rtypes `0xD1` to `0xD3`). The crate depends only on
upstream `dbn` and std, and its `TradeConditions` (`0xD4`) is already
byte-identical to tinytrader's. Hyphae's crypto days are then readable here
unchanged.

**Not taken:** Hyphae's `message`, `transport` and `archive` crates (its
framed protocol and its archive), and `store` as a library: tinytrader's
store is a service, the only writer to the data disks (0005).

### 7. Shared plumbing in `libs`

- **`libs/net`:** one retry/backoff (doubling, capped, jittered, **bounded**:
  it ends with an error saying it gave up), one reconnect loop for streams,
  an HTTP client builder (timeouts, user agent), and a `Transport` seam
  (HTTP request/response, websocket stream) with one fake provider for
  tests, so a service's tests don't each build their own server.
- **`libs/store-client`:** the store's reads (`/catalog`, `/coverage`,
  `/files`, with the 503-and-dropped-connection retry `databento-jobs` has)
  and filing (`PUT /incoming`, `POST /incorporate`, never retried blindly),
  with the write token. `alpaca-history` and `databento-jobs` use it.
- **DBN encoding** moves to `libs/dbn` as services are touched (not in one
  sweep).

**The gateways come later.** The data services (relay, ticker,
alpaca-news, alpaca-history, databento-jobs) move onto `libs/net` first.
order-gateway's and alpaca-account's reconnects move in their own PR,
reviewed and deployed after a close, under the trading hold.

Services keep their own HTTP APIs and guards. Nothing here is a framework:
no service is required to have a given shape, only to stop copying these.

### 8. The store producer

The store producer is served by the relay, as a source beside its
providers: a `listen` (or `fetch`) of `store` sends the stored records for
the days asked, then
`ReplayCompleted`, then live if the request asked for it. The records sent
are the stored DBN, unchanged.

- **Order.** Files are merged k-way by each record's index time, as
  Databento orders them (`ts_recv` where the schema has it, else
  `ts_event`). Ties keep the order of the request's symbols, then each
  file's own order, so a replay is the same every time.
- **Pace.** As fast as the client reads, by default, with backpressure (it
  is lossless: it waits for its reader, never drops): the relay never
  buffers a day. (Hyphae specified pacing but never built it.) `speed=1` paces by index time (real time),
  `speed=N` N times faster. There is no simulated clock: a client that
  wants one keeps it from the record times.
- **Size and drives.** A `cmbp-1` day is up to about 28 GB, on drives that
  sleep (12–30 s for a first read). Files are streamed, never loaded whole;
  a read that stalls over 5 s sends a waiting message, and one that fails
  sends a gap, never a timeout that drops the client.
- **Holes.** A day the store lacks is a gap message, never a silent skip.
  A partial day sends what's held and a gap message naming what isn't (its
  coverage pieces: the contracts and hours missing).

**As built (step 5a, `libs/producer`).** The producer is a library first,
so sessions (0022) and, later, the relay serve the same code. It reads the
store's API, or a local copy (a finished `databento-jobs` cache job, or a
folder in the store's layout) behind the same contract. The gap is a
`SystemMsg` `store-gap day=… reason=…` and a partial day is announced by
`store-partial day=…`; the coverage pieces stay in `/coverage`, since a
`SystemMsg` holds 303 characters. Batch files carry no definitions, so each
day starts with a `SymbolMappingMsg` per instrument its files map, stamped
at the day's start, with `stype_out_symbol` the raw symbol, as a live
session sends them. Each file's SHA-256 is taken while it streams and
checked where one is stated: the cache job's manifest, the store's
`/coverage` for a day in one file, or an earlier run's pins, which must
also name the same files (a gap then must be a gap now). A local file is
checked before its records are released; a store file at the end of its
day, so a run must count a failed read as a failed run. Each file
contributes only the asked schema's records inside the day's window (a
range file is listed on each of its days; sidecars are listed beside
them), and a file whose header names another schema or symbol is pinned
but not read. A day whose copies differ (`conflict`) is refused; of
several copies, the day's own volume's is read.

### 9. Warm-up: history, then live, in one join

A strategy that needs a lookback (indicator windows, today's high and low,
a book) gets it from the join that gives it live data: `listen` with
`start`. It reads history from `start`, then `ReplayCompleted`, then live,
in one stream, and never stitches sources itself. The relay assembles the
history from three tiers:

| Part of the lookback | Comes from |
|---|---|
| Days the store holds | the store producer (section 8) |
| Today, before the join | the provider's intraday replay where it has one: Alpaca's history; for OPRA, Databento's intraday replay of `cbbo-1s`, `cbbo-1m`, `tcbbo` or `trades` (it won't replay `cmbp-1`). At full `cmbp-1` fidelity, only the relay's own retention of today or the recorder's hot copy, once capture exists. |
| From the join on | live |

**One cut, no seam errors.** The relay fixes a cut point at the join:
history is every record before it, live every record after it, so nothing
is sent twice or missed at the hand-over, and a tier that can't supply its
part is an explicit gap, never a skip. A client that reconnects mid-session
resumes at its own last point (section 10), not at a fresh `start`.

**For OPRA today, a coarser schema closes the gap.** Most lookbacks want
minute or second quotes and trades, not every book update. A join that asks
for `cmbp-1` live with a warm-up in `cbbo-1m` (or `cbbo-1s`, `tcbbo`) gets
today's part from Databento's intraday replay. The relay already does this
for one contract (`/session`); the warm-up asks for the strategy's symbols,
or the whole parent. The history's metadata says which schema it is, so a
strategy never mistakes minute quotes for the book. Before it's built: check
that an intraday replay is covered by the live subscription. If it would be
charged, it is a priced request under 0019, like any purchase.

**Full fidelity before the join** (`cmbp-1` from the open) needs the relay
to keep today itself, in memory or on a local hot disk (a `cmbp-1` day is up
to about 28 GB). That is sized and decided with 0013's capture, since the
recorder's hot copy serves the same need.

**A cheaper warm-up** for lookbacks that are bars: `fetch` the bars, then
`listen` live, accepting a seam at a bar boundary. The one join stays the
default because it is exact.

### 10. Resume and loss accounting, in DBN

Hyphae's framed protocol wrapped the stream in frames with a stream ID, an
epoch per producer restart, a contiguous sequence, resumable subscriptions
and acknowledgement windows. That gives exact resume after a disconnect,
and exact accounting of what a subscriber missed, at the cost of a wire
format no DBN tool reads.

tinytrader needs those guarantees, not that format. They are what makes
the warm-up's cut exact (section 9) and a mid-session reconnect resume
where it left off, and later what lets the recorder (step 7, 0013) prove a
capture whole across a relay restart. So they go in-band, in standard DBN: the relay sends an epoch at
each start and a running record count in periodic `SystemMsg` markers, and
the join takes `after: {epoch, count}` to resume from what it still holds,
or says the gap. A DBN tool skips the markers; strategies and the recorder check them.
Built with the store producer's hand-over to live (step 5c).

## Steps and budgets

Each step is its own PR, built after hours. Production / tests lines; stop
and report before going 30% over.

**First deliverable: the approval command line.** Steps 0–3 stand on their
own and don't wait for any of the rest.

| Step | Prod | Tests |
|---|---|---|
| 0. Generated OpenAPI for the endpoints steps 2–3 use, and the CI check | 300 | 100 |
| 1. `libs/net`; the data services' backoff loops moved onto it (net negative) | 400 | 300 |
| 2. `libs/store-client`; `alpaca-history` and `databento-jobs` on it | 250 | 200 |
| 3. `tt fetch` (historical, `--write store`), `quotes`, `approve`, `decline`, `jobs`; quote `expires_at` in `tt` and the console | 450 | 250 |
| **First deliverable** | **1,400** | **850** |

**Then the Hyphae model,** each step accepted on its own:

| Step | Prod | Tests |
|---|---|---|
| 1b. order-gateway and alpaca-account onto `libs/net` (own PR, after a close) | 150 | 150 |
| 4. Addresses: `--publish`/`--connect` with `tcp://` and `unix://`, `--output -`/`--input -`, on the relay and `tt` | 300 | 250 |
| 5a. The store producer: one symbol-day, direct and lossless, holes as gaps | 450 | 300 |
| 5b. The k-way merge across files, pace, the shared (hub) path, and `tt listen store` | 450 | 300 |
| 5c. Warm-up: the cut, the hand-over from store to live, epoch and count markers, `after` resume | 400 | 400 |
| 6a. `--every` time and calendar triggers, in the services that own the fetch | 250 | 200 |
| 6b. `--every` stream-event triggers on the relay (one provider), coalescing | 350 | 300 |
| 7. `tt record`: the recorder as a subscriber filing through the store (with 0013) | (scoped with 0013) | |
| 9a. Hyphae's crypto records into `libs/dbn` (moved, byte-compatible, with their tests) | ~1,300 moved | ~700 moved |
| 9b. The dependency-rule CI check; `libs/alpaca` to `libs/venues/alpaca`; `libs/instruments` from the services' OCC and root parsing | 350 | 250 |
| 9c. `libs/venues/databento` from the relay's and `databento-jobs`' mapping (net negative) | 300 | 200 |
| 8. Docs: the roles, verbs and addresses in each API page | (docs) | |
| **The rest, before 7** | **2,350** | **1,900** |

Order: 0 first for the endpoints 2 and 3 use; then 1, 2 and 3 in any
order. After them: 4 before 5b and 6b; 5a needs 2, 5b needs 5a, 5c needs 5b; 6a needs
1; 6b needs 4 and 6a. 9a stands alone; 9b before 9c, and 9c before 6b. Each step reports the lines it removes beside these.

## Consequences

- Approving a purchase is `tt fetch …` then `y`, from any tailnet machine,
  with the same guards and ledger as the console.
- A new feed gets retries, reconnects, store filing and a fake provider for
  free, and the review of a feed is of what's specific to it.
- A strategy listens to held days through the relay, as it listens live; OPRA
  `cmbp-1` days held in the store can be replayed although Databento won't.
- A chain snapshot on every bar close, a top-off every night, and a capture
  with a recorder are compositions of the same commands, not new services.
- `libs/net` and `libs/store-client` are contracts between services; change
  them additively.
- **The name stays `tt`.** It was the queue's command (removed by 0020),
  and research imports a Python package `tt`; a binary and a Python package
  don't collide on `PATH`.
- Not doing: Hyphae's slot transactions and page evidence (0014's reasons
  stand), its HTRN/MDS1 framed transport (the relay's DBN-over-TCP join stays
  the stream protocol), a connector framework, or a batch scheduler.

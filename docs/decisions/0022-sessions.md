# 0022: Sessions, one entry point for backtest, paper and live

- **Status:** Proposed
- **Date:** 2026-10-26
- **Builds on:** [0009](0009-trading-runtime.md) (the trading runtime: one
  total order, virtual time, run provenance), [0017](0017-client-contracts.md)
  (client contracts), [0021](0021-acquisition-verbs-and-shared-plumbing.md)
  (producers, the store producer, warm-up, `tt`), and the
  [order gateway](../api/order-gateway.md) with its simulated broker

## Context

A strategy is a client of two protocols: market data (`tinytrader.relay/1`,
DBN) and orders (`tinytrader.gateway/1`, one process per account). The
gateway's simulated broker (`--broker sim`) answers the same protocol as an
Alpaca account, fills against real quotes, and in client-clocked replay
(`advance{to_ts}`) is deterministic. So the pieces of a backtest exist, but
not the whole:

- **Nothing joins them.** A backtest means starting a replay and a
  simulator separately, pointing both at the same data, and keeping their
  clocks in step by hand. A strategy can't move from backtest to paper to
  live by changing one thing.
- **The simulator can't read the store.** It takes an uncompressed DBN file
  or a stream, one source.
- **One source, one venue.** A cross-sectional strategy (seven roots' chains
  at once) or a multi-venue one (futures at one venue, options at another)
  needs several sources merged on one clock and several simulated venues,
  each with its own instruments, calendar, fees and latency.
- **No run, end to end.** Choosing inputs, checking they're held, pinning
  them, running, keeping the results and reproducing them is by hand.
- **Multi-day options aren't simulated.** The simulator expires day orders
  at the close, but options don't expire, settle or get assigned.

0009 designs an in-process runtime (actors, virtual time, the `(ts, class,
seq)` order, manifests). This decision gives strategies **outside** that
runtime, in any language and any repository, the same semantics over the
two protocols they already speak, and has the runtime use the same pieces.

## Decision

### 1. A session is the one thing that changes

A strategy connects to a **session**: one or more market data streams and
one or more order gateways, on one clock. It runs the same code against any
session; only the session's mode says what's behind the addresses.

| Mode | Data | Orders | Clock |
|---|---|---|---|
| `backtest` | the store producer, from pinned days | simulated venues | the session's (virtual) |
| `paper` | the relays (live) | paper accounts, or simulated venues on live quotes | the wall |
| `live` | the relays | live accounts | the wall |

A session announces itself the way every Hyphae-style command does: one
line, `READY session=<id> mode=<m> data=<addr>[,<addr>…] orders=<name>=<addr>[,…]`,
and the same as JSON at `GET /sessions/{id}`. A strategy reads its addresses
from there (or from `TT_SESSION`), never from its own configuration, so
nothing in it names a mode.

`backtest` is the new part, served by a **session service**; `paper` and
`live` are a manifest pointing at the relays and gateways that already run,
plus the run record (section 6).

### 2. One clock, and lockstep

In a backtest the session owns time, so the strategy, every simulated venue
and every data stream agree on what has happened. The order of everything
is 0009's: one key `(ts, class, seq)`; market records by their index time
(`ts_recv`, or `ts_event` plus the source's declared latency offset where
the record carries `F_BAD_TS_RECV`); then venue reports, then timers.

The strategy says how it keeps up:

- **`lockstep`** (the default; deterministic). The session sends the
  records of one timestamp (or one batch the strategy chose: per record,
  per timestamp, per N ms of market time) and waits for the strategy's
  `ready{as_of}` on the orders connection before sending more. Orders sent
  before `ready` are stamped at `as_of` plus the strategy's declared
  latency (a constant per session, 0 by default), so a slow strategy is
  modelled, not timed. The simulator's existing `advance{to_ts}` becomes
  this: the session advances every venue together.
- **`paced`** (`speed=1` market time as wall time, or `speed=N`). The
  session doesn't wait; a strategy's real latency counts. Not reproducible,
  and the run says so.
- **`free`**: as fast as the session can read. For strategies that only
  observe (research features, recording).

A strategy that stops answering in lockstep stops the run after a stated
timeout, with the run marked incomplete, never silently advanced.

### 3. Many sources, many venues

A session names its sources and venues:

```text
sources:
  opra:  store OPRA.PILLAR cmbp-1 SPY.OPT,SPXW.OPT        latency_offset 0
  glbx:  store GLBX.MDP3  mbp-1  ES.FUT                   latency_offset 0
  iex:   store ALPACA.IEX mbp-1  SPY                      latency_offset 2ms (F_BAD_TS_RECV)
venues:
  options: sim  quotes=opra  calendar=XNYS  fees=opra-retail  latency=5ms
  futures: sim  quotes=glbx  calendar=CMES  fees=cme-retail   latency=3ms
```

- **One merge.** Every source is a store producer stream (0021), already in
  time order; the session merges them lazily (one heap entry per source, as
  0009 has it), so a cross-section of chains costs one comparison per
  record.
- **The strategy chooses its streams.** It reads the merge as one stream,
  or each source as its own stream, each in order and all released by the
  same lockstep. Either way no stream runs ahead of the clock.
- **Each venue is its own simulated account**, a gateway with its own
  address, instruments (only those its quote sources define), calendar,
  fees, latency and limits, as accounts at different brokers are. A
  strategy holding futures at one and options at another sees two gateways,
  as it would live. Margin across venues is not modelled: each account is
  on its own, as it is unless a broker offers portfolio margin.
- **Venues are defined by profile, not code.** A new venue for an existing
  instrument class is a profile; a new instrument class (futures expiry
  and daily settlement, say) is simulator work.

### 4. Cross-sectional strategies

- **Instrument ids change daily** in OPRA. The session sends each day's
  definitions before that day's records, and the simulator and strategy key
  by `raw_symbol` (the simulator already does). A contract listed or expired
  mid-run arrives as a definition or stops appearing; nothing is carried over.
- **Universes are filters on the store.** A source names parents
  (`SPY.OPT`), lists, or rules (expiries, strike bands), as `databento-jobs`
  prices them. The store producer reads only those contracts' records, so a
  seven-root cross-section in `cbbo-1m` doesn't read every contract's book.
- **Coverage first.** A session refuses to start over days its sources don't
  hold in full for its universe, unless the strategy accepts named holes;
  the holes are then gap messages at their times (0021 section 9).
- **Schema per need.** A cross-section can read `cbbo-1m` for the chain and
  `cmbp-1` for the few contracts it trades, as two sources.
- **Mixed schemas are said so.** A stream carrying more than one schema
  (two sources merged, or a `cbbo-1m` warm-up before `cmbp-1` live, 0021
  section 9) has `schema` unset in its DBN metadata (mixed), and each
  record's rtype says what it is, so a reader that trusts the header never
  misdecodes. `ReplayCompleted` marks the hand-over from history to live.

### 5. The simulator grows for multi-day options

- **Expiry, settlement and assignment.** At each expiry: cash-settled
  options (SPX, XSP) settle at the settlement value (the AM special
  opening quotation, or the PM close) from a source the session names; a
  missing value fails that settlement loudly rather than guessing;
  physically settled (SPY) in-the-money long options exercise and short
  ones are assigned into the underlying, by the OCC's threshold unless the
  profile says otherwise. These are broker events on the account, as they
  would be live.
- **Reads the store**, through the session (one or more sources), not only
  a file or a stream.
- **Limits stated in the run.** Fills are at the touch, with optional
  displayed-size caps; no queue position or market impact. The pin names
  the model version, so a result says what it assumed.

### 6. The run, end to end

`tt` drives it, as a front end to the session service:

```sh
# 1. Plan: what the inputs are, whether the store holds them, what's missing.
tt session plan sessions/spy-es.toml --start 2026-09-01 --end 2026-09-30
#    → per source: days held, partial, missing; for missing days, a free
#      quote from databento-jobs to buy them (approved as any purchase)

# 2. Run: start the session, then the strategy reads TT_SESSION.
tt session run sessions/spy-es.toml --start 2026-09-01 --end 2026-09-30 \
  --warmup 5d --trade-from 2026-09-08 --seed 7 &
my-strategy                       # any language; reads TT_SESSION

# 3. Results, and reproducing one.
tt runs                            # every run: mode, inputs, status, audit hash
tt runs show RUN                   # orders, fills, positions, P&L, the pins
tt session run --manifest RUN      # the same inputs and pins again
```

- **Warm-up.** `--warmup` starts the data before `--trade-from`; the venues
  refuse orders until then (`rejected{code:"warmup"}`), so a lookback fills
  without trading on it.
- **The manifest** pins every input day by the store's SHA-256, the
  universe, the venues' profiles and the simulator's model version, the
  strategy's declared identity (a revision it sends in `hello`), the
  lockstep batch and latency, and the seed.
- **The record** of a run is the gateways' journals (hash-chained already),
  the session's audit hash over what it delivered and what it was sent, and
  whatever the strategy reports through the session (named series, opaque
  to tinytrader). It is filed through the store as a run, immutable like
  data (0009).
- **Same manifest, same audit hash.** A test runs a session twice and
  compares the hashes.
- **Sweeps** are many sessions, each a batch job in the deployment's
  scheduler (0020). The store producer serves them all; a session reads the
  days it needs once, in order, so a sweep is bounded by the drives' read
  rate, which the plan reports.

### 7. Clients outside the repository

Strategies live elsewhere, in any language. Each protocol they use is a
stable, versioned contract (0017), and each gets a small client library
kept beside it, with the same tests:

- **Python** (`tinytrader-client`): session discovery, the relay join and
  DBN decoding (the official `databento-dbn` package), the gateway protocol,
  lockstep `ready`.
- **Rust**: the same, on `libs/dbn` and the layered libraries (0021).

The session's own HTTP endpoints are in the generated contract (0021
section 5). The in-process runtime of 0009 uses the same session
libraries (the merge, the clock, the simulated venues), so an actor in the
runtime and a client over TCP see the same order and the same fills.

## Steps and budgets

Each step is its own PR, built after hours. Production / tests lines; stop
and report before going 30% over. Steps 1 and 2 need 0021's store producer
(5a, 5b) and its warm-up cut (5c).

| Step | Prod | Tests |
|---|---|---|
| 1. The session service: one source, one simulated venue, lockstep, `READY`; the simulator reads through it | 700 | 600 |
| 2. Many sources (the merge, per-source latency offsets) and many venues (profiles, one gateway each) | 500 | 450 |
| 3. Universe filters on the store producer; daily definitions; coverage refusal and accepted holes | 400 | 350 |
| 4. The run: `tt session plan/run`, `tt runs`, the manifest, the audit hash, filing the record; warm-up and `trade-from` | 600 | 400 |
| 5. Option expiry, settlement and assignment in the simulator | 450 | 450 |
| 6. Python and Rust client libraries, with the strategy tutorial on them | 600 | 400 |
| 7. `paper` and `live` sessions: manifests over the running relays and gateways, the run record | 250 | 200 |
| **Total** | **3,500** | **2,850** |

Order: 1 → 2 → 3 → 4; 5 and 6 after 1; 7 after 4.

## Consequences

- A strategy moves from backtest to paper to live by being handed a
  different session; it never names a mode, a file or a host.
- Cross-sectional and multi-venue strategies are backtested on one clock,
  each venue as its own account, so a strategy can't see one venue's
  future while trading another.
- A result can be reproduced from its manifest, and says which fill model
  and which inputs it assumed.
- The session service is new, and a stable interface once step 1 lands.
  The simulator's `advance{to_ts}` stays for clients that use it directly.
- Not doing now: queue-position or impact fill models, margin across
  venues, futures expiry (a later simulator step), and a strategy framework
  (0009 rule 2: no opinions on decision-making).

# 0024: The crate layout, and moving the services onto it

- **Status:** Proposed
- **Date:** 2026-10-26
- **Builds on:** [0003](0003-front-ends.md) (one data layer),
  [0021](0021-acquisition-verbs-and-shared-plumbing.md) (section 6, layered
  libraries; its steps 1, 2 and 9), [0023](0023-composable-interfaces.md)
  (libraries carry the logic; the console is a client)

## Context

Four read-only audits (2026-10-26) of `main` at 3f39ba0 against Hyphae's
crates and Reactor's layout found tinytrader organised by service rather
than by concern. The numbers:

- **Big `main.rs` files.** The store's is 1,956 lines of code (plus 889 of
  tests), `databento-jobs`' 2,813, the console's 1,919. Each holds
  configuration, state, routes, background loops, clients of other
  services and domain logic together. `order-gateway` (421 lines over a
  library with one `Broker` trait) is the shape to copy, though its
  `session.rs` is 3,269 lines.
- **Copied helpers.** Civil-date and calendar maths about 18 times (while
  `libs/schedule` keeps its own private); OCC symbol parsing 9 times, in two
  forms with different validation; DBN header reading 8 times (the store
  and `databento-jobs` depend on `dbn` directly, not `tinytrader-dbn`);
  SHA-256 and fsync helpers 4 times; JSON error replies in several
  services and 33 inline in the console; three token comparisons.
- **Alpaca plumbing in six services.** The DBN mapping is good and in one
  place (`libs/alpaca`); the HTTP and websocket around it isn't: 8 copies of
  the auth headers, 13 page loops (two unbounded), three rate-limit designs
  and three services with none, 4 websocket handshakes over two protocols,
  4 reconnect loops. `alpaca-history`'s page loop is the safest anywhere,
  Hyphae's included.
- **Service lifecycle.** The same bind-and-serve block in 11 services; 24
  HTTP client set-ups; secret-or-exit handling repeated; three copies of
  "idle while capture is live" and two of the calendar fetch; 37 spawned
  tasks, none supervised; only 2 of 12 services drain on SIGTERM.
- **The relay** mixes fan-out with bars, quotes, tape and reference state
  in one 1,195-line `hub.rs`, and each source owns its reconnect loop.
  Hyphae's relay is better organised (one file per concern, byte-bounded
  per-client queues, a recorder, and catch-up from its own recording with
  an exact cut); tinytrader's source trait, with optional capabilities and
  a refusal when one is missing, is the better interface.
- **The console's JavaScript** is 9,853 hand-written lines (plus 4,171 of
  bundled libraries). About two thirds is rendering, as it should be; about
  1,780 lines are data logic: Black-Scholes, payoffs and chance of profit,
  gamma exposure and expected moves, liquidation planning and order
  pricing, a trade-suggestion engine, return statistics. Options maths and
  order rules don't belong in a browser. The terminal UI and the page each
  derive the same views (accounts, sessions, coverage, storage) and even
  the status rule separately, so 0003's one data layer holds only for
  topology and snapshots.

**Two bugs found:**

1. `alpaca-data` turns an upstream `429` into a `502` (`main.rs:259`), so
   its own 429 handling never sees Alpaca's; a real rate limit ends the
   loop and the symbol is marked scanned.
2. The relay's Alpaca `minute_bars` and `pages` loops have no bound and no
   repeated-token check (`relay/src/alpaca.rs:521, 571`).

## Decision

### 1. Tiers

0021 section 6's layers, completed:

| Tier | Crates | Depends on | Holds |
|---|---|---|---|
| Foundation | `dbn` (records, extensions, header reading, the merge), `schedule` (clock, calendar, civil dates: public), `instruments` (OCC contracts both forms, option parents, a symbol's dataset), `options` (pricing, greeks, payoffs, implied moves), `observe` | upstream and std | pure functions and types; no I/O |
| Plumbing | `net` (retry, backoff, reconnect, the transport seam, a fake provider), `fanout` (byte-bounded fan-out, taps, the recorder hook), `relay-proto` (the join, markers, versioned) | foundation | mechanisms, no provider knowledge |
| Venues | `venues/alpaca`, `venues/databento`, later `coinbase`, `kraken` | foundation, plumbing | each provider's wire format, auth, pagination, rate rules, streams and DBN mapping: the only place it is known |
| Clients | `store-client`, generated clients (0021 section 5), `fleet` (and its typed views) | the above | how to talk to tinytrader's own services |
| Runtime | `service` (the service skeleton) | the above | process lifecycle, for services only |
| Services and apps | every service, `tt`, the TUI, the console | any | configuration, wiring, an HTTP or TCP surface |

`libs/README.md` says libraries hold "no process lifecycle"; that stays
true for every tier but runtime, which exists to hold it, once.

### 2. Services become thin

A service's `main.rs` reads configuration and wires libraries together, in
the shape of `order-gateway` and Reactor's services: `main.rs` (tens of
lines), `app.rs` or `state.rs`, `http/*.rs` per resource, background loops
in their own modules, tests beside them. `libs/service` gives every one:

- bind and serve, with draining on SIGTERM and ctrl-c (all of them);
- the rustls install, an HTTP client with timeouts, secret-or-exit;
- one supervised loop for a source or a background task (`supervise`, with
  `libs/net`'s backoff and the market hours), and a `CaptureGate` for
  "idle while capture is live" with the calendar fetch.

Secrets resolve one way everywhere: `NAME_FILE`, then `NAME`
(`order-gateway` reads them the other way round today, and moves).

### 3. Each venue once

`venues/alpaca` (I/O through `libs/net`'s transport):

- `wire` (one set of row and envelope types), `decode`, `dbn` (from
  `libs/alpaca`: one `instrument_id`, the metadata, the multi-symbol
  interleave), `rest` (endpoints, credentials, request builders), `paging`
  (one bounded loop with `alpaca-history`'s checks: empty, malformed or
  repeated tokens, rows out of order or out of day), `rate` (one status
  classifier, one rate-limit header parser, one pace), `stream` (both
  handshakes as a pure state machine, the frame decoder), `trading`
  (moved from `order-gateway`).
- Taken from Hyphae as is: its page-token validation and tests, the status
  classifier, the paced and budgeted transport shapes, the fake-clock and
  canned-transport tests. Not taken: its adaptive worker scaling, page
  evidence and manifests, reading a `403` as an empty day, and pagination
  outside the venue.

`venues/databento`: the API wrappers, cost and availability, the job-state
mapping, and request planning and "what's had" as pure functions over data
passed in (holdings, jobs, purchases). The ledger and the daily conditions
task stay in `databento-jobs`.

### 4. The relay: a fan-out core and taps

- `libs/fanout`: the hub's core (serve, the cut, upstream state, the
  roster, slow-reader diagnostics), with per-client queues bounded in
  bytes, a `Tap` hook and a recorder hook.
- The relay keeps its sources and becomes taps for bars, quotes, tape and
  the session; `Source::run` becomes one session, under `libs/service`'s
  supervised loop.
- Hyphae's recorder and catch-up are ported: catch-up from the relay's own
  recording with an exact cut, so Databento sources get catch-up and
  `Source::catch_up` becomes the fallback. This is also 0021 section 9's
  today tier and the recorder's hot copy, sized with 0013.

### 5. The console renders

Logic leaves the page for the services that own the data:

| Logic | To |
|---|---|
| Black-Scholes, payoffs, breakevens, chance of profit, delta; gamma exposure, walls, expected moves | `libs/options`, served by `alpaca-data` |
| Liquidation planning, order pricing, closing ratios | the console's gateways module, as a plan-and-price endpoint |
| Trade suggestions; trend and observation rules; return statistics; earnings implied jumps | the services that serve those streams |
| Minute-bar aggregation | `alpaca-data`'s `/bars?interval=` (already there) |
| Status, sessions, releases, coverage, storage views | `libs/fleet`'s typed views, used by the TUI and the page alike |
| Price alerts (per tab, in browser storage) | the console server |

What stays in the page: the workspace, charts and layout, forms, per-device
settings, formatting and fetching. About 1,200 to 1,500 lines of
JavaScript go.

### 6. Not taken

Hyphae's slot transactions, page evidence and framed transport; Reactor's
envelope and topic machinery and its DBN crate; the store as a library
(0005); a connector framework.

## Steps and budgets

Each step is its own PR, built after hours, accepted on its own. Lines
are production / tests added, then removed; a move counts once. Stop and
report before going 30% over.

| Step | Adds | Removes | Notes |
|---|---|---|---|
| **R0.** The two bugs: `alpaca-data`'s 429, the relay's unbounded pages | 40 / 60 | – | first, small |
| **R1.** `libs/schedule`'s civil dates public; the copies deleted (not `order-gateway`'s yet) | 20 / 30 | ~250 | pure |
| **R2.** `libs/instruments`: both OCC forms, option parents; tests over today's behaviour first | 250 / 250 | ~300 | |
| **R3.** `libs/service`: serve and drain, clients, secrets, `supervise`, `CaptureGate`; services moved onto it | 300 / 200 | ~500 | every service drains on SIGTERM |
| **R4.** The store's `main.rs` split into modules (a pure move) | moves ~1,700 | – | no logic change |
| **R5.** `venues/databento` (0021 step 9c); `databento-jobs` split | moves ~1,400 | ~150 | the purchase path: its guard tests must pass unchanged |
| **R6.** `venues/alpaca` (0021 step 9b); the six services onto it | moves ~2,700 | ~620 | |
| **R7.** `libs/fanout`, `relay-proto`; the relay as sources and taps | 700 / 500 | ~600 | |
| **R8.** The recorder and catch-up, ported from Hyphae | ~1,000 / ~800 | – | with 0013's capture sizing |
| **R9.** `libs/options` and the console's analytics moved into the services | ~900 / ~600 | ~1,200 JS | |
| **R10.** `libs/fleet` typed views for both front ends | 250 / 150 | ~150 JS, ~200 Rust | |
| **R11.** DBN header reading and the merge into `libs/dbn` | moves ~500 | ~250 | the store producer (0021 step 5) needs the merge |
| **R12.** `order-gateway` onto the shared crates; `session.rs` split by phase | moves ~2,000 | ~400 | under the trading hold, after a close |

0021's steps 1 (`libs/net`) and 2 (`libs/store-client`) come before R3, R5
and R6; R5 and R6 are 0021's 9c and 9b. Order: R0, R1, R2 any time; then
R3; R4 and R5 any order; R6 after R3; R7 after R3; R8 after R7; R9 and R10
any time; R11 before 0021's store producer; R12 last.

## Consequences

- Each concern is written once, in a crate that says what it excludes;
  a new service is a few dozen lines of wiring.
- Every service drains on SIGTERM and supervises its loops the same way.
- Options maths and order rules are in Rust, tested, and the same for the
  page, `tt` and strategies.
- The relay gains catch-up for every source, from its own recording.
- The work is large but each step stands alone; nothing is a rewrite, and
  each PR reports its lines removed.
- How services are deployed is a separate decision; this one changes code,
  not placement. Today they run under Compose and launchd, with Nomad for
  batch (tinytrader-deploy's 0014); the deployment's plan is to move the
  long-lived services onto Nomad service jobs, which would own restarts,
  health, environment and secrets, and placement. `libs/service` then keeps
  only what must be in-process: draining, supervised reconnects, and
  holding off while capture is live. One overlap is used now: finite work that waits for the
  close or for capture to go idle (`alpaca-history`'s backfills,
  `databento-jobs`' daily conditions) can be a batch job behind Nomad's
  start gate, and `CaptureGate` is then only for what a running service
  must hold off on.

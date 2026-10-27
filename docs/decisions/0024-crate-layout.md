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
  tasks, none supervised. Of the 12 services none drains on SIGTERM: the
  console drains its HTTP on ctrl-c only (`console/src/main.rs:393`); the
  relay and `ticker` stop on SIGTERM without draining
  (`relay/src/main.rs:459`, `ticker/src/main.rs:394`); `order-gateway`
  stops its session cleanly on SIGTERM (`order-gateway/src/main.rs:340`);
  the other eight handle no signal. The image runs each service as PID 1
  (`Dockerfile`'s `ENTRYPOINT ["/app"]`), where an unhandled SIGTERM is
  ignored, so `docker stop` waits its 10 s and kills.
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

Both are fixed by R0 (#63); the lines are at 3f39ba0.

1. `alpaca-data` turns an upstream `429` into a `502` (`main.rs:264`), so
   its own 429 handling never sees Alpaca's; a real rate limit ends the
   loop and the symbol is marked scanned.
2. The relay's Alpaca `minute_bars` and `pages` loops have no bound and no
   repeated-token check (`relay/src/alpaca.rs:383, 561`).

## Decision

### 1. Tiers

0021 section 6's layers, completed:

| Tier | Crates | Depends on | Holds |
|---|---|---|---|
| Foundation | `dbn` (records, extensions, header reading, the merge), `schedule` (clock, calendar, civil dates: public), `instruments` (OCC contracts both forms, option parents, a symbol's dataset), `options` (pricing, greeks, payoffs, implied moves) | upstream and std | pure functions and types; no I/O |
| Plumbing | `net` (retry, backoff, reconnect, the transport seam, a fake provider), `fanout` (byte-bounded fan-out, taps, the recorder hook), `relay-proto` (the join, markers, versioned) | foundation | mechanisms, no provider knowledge |
| Venues | `venues/alpaca`, `venues/databento`, later `coinbase`, `kraken` | foundation, plumbing | each provider's wire format, auth, pagination, rate rules, streams and DBN mapping: the only place it is known |
| Clients | `store-client`, generated clients (0021 section 5), `fleet` (and its typed views) | the above | how to talk to tinytrader's own services |
| Runtime | `service` (the service skeleton), `observe` (the health, metrics and describe routes, the write guards; its secret and listen-address helpers move to `service`) | the above | process lifecycle and the HTTP surface every service shares, for services only |
| Services and apps | every service, `tt`, the TUI, the console | any | configuration, wiring, an HTTP or TCP surface |

`libs/README.md` says libraries hold "no process lifecycle"; that stays
true for every tier but runtime, which exists to hold it, once. `observe`
is runtime, not foundation: it serves axum routes and reads the
environment.

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
(`order-gateway` reads them the other way round today, and moves in R12).

`order-gateway` is not moved by R3 or R6: it is synchronous (threads and
`signal-hook`, no tokio), already stops cleanly on SIGTERM, and is under
the trading hold. It moves onto `libs/service` and `venues/alpaca` in R12.

Until R3 lands, `init: true` on each service in the Compose files is the
stopgap: an init process is PID 1 and forwards SIGTERM, so a service that
handles no signal exits at once instead of after `docker stop`'s 10 s.
That is a prompt exit, not a drain.

### 3. Each venue once

`venues/alpaca` (I/O through `libs/net`'s transport):

- `wire` (one set of row and envelope types), `decode`, `dbn` (from
  `libs/alpaca`: one `instrument_id`, the metadata, the multi-symbol
  interleave), `rest` (endpoints, credentials, request builders), `paging`
  (one bounded loop with `alpaca-history`'s checks: empty, malformed or
  repeated tokens, rows out of order or out of day), `rate` (one status
  classifier, one rate-limit header parser, one pace), `stream` (both
  handshakes as a pure state machine, the frame decoder), `trading`
  (moved from `order-gateway` in R12, not R6).
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

### 6. `sync`: topping off, as sugar over `fetch`

Hyphae's `feed sync` kept a dataset current: a finite, bounded `fetch` of
only what the store doesn't hold yet. tinytrader does this in pieces today
(`alpaca-history`'s `HISTORY_TOPOFF` evening run; `databento-jobs`' "buy
only what's missing"). It becomes one verb, the same for every provider:

```sh
tt sync alpaca --feed sip --schema cmbp-1 --symbols SPY,QQQ --from 2026-09-28 --write store
tt sync databento --dataset OPRA.PILLAR --schema statistics --symbols SPY.OPT --from 2025-10-01 --write store
tt sync alpaca --feed iex --schema mbp-1 --symbols SPY --latest --write store   # from the newest day held to today
```

- **What it fetches:** every session day from `--from` through the latest
  complete day (its close plus 15 minutes, by the market calendar) that the
  store doesn't hold in full, newest first, bounded by `--max-days`
  (default 10, every source), `--max-usd` (paid sources only) and the
  provider's request budget. Days held are skipped by the store's
  coverage, not by a local record.
- **`--latest`** instead of `--from`: start the day after the newest day
  the store holds for that dataset, symbol and schema, and fill through the
  latest complete day, newest first. Holes before that newest day are left
  to an explicit `--from` run.
  - **If the store holds nothing**, `--latest` starts at the provider's
    earliest day for the symbol (probed once; Hyphae's `--start earliest`),
    and the bounds still hold: a run takes the newest `--max-days` of it,
    and later runs work back. Empty storage never buys on its own (0019).
- **Paid and included sources.** A source is paid when its requests are
  charged one by one: a Databento quote whose `cost_usd` is not 0 (its
  `needs_approval`). Alpaca's feeds, SIP included, are a subscription:
  included, not paid per request, so they are bounded by `--max-days` and
  the rate limit and need no quote. A Databento request its plan covers
  (cost 0) is included too.
- **Pricing a paid source, in chunks.** The days are priced newest first in
  chunks no longer than `/price` takes (1,500 days, `MAX_DAYS` in
  `databento-jobs/src/main.rs:119`), and pricing stops at `--max-days` or
  when the next chunk would pass `--max-usd` (default 0: price nothing
  paid, say what is missing). It never prices a whole history in one
  request and never shows a total it hasn't priced: it says how many days
  are priced and how many are left.
- **It never approves.** `sync` never calls `/submit`, never reads or
  sends the order token, and has no `--yes`. A person approves with
  `tt approve` at a terminal or in the console. With no TTY it leaves the
  quote, notifies and exits 3 (refused: it needs a person), as `tt` does
  (#64).
- **A quote is reused while it is good.** Before pricing, `sync` looks for
  a waiting quote for the same request (`GET /quotes`) and offers that one
  instead of making another. Paid quotes are good for an hour
  (`quote_ttl`, `databento-jobs/src/main.rs:107`), so a nightly run usually
  finds last night's expired and prices again; a rerun within the hour
  reuses it.
- **Pricing from a batch job.** The console's write gate refuses
  `/api/jobs/...` without a person's login, and a tagged node has none, so
  a batch job doesn't go through the console. It prices at
  `databento-jobs`' own `/price` on the service network (free, no token),
  and the quote waits in `databento-jobs`' `/quotes`, where the console
  shows it. The notice goes to the deployment's notification address (the
  one the console's `CONSOLE_NOTIFY` uses), naming the quote, its days and
  price, and how to approve it; the people in `CONSOLE_WRITE_LOGINS` are
  the ones who can.
- **One operation:** it plans the days and calls `fetch` for them, with the
  same paging, pacing, retries and output.
- **Run anywhere:** by hand, or nightly as a batch job behind the
  deployment's start gate (after the close, capture idle). The evening
  top-off inside `alpaca-history` becomes that job; the job's definition
  lives in the site's repository, not here.

### 7. Tinytrader is Cargo and Docker; deployment is elsewhere

tinytrader builds and runs with Cargo, and packages with its `Dockerfile`
and `docker buildx bake`. It carries no Nix: the `tinytrader.services`
NixOS and nix-darwin modules, the Nix packages and the image building in
`nix/` move to tinytrader-deploy, which runs the stack (Compose for one
machine, Nomad for a network, Nix as the advanced path). The leak check
(`generic`) becomes a plain script CI runs. A deployment's own repository is
the site.

It can't be one change on one day. A services host's auto-deploy runs
`$TINYTRADER_SRC/nix/build-images.sh` every 5 minutes, and both
tinytrader-deploy and a site take `nixpkgs.follows = "tinytrader/nixpkgs"`:
deleting tinytrader's flake first breaks both. So, strictly in order:

1. **tinytrader-deploy first.**
   - Its own `nixpkgs`, `crane` and `rust-overlay` inputs, `nixpkgs`
     pinned to tinytrader's current rev (825e2028 in `flake.lock`).
   - tinytrader as a `flake = false` input: source, not a flake.
   - In from tinytrader: `nix/rust.nix` (with `tt`), `nix/images.nix`,
     `nix/build-images.sh`, `nix/watch*` and `infra/modules`, keeping the
     `tinytrader.services` option name, so a site's configuration is
     unchanged; its packages and modules point at itself.
   - `scripts/deploy`, `scripts/local-up.sh` and `scripts/auto-deploy` run
     its own `build-images.sh` over the tinytrader source, and
     auto-deploy's path classifier (which today names `flake.nix`,
     `nix/rust.nix`, `nix/images.nix` and `nix/build-images.sh` in
     tinytrader) is updated to match.
2. **Then the site.** Its `nixpkgs.follows` points at
   `tinytrader-deploy/nixpkgs`, and its tinytrader input is dropped or made
   `flake = false`. The machines' configuration is bumped and each host
   switched, after hours.
3. **Only then tinytrader.** `flake.nix`, `flake.lock`, `nix/`, `infra/`
   and `.envrc` deleted; CI's `nix-linux` and `nix-macos` jobs replaced by
   a `generic` check script and a `tsc` job for the console's types (today
   `nix/web.nix`'s `console-types`); and the `main` ruleset's required
   checks (today `nix-linux`, `nix-macos`, `cargo`) changed in the same
   change, so no PR waits on a check that no longer runs.

### 8. Filing throughput

Filing 680 small `cbbo-1m` jobs (about 5 MB each) ran at about one a
minute (2026-10-26). Two things make it slow:

- `databento-jobs` holds one semaphore, `one_at_a_time`
  (`databento-jobs/src/main.rs:426`, taken at 2364), across both the
  download and the filing, so no job downloads while another files.
- The store rewrites a whole day file for each job that merges into it
  (0018's merge). A job with nothing to merge files in about 6 s; one that
  merges into a day the store partly holds takes 45 to 100 s.

The store already has one global `filing` mutex
(`store/src/main.rs:149`), taken by every write that files, imports,
adopts or applies conditions; it is the one-writer rule, and a second one
in `databento-jobs` adds nothing to it.

- **(a) Downloads in parallel.** `databento-jobs` downloads up to N jobs at
  once (`--downloads`, default 4) and no longer serializes filing itself:
  each job files when its download is done, and the store's lock is the
  only single-writer rule. The capture-live pause, the in-flight set (one
  run per job), the retry schedule and the purchase guards (0019) are
  unchanged.
- **(b) One rewrite per day.** When several staged jobs touch the same
  (dataset, symbol, schema, day), the store merges them into that day file
  once: the held day and every incoming file, in timestamp order, in one
  write. Each job's filing still answers for itself. What 0018's merge
  guarantees still holds, input by input: every input's records are in the
  merged file, all of them, in order (counts and running SHA-256 per
  input, and the total is the sum); the ledger records each input's size,
  SHA-256 and record count, and the merge step names every input; the
  merged file is written beside the day, read back, recorded as pending
  and swapped in by rename, so an interrupted merge settles from the hash
  to before or after, never between. A job refused by the checks (headers
  that can't merge, contracts already held for those hours) is refused
  alone; the others in the group merge without it.
- **(c) A lock per day.** The global `filing` mutex becomes a lock per
  (dataset, symbol, schema, day) under a store-wide read-write lock, which
  the whole-store writes (adopting, coverage rebuild, conditions, the
  scan's settling) take exclusively. Deferred until the store runs on a
  machine with more memory and I/O than the 8 GB M2 with USB disks: there,
  parallel rewrites contend for the same disks and memory, and (a) and (b)
  remove most of the wait.

(a) and (b) are one PR (R4a); (c) is a later step (R4b).

**R4a acceptance:**

- With a fake provider, four downloads run at once and a fifth waits;
  with `--downloads 1` the old order is kept. A filing in progress doesn't
  hold a download back.
- The capture-live pause holds every download; one job is never run twice
  at once; a failed filing is retried on the same schedule as today.
- `databento-jobs`' purchase-guard tests pass unchanged.
- Three staged jobs into one partly held day: the day file is written
  once (one rename), its record count is the held count plus the three
  inputs', each input's records are found in order by its running hash,
  and the ledger has each input's SHA-256 and count.
- A crash injected after the write, after the pending record and after
  the swap: the next scan settles each to the day before or after the
  whole group, and filing again completes it.
- One refused job in a group of three: it is refused with the same reason
  as alone, and the other two are filed.
- The PR reports the filing rate on a disposable store of 50 jobs of about
  5 MB into partly held days, against the one a minute seen on 2026-10-26. Nothing runs
  against the real volumes.

**R4b acceptance:**

- Two filings into different days run at once; two into the same day run
  one after the other; adopting, a coverage rebuild, applying conditions
  and the scan's settling wait for every filing and hold all of them off.
- R4a's merge, crash and refusal tests pass unchanged.
- It is measured on the machine it was deferred for before it is turned
  on: the rate with one lock and with per-day locks, same backlog.

### 9. Not taken

Hyphae's slot transactions, page evidence and framed transport; Reactor's
envelope and topic machinery and its DBN crate; the store as a library
(0005); a connector framework.

## Steps and budgets

Each step is its own PR, built after hours, accepted on its own. Lines
are production / tests added, then removed; a move counts once. Fixtures
and docs are counted where a step has them (R4a and R4b give all four).
Stop and report before going 30% over.

| Step | Adds | Removes | Notes |
|---|---|---|---|
| **R0.** The two bugs: `alpaca-data`'s 429, the relay's unbounded pages | 40 / 60 | – | first, small |
| **R1.** `libs/schedule`'s civil dates public; the copies deleted (not `order-gateway`'s yet) | 20 / 30 | ~250 | pure |
| **R2.** `libs/instruments`: both OCC forms, option parents; tests over today's behaviour first | 250 / 250 | ~300 | |
| **R3.** `libs/service`: serve and drain, clients, secrets, `supervise`, `CaptureGate`; the services but `order-gateway` moved onto it | 300 / 200 | ~500 | every service but `order-gateway` (R12) drains on SIGTERM; `init: true` in Compose until then |
| **R4.** The store's `main.rs` split into modules (a pure move) | moves ~1,700 | – | no logic change |
| **R4a.** Filing throughput (section 8, a and b): `databento-jobs` downloads N at once and leaves filing to the store's lock; the store merges each day once per group of jobs | 200 / 350; fixtures 0 bytes (generated in temporary roots); docs 40 | ~30 | one PR; after R4; the purchase guard tests pass unchanged |
| **R4b.** The store's per-day filing locks (section 8, c) | 120 / 200; fixtures 0 bytes; docs 15 | ~20 | deferred until the store runs on a bigger machine than the 8 GB M2 with USB disks |
| **R5.** `venues/databento` (0021 step 9c); `databento-jobs` split | moves ~1,400 | ~150 | the purchase path: its guard tests must pass unchanged |
| **R6.** `venues/alpaca` (0021 step 9b); the five data services onto it (`alpaca-account`, `alpaca-data`, `alpaca-history`, `alpaca-news`, the relay) | moves ~1,700 | ~450 | not `order-gateway` (R12) |
| **R7.** `libs/fanout`, `relay-proto`; the relay as sources and taps | 700 / 500 | ~600 | |
| **R8.** The recorder and catch-up, ported from Hyphae | ~1,000 / ~800 | – | with 0013's capture sizing |
| **R9.** `libs/options` and the console's analytics moved into the services | ~900 / ~600 | ~1,200 JS | |
| **R10.** `libs/fleet` typed views for both front ends | 250 / 150 | ~150 JS, ~200 Rust | |
| **R11.** DBN header reading and the merge into `libs/dbn` | moves ~500 | ~250 | the store producer (0021 step 5) needs the merge |
| **R12.** `order-gateway` onto the shared crates (`libs/service`, and `venues/alpaca` with its `trading` client moved in); `session.rs` split by phase | moves ~3,000 | ~570 | under the trading hold, after a close |
| **R13.** `venues/coinbase` and a Coinbase relay source, ported from Hyphae's venue and live driver (public feed, no account); `ticker` retired, and everything that names it changed in the same step | ~1,800 / ~900; docs ~150 | ~430 (`ticker`) | needs Hyphae's crypto DBN records (`TradeWide`, `MboWide`, `Mbp10Wide`) in `libs/dbn` first, which is 0021's step 9a; see below |
| **R14.** `tt sync` (section 6): newest first, `--max-days` and `--max-usd`, chunked pricing, quote reuse, the batch job's pricing and notice; the evening top-off as a batch job | 350 / 300; docs ~60 | ~60 | needs `tt` (0021 step 3, #64) and R6; the batch-job definition is in the site's repository |
| **R15.** Nix out of tinytrader (section 7): modules, packages and image building to tinytrader-deploy; `generic` as a script and `tsc` as a job; CI and the required checks without Nix | moves ~1,500; docs ~40 | ~60 (flake, `.envrc`, CI jobs) | three PRs in strict order: tinytrader-deploy, then the site (hosts switched after hours), then tinytrader |
| **Total** | ~5,930 / ~4,340 added; moves ~9,800; docs ~305; fixtures 0 bytes | ~3,870 Rust, ~1,350 JS | R4a and R4b are ~320 / ~550 of the added lines, ~55 of docs and ~50 removed |

0021's steps 1 (`libs/net`) and 2 (`libs/store-client`) come before R3, R5
and R6; R5 and R6 are 0021's 9c and 9b. Order: R0, R1, R2 any time; then
R3; R4 and R5 any order; R4a after R4; R4b after R4a, deferred as above; R6 after R3; R7 after R3; R8 after R7; R9 and R10
any time; R11 before 0021's store producer; R12 last; R13 after 0021's 9a (the crypto records), R3 and R7 (it uses `libs/service`'s supervised loop and the fan-out), and early among them; R14 after #64 and R6; R15 any time, its three PRs in section 7's order.

**R13: what retiring `ticker` breaks**, all changed in the same step:

- the quick starts: tinytrader-deploy #2's, and #65's quick start and its
  Services table, both replaced by the Coinbase relay and the console, so
  the first example still needs no account;
- `docs/api/README.md:79` (its port, 9105), `docker-bake.hcl`'s `SERVICES`,
  and `flake.nix`'s `nixosConfigurations.example` (if R15 hasn't removed
  it);
- the `ghcr.io/<owner>/tinytrader-ticker` image: no longer built or
  published (`SERVICES` drives `images.yml`); its last tags stay, not
  deleted, so a deployment that pins one still pulls, and the package's
  description says it is retired for the Coinbase relay.

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

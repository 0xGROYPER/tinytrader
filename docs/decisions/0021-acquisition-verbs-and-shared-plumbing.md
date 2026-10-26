# 0021: Acquisition as verbs any front end calls, on shared plumbing

- **Status:** Proposed
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

### 1. Four verbs, one meaning everywhere

| Verb | What it does | Served by |
|---|---|---|
| `fetch` | A historical range into the store: priced first; free goes ahead, paid waits for a person's yes. | `databento-jobs`, `alpaca-history` |
| `poll` | `fetch` of the latest days, on a schedule. Not a separate code path: the deployment's scheduler (0020; Nomad today) runs `tt fetch`, so nothing in tinytrader keeps a schedule. | the same |
| `listen` | Live DBN from now. | the relay |
| `replay` | DBN of held days from the store, then (optionally) `listen`: the same protocol and bytes as live. | the relay, with the store as a source |

**A paid poll still needs a person.** 0019 stands: a scheduled `fetch` of a
free source goes ahead; of a paid one it prices, leaves the quote waiting
and notifies, and buys nothing until someone says yes. A standing approval
(per dataset, a daily dollar cap, expiring) would be its own decision; this
one doesn't propose it.

Each verb is an HTTP endpoint first. A command line, the TUI and the console
are front ends over the same endpoints, as 0003 has it for reads.

### 2. `tt`: the command line

`apps/tt`, one binary, front end only (no logic a service doesn't own):

```text
tt fetch PROVIDER DATASET SCHEMA SYMBOL START END [--hours 09:30-16:01] [--contracts FILE]
    → prices it, prints the quote (cost, requests, skipped, expiry), and for a
      paid one asks "Buy for $45.03? [y/N]"; y approves, then follows the
      submit and filing to the end
tt quotes                      waiting quotes, with cost and time left
tt approve QUOTE | tt decline QUOTE
tt jobs [--follow]             purchases and their downloads and filing;
                               --follow ends with each job's filing result
tt listen SYMBOL… | tt replay SYMBOL… --start T [--end T]   DBN to stdout
```

**Approval goes through the console, not a key.** `tt approve` sends the
console's own `POST /api/job-quotes/{id}/approve` with `confirm_usd`; the
console's write gate takes the login from its authenticating proxy
(`tailscale serve` sets `Tailscale-User-Login`) and adds the approval token,
exactly as for the button. So `tt` works from any machine on the tailnet
signed in as an allowed login, holds no secret, and every approval records
who gave it: the approver is the login the proxy sets, never a value `tt`
sends.

- `tt` approves only through the proxy's address (`TT_CONSOLE`, for example
  `https://mini.tail8aa7c8.ts.net`), even on the console's own host. A
  loopback address has no proxy login, so the write gate refuses it, and
  `tt` says why instead of retrying.
- With no authenticating proxy (no allowed logins configured), the console
  refuses every write, approvals included, and `tt approve` refuses with
  that message. There is no fallback to a token alone. Where the console asks for its order token on approvals (as
for a live order, unless `CONSOLE_LIVE_NEEDS_TOKEN` is `false`), `tt` asks
for it at the prompt and keeps it for that run only. The y/N is the confirmation 0019 asks for: it shows the exact
price and sends that price.

**A quote that expires is said so.** Quotes carry `expires_at`; `tt` and the
console's Jobs widget both show the time left, and an approval after expiry offers to price it
again (free) and asks again, never buys at a new price unasked.

`tt` replaces the queue's `tt` that 0020 removes; it shares no code with it.

### 3. One contract, generated

The endpoints these verbs use are described once, by an OpenAPI document
generated from the axum handlers' types, and CI fails on a breaking change
to it. `tt`, `libs/store-client`, the console's widgets and research's
Python client are built against that document, not kept by hand. This
covers only the endpoints the verbs touch at first: `databento-jobs`
`/price`, `/quotes`, `/submit`; the store's `/catalog`, `/coverage`,
`/files`; the console's `/api/job-quotes`. Other services follow as they're
touched.

### 4. Shared plumbing in `libs`

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

### 5. Replay from the store

The relay gains a store source: for `start` before today on a dataset the
store holds, it sends the stored records for the days asked, then
`ReplayCompleted`, then live if the request asked for it. The records sent
are the stored DBN, unchanged.

- **Order.** Files are merged k-way by each record's index time, as
  Databento orders them (`ts_recv` where the schema has it, else
  `ts_event`). Ties keep the order of the request's symbols, then each
  file's own order, so a replay is the same every time.
- **Pace.** As fast as the client reads, by default, with backpressure: the
  relay never buffers a day. `speed=1` paces by index time (real time),
  `speed=N` N times faster. There is no simulated clock: a client that
  wants one keeps it from the record times.
- **Size and drives.** A `cmbp-1` day is up to about 28 GB, on drives that
  sleep (12–30 s for a first read). Files are streamed, never loaded whole;
  a read that stalls over 5 s sends a waiting message, and one that fails
  sends a gap, never a timeout that drops the client.
- **Holes.** A day the store lacks is a gap message, never a silent skip.
  A partial day sends what's held and a gap message naming what isn't (its
  coverage pieces: the contracts and hours missing).

## Steps and budgets

Each step is its own PR, built after hours. Production / tests lines; stop
and report before going 30% over.

| Step | Prod | Tests |
|---|---|---|
| 0. Generated OpenAPI for the endpoints above, and the CI check | 300 | 100 |
| 1. `libs/net`; the data services' backoff loops moved onto it (net negative) | 400 | 300 |
| 1b. order-gateway and alpaca-account onto `libs/net` (own PR, after a close) | 150 | 150 |
| 2. `libs/store-client`; `alpaca-history` and `databento-jobs` on it | 250 | 200 |
| 3. `tt fetch/quotes/approve/decline/jobs`; quote `expires_at` in `tt` and the console | 450 | 250 |
| 4a. Relay store source: one symbol-day, streamed, holes as gaps | 450 | 300 |
| 4b. The k-way merge across files, pace, and `tt listen/replay` | 450 | 300 |
| 5. Docs: the verbs in each API page | (docs) | |

Step 0 comes first for the endpoints 2 and 3 use; 1, 2 and 3 can then land
in any order; 4a needs 2, 4b needs 4a. Each step reports the lines it
removes beside these.

## Consequences

- Approving a purchase is `tt fetch …` then `y`, from any tailnet machine,
  with the same guards and ledger as the console.
- A new feed gets retries, reconnects, store filing and a fake provider for
  free, and the review of a feed is of what's specific to it.
- A strategy runs over held days through the relay, as it runs live; OPRA
  `cmbp-1` days held in the store can be replayed although Databento won't.
- `libs/net` and `libs/store-client` are contracts between services; change
  them additively.
- **Open: the name.** `tt` is already the queue's command (removed by 0020),
  and research imports a Python package `tt`. A binary and a Python package
  don't collide on `PATH`, but if the shared name confuses, the command can
  be renamed before step 3.
- Not doing: Hyphae's slot transactions and page evidence (0014's reasons
  stand), a connector framework, or a scheduler.

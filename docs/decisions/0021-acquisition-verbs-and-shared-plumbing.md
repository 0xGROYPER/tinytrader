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
| `poll` | `fetch` of the latest days, on a schedule. Not a separate code path: the deployment's scheduler (0020) calls `fetch`. | the same |
| `listen` | Live DBN from now. | the relay |
| `replay` | DBN of held days from the store, then (optionally) `listen`: the same protocol and bytes as live. | the relay, with the store as a source |

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
tt jobs [--follow]             purchases and their downloads and filing
tt listen SYMBOL… | tt replay SYMBOL… --start T [--end T]   DBN to stdout
```

**Approval goes through the console, not a key.** `tt approve` sends the
console's own `POST /api/job-quotes/{id}/approve` with `confirm_usd`; the
console's write gate takes the login from its authenticating proxy
(`tailscale serve` sets `Tailscale-User-Login`) and adds the approval token,
exactly as for the button. So `tt` works from any machine on the tailnet
signed in as an allowed login, holds no secret, and every approval records
who gave it. Where the console asks for its order token on approvals (as
for a live order, unless `CONSOLE_LIVE_NEEDS_TOKEN` is `false`), `tt` asks
for it at the prompt and keeps it for that run only. The y/N is the confirmation 0019 asks for: it shows the exact
price and sends that price.

**A quote that expires is said so.** Quotes carry `expires_at`; `tt` and the
console show the time left, and an approval after expiry offers to price it
again (free) and asks again, never buys at a new price unasked.

`tt` replaces the queue's `tt` that 0020 removes; it shares no code with it.

### 3. Shared plumbing in `libs`

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

Services keep their own HTTP APIs and guards. Nothing here is a framework:
no service is required to have a given shape, only to stop copying these.

### 4. Replay from the store

The relay gains a store source: for `start` before today on a dataset the
store holds, it reads the store's files for the days asked (in time order
across symbols, as the live feed is) and sends them, then `ReplayCompleted`,
then live if the request asked for it. The bytes sent are the stored DBN,
unchanged. A day the store lacks is a gap message, never a silent skip.

## Steps and budgets

Each step is its own PR, built after hours. Production / tests lines; stop
and report before going 30% over.

| Step | Prod | Tests |
|---|---|---|
| 1. `libs/net`, and the seven backoff loops moved onto it | 400 | 300 |
| 2. `libs/store-client`; `alpaca-history` and `databento-jobs` on it | 250 | 200 |
| 3. `tt fetch/quotes/approve/decline/jobs`; quote `expires_at` | 450 | 250 |
| 4. Relay store source and `tt listen/replay` | 600 | 450 |
| 5. Docs: the verbs in each API page | (docs) | |

Steps 1–3 can land in any order; 4 needs 2. Lines removed by moving to
shared code are reported beside these.

## Consequences

- Approving a purchase is `tt fetch …` then `y`, from any tailnet machine,
  with the same guards and ledger as the console.
- A new feed gets retries, reconnects, store filing and a fake provider for
  free, and the review of a feed is of what's specific to it.
- A strategy runs over held days through the relay, as it runs live; OPRA
  `cmbp-1` days held in the store can be replayed although Databento won't.
- `libs/net` and `libs/store-client` are contracts between services; change
  them additively.
- Not doing: Hyphae's slot transactions and page evidence (0014's reasons
  stand), a connector framework, or a scheduler.

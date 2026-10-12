# Order gateway protocol

**Status:** contract v1 (`tinytrader.gateway/1`). It began as Hyphae's `hyphae.gateway/1` (agreed 2026-10-04), whose simulated broker and Alpaca paper broker were built and tested there; the operator moved order handling to tinytrader on 2026-10-08, and v1 gained stock orders (single limit orders, `day` or limit-on-close) the same day. Items marked **UNVERIFIED** are broker behaviors that the paper qualification must capture.

`order-gateway` is a broker-access passthrough. It handles authentication, validation, forwarding, correlation, event delivery and mechanical safety. It has no strategy, no portfolio and no planning. Clients own their strategy, sizing, positions and P&L; the broker is the authority for order state.

## 1. Process and account model

- **One process per broker account.** `order-gateway` runs in the `services/order-gateway` package. Each process serves exactly one account, holds only that account's credentials, and owns the single `trade_updates` connection that account is allowed.
  - Several accounts mean several processes, for example `alpaca-paper-1` on port 7601 and `alpaca-paper-2` on 7602.
  - Each process runs in its own tmux session or supervisor unit.
- **Broker kinds:** `alpaca` (paper only in v1) and `sim` (see [section 10](#10-simulated-broker)).
  - Both implement one synchronous `Broker` seam.
  - The process selects one of them through an enum. There are no plugins.
- **Independence:**
  - The process does not need feed, relay, replay or the data gateway to run.
  - The one exception is the simulated broker in market mode, which reads a quote source that the operator names.
- **Library use:** the same library can be embedded in-process, for example a backtest driving `SimBroker` directly.

The inputs (the full syntax is in [Running `order-gateway`](#running-order-gateway)):

| Input | Example | Rule |
|---|---|---|
| account name | `--account alpaca-paper-1` | Gateway-local name. |
| broker kind | `--broker alpaca\|sim` | Selects the enum arm. |
| environment | `--environment paper` | Selects a compiled endpoint pair. There is no URL flag. `live` also requires `--allow-live` and is out of scope for v1. |
| credentials | `--credentials env`, or `--credentials-file FILE` | `env`: `APCA_API_KEY_ID` and `APCA_API_SECRET_KEY`, each from the environment or a `*_FILE` (a Docker secret), decrypted by sops at launch. A file: mode 600, not a symlink, `APCA_*` or `ALPACA_*` names. Values are never logged. |
| listen | `--listen 127.0.0.1:7601` | Must be a loopback address; anything else is refused, unless `--listen-any`, which is only for a container whose port is published on loopback. |
| state root | `--state ~/.local/state/order-gateway/alpaca-paper-1` | Holds the journal ([section 11](#11-order-journal)). It must not be under `/tmp` or `/Volumes/Data`. |
| clients | `--client NAME,ROLE,PREFIX,TOKEN` (repeatable; `TOKEN` is a mode-600 file or `env:VAR`) | See [section 4](#4-authentication-and-roles). |
| profile | `--max-qty 10 --budget 150 …` | Caps and allowlist ([section 8](#8-rate-budget-and-caps)). |

### Identities

| Identity | Assigned by | Rule |
|---|---|---|
| `account` | operator | Binds the broker kind, environment, credential file and profile. |
| `client` | operator | A name and a token, bound to one account, with one role and one `client_order_id` prefix (Reactor uses `wse-`). |
| `client_order_id` | client | Mandatory. It is the **only logical order key**: 1 to 128 characters from `[A-Za-z0-9._:-]`, starting with the client's prefix. It is unique per account for as long as the journal reserves it (30 days), and is never reused, even after a rejection or an `absent_after` result. |
| `request_id` | client | Correlates one request with its response. 1 to 64 characters, unique within a connection. |
| `broker_order_id` | broker | Kept exactly as received (an Alpaca UUID). `cancel` and `order` accept it as well as `client_order_id`. |
| `seq` | gateway | Per account, starting at 1, monotonic and gap-free in the journal. It never resets across days or restarts. |

There is no gateway `order_id` and no separate `command_id`. The broker is the authority for order state.

## 2. Framing and versioning

- **Transport:** TCP on loopback.
- **Framing:** each message is one UTF-8 JSON object on one line, terminated by `\n`.
  - The maximum line is 65,536 bytes, including the newline.
  - JSON whitespace outside strings is allowed but never contains a newline.
- **Schema field:** every message in both directions carries `"schema":"tinytrader.gateway/1"` and a `type`. Sketches in sections 3 to 12 leave out `schema` to save space; the worked examples in [section 13](#13-worked-examples) show it.
- **Strictness:**
  - Requests from clients are strict: an unknown field, a missing field or a wrong JSON type is refused.
  - Messages from the server may gain fields within v1, and clients must ignore fields they do not know.
  - A new `event` value or `kind` is additive. A client that sees an unknown value must keep the line and not fail.
- **Value encodings:**
  - Prices are decimal strings, such as `"1.25"` or `"-0.40"`.
  - Quantities and ratios are JSON integers.
  - Timestamps are integer nanoseconds since the Unix epoch, UTC. They exceed 2^53, so JavaScript clients must parse them as BigInt.
- **`broker_raw`:** the broker's JSON value, embedded as a JSON value.
  - Key order, string contents and number lexemes are preserved.
  - Only whitespace outside strings is removed, which is needed for line framing.
  - Clients can parse it exactly as they parse Alpaca responses today.
- **Breaking changes** ship as `tinytrader.gateway/2`, served alongside `/1` for an announced overlap period. They are recorded on this page and in the Reactor interface contract.

## 3. Connection lifecycle

```text
C→S hello      {client, token, resume_after_seq: N | null, cancel_on_disconnect?: false}
S→C welcome    {account, broker, environment, state, last_seq, resume, capabilities, pins, limits}
S→C event …    replayed journal suffix (seq > N, visible to this client), then live events
C→S request …  S→C response …   (interleaved with events, in production order)
S↔C heartbeat  {ts, last_seq}   after 10 s without sending; a peer silent for 30 s is closed
S→C error      {code, message}  fatal errors close the connection after sending
```

- **Hello:**
  - `hello` must be the first line and must arrive within 5 s.
  - The token is compared in constant time.
  - A wrong token, or a client bound to a different account, gets `error{unauthorized}` and the connection closes.
  - The process has no general unauthenticated phase. A client must authenticate even to run `clock`.
- **`welcome.state`:** one of `reconciling`, `ready`, `degraded` or `killed`.
  - `capabilities` gives the shapes, the maximum number of legs, the tick rule, the `client_order_id` length, whether the broker supports client ids (`idempotency: "broker_client_id"`), and the events the broker can produce.
  - `pins` identifies the broker build, the profile hash and, for the simulator, the reproducibility pin from [section 10](#10-simulated-broker).
- **`welcome.resume`:**
  - `full`: every requested event follows.
  - `truncated{first_available_seq}`: the requested seq is older than the resume window, so the client must reconcile through queries.
  - `none`: `resume_after_seq` was null and only live events follow.
- **Ordering:**
  - Each connection has one writer thread that drains a bounded queue (4 MiB by default).
  - Responses and events go out in the order the account session produced them.
  - A `submit` response always precedes every event for that order.
- **Slow consumers:**
  - If the queue overflows, the gateway sends `error{slow_consumer}` and closes the connection.
  - Nothing is lost: the client reconnects with `resume_after_seq` and the journal replays the suffix. This is a deliberate difference from the market-data relay.
- **Responses are not replayed.** Every outcome a client needs after a reconnect is also a journaled event (`submit`, `cancel`). A client that lost its connection before reading a response recovers the outcome from the replay.
- **Heartbeats** carry `last_seq`, so an idle client can confirm it is caught up.

## 4. Authentication and roles

- **Token files:**
  - Each client has a token file, by convention `~/.config/tinytrader/gateway-clients/<client>.token`.
  - The file is mode 600, not a symlink, and holds 32 or more random bytes encoded as hex.
  - The operator creates token files; agents never do.
  - A process loads only the tokens bound to its own account.
  - Tokens never appear in logs, the journal or errors.

| Role | Allowed |
|---|---|
| `trade` | `submit` and `cancel` for ids with its own prefix, plus every query. It sees its own orders and account-wide events. |
| `observe` | Every query and every event on the account. It cannot submit or cancel. |
| `admin` | Everything `observe` can do, plus `kill`, `unkill`, and `cancel` of any order. |

- A request outside the client's role gets `response{outcome:"error", code:"forbidden"}`.
- Access to the market-data relay grants no access to orders. They are separate processes, ports and tokens.

## 5. Requests and responses

Every request has the form `{type, request_id, …}`. Every response is `{type:"response", request_id, op, outcome, …}`.

| `type` | Fields | Roles | Response `outcome` |
|---|---|---|---|
| `submit` | `intent` ([section 6](#6-order-intent)) | trade | `accepted`, `rejected`, `not_sent`, `duplicate` or `unknown` |
| `cancel` | `client_order_id` or `broker_order_id` | trade (own prefix), admin | `accepted`, `rejected`, `not_sent` or `unknown`. The terminal state arrives later as an event. |
| `order` | `client_order_id` or `broker_order_id` | all | `ok{order: Order \| null}` or `error` |
| `open_orders` | — | all | `ok{orders: [Order]}` (nested legs) |
| `account` | — | all | `ok{account: {status, cash, buying_power, options_trading_level, blocked}}` |
| `positions` | — | all | `ok{positions: [{symbol, qty (signed integer), avg_entry_price}]}` |
| `activities` | `types: [..]`, `after?`, `page_token?` | all | `ok{items: [raw], next_page_token}`, one bounded page |
| `clock` | — | all | `ok{clock: {ts, is_open, next_open, next_close}}` |
| `calendar` | `start`, `end` (dates) | all | `ok{days: [{date, open, close}]}`, with times in America/New_York |
| `kill` | `level: halt_opens \| halt_all`, `reason` | admin | `ok{state}` |
| `unkill` | `reason` | admin | `ok{state}` |
| `flatten` | `reason` | admin | `ok{state, positions: [{symbol, status, broker_raw}], broker_raw}`, or `rejected` / `unknown` ([section 9](#9-safety-rules)) |
| `advance` | `to_ts` | trade | `ok{market_ts}`. Only for the simulator in client-clocked replay ([section 10](#10-simulated-broker)). |

- Every `ok` query response carries `ts_gateway` and `broker_raw`.
- An `Order` contains: `client_order_id`, `broker_order_id`, `status`, `filled_qty`, `avg_price`, `legs?[{symbol, filled_qty, avg_price}]`, `intent_sha256` (null for an order the gateway did not submit) and `broker_raw`.

### Submit outcomes

| Outcome | Meaning | Fields |
|---|---|---|
| `accepted` | The broker acknowledged the order. This is **not** a fill. | `client_order_id`, `broker_order_id`, `status`, `broker_raw` |
| `rejected` | The broker refused the order. Final. | `http_status`, `code`, `message`, `broker_raw` |
| `not_sent` | **Nothing reached the broker.** Final, and journaled. | `reason`, `detail` (see [error codes](#12-error-codes)) |
| `duplicate` | The `client_order_id` is known **and** its canonical intent is byte-identical. Nothing is sent. | `current: Order` (the last journaled state) |
| `unknown` | The request may have reached the broker. | `reason: timeout \| connection \| http_5xx \| http_429_after_send` |

A known `client_order_id` with a *different* intent is `not_sent{duplicate_conflict}`.

### Unknown outcome: never resend

1. The gateway **never resends** a submit. The `send` phase is journaled before any bytes leave, so this rule also holds across a crash or restart.
2. It looks up the `client_order_id` at +1, +2 and +4 s, using the critical budget.
3. If the order is found, it emits `resolution{outcome:"found"}` followed by the order's state as `order` events with `source: rest_reconcile`.
4. If the order is not found by the end of the window, it emits `resolution{outcome:"absent_after", window_ms}`.
   - This is **not** a statement that the order was never submitted.
   - The id stays reserved, and a late `trade_updates` event or a later reconciliation still correlates to it.
   - What to do next is the client's decision. Reactor today treats an order absent for 5 s as not submitted.

Cancel follows the same rule. An `unknown` cancel is not repeated by the gateway. The client may send another `cancel`, which is idempotent at the broker.

## 6. Order intent

The intent is broker-neutral in its naming and closed in its content. v1 is isomorphic to Reactor's two Alpaca shapes, so the Alpaca mapping is total and loses nothing.

```text
intent = {
  client_order_id,
  shape: "single" | "combo",
  // single:
  instrument: {class: "option", symbol: OCC} | {class: "equity", symbol: TICKER},
  side: "buy"|"sell",  effect: "open"|"close",
  // combo:
  legs: [{instrument, ratio: int ≥ 1, side, effect}]   // 2..=4 legs, distinct symbols, gcd(ratios) = 1
  qty: int ≥ 1,                 // contracts or shares; for a combo, parent units
  type: "limit",
  limit_price: decimal string,  // single: > 0; combo: signed net per unit, + debit, − credit, ≠ 0
  time_in_force: "day" | "cls"  // cls: the closing auction (limit-on-close), stock singles only
}
OCC    = ^[A-Z]{1,6}\d{6}[CP]\d{8}$   (compact, spaces removed)
TICKER = ^[A-Z]{1,5}(\.[A-Z])?$       (e.g. AAPL, BRK.B)
```

**Protocol validation** always runs:
- the fields are exact for the shape
- integers are within range
- the price has at most 2 decimal places and a non-zero magnitude
- the OCC grammar holds and its date is valid
- the leg rules hold
- the id grammar and prefix are correct

**Stock orders** are single legs only: a combo with an `equity` leg is invalid. `side: sell` with `effect: open` is a short sale. Limit-on-close (`cls`) fills at the closing price if the close is within the limit; the limit keeps the per-order dollar cap meaningful and guards against a bad closing print.

**The account allowlist** comes from the operator profile:
- underlyings (default `SPY`), class `option`
- clients allowed options on any underlying (`--any-underlying-client NAME`, repeatable, default none): for a manual trading client such as the console's. Only the underlying check is skipped, and only for that client's orders; every cap, the kill switch and the expiry-day cutoff still apply, and other clients on the account keep the list. The name must be a `trade` client. The profile in `welcome` lists them as `any_underlying_clients` when there are any.
- stocks (`--equities`, default none), and whether short sales are allowed (`--allow-short`, default no)
- shapes and maximum legs
- the caps in [section 8](#8-rate-budget-and-caps)

The broker's `capabilities` declare its tick rules, and the gateway refuses an intent the broker cannot accept with `not_sent{unsupported}`, before any I/O. Whether an instrument exists is the broker's answer and arrives as a broker reject. The gateway keeps no instrument master.

**Canonical form:** the gateway re-serializes the parsed intent with a fixed field order, no whitespace, and prices normalized to the shortest form with 2 decimal places. Its SHA-256 is `intent_sha256`, which is used for duplicate detection.

**Reserved for later versions, as allowlist additions rather than protocol changes:**
- market, stop and stop-limit types
- GTC, IOC and FOK
- `replace`
- index-option roots

### Alpaca mapping (v1)

| Intent | Alpaca `POST /v2/orders` body |
|---|---|
| single | `{symbol, qty, side, type:"limit", limit_price, time_in_force:"day", client_order_id, position_intent}` |
| combo | `{order_class:"mleg", qty, type:"limit", limit_price, time_in_force:"day", client_order_id, legs:[{symbol, ratio_qty, side, position_intent}]}` |
| stock single | `{symbol, qty, side, type:"limit", limit_price, time_in_force:"day"\|"cls", client_order_id}`, with no `position_intent`: Alpaca infers opening or closing from the position |
| `side` + `effect` | `position_intent` is `buy_to_open`, `buy_to_close`, `sell_to_open` or `sell_to_close` (options) |

`qty`, `ratio_qty` and `limit_price` are sent as strings. The byte form follows Reactor's `orders.py` field order (compact JSON): a single is `{symbol, qty, side, type, limit_price, time_in_force, client_order_id, position_intent}`; an MLEG is `{order_class, qty, type, time_in_force, limit_price, client_order_id, legs:[{symbol, ratio_qty, side, position_intent}]}`. `limit_price` always has two decimal places.

## 7. Events

Each event has the envelope `{type:"event", seq, ts_gateway, account, kind, …}`. Every journal line is an event. A client receives the events visible to its role ([section 4](#4-authentication-and-roles)), so a `trade` client's view is a subsequence of the account's `seq`, and gaps in that view are expected.

| `kind` | Fields |
|---|---|
| `submit` | `client`, `request_id`, `client_order_id`, `phase: admitted \| sent \| outcome`, `intent` and `intent_sha256` (admitted only), `outcome` (outcome only, the same object as the response) |
| `cancel` | `client`, `request_id`, `target`, `phase`, `outcome` |
| `order` | `client_order_id`, `broker_order_id`, `event`, `status`, `ts_broker`, `fill?{execution_id, qty, price}`, `filled_qty`, `avg_price`, `legs?[{symbol, filled_qty, avg_price, fill?{qty, price}}]`, `position_qty?`, `reason?{code, message}`, `source: stream \| rest_reconcile \| sim`, `broker_event`, `broker_raw` |
| `resolution` | `client_order_id`, `outcome: found \| absent_after`, `window_ms` |
| `account` | `snapshot: account \| positions`, `broker_raw`. Derived from polling and emitted on change; Alpaca has no account stream. |
| `activity` | `activity_type`, `broker_raw`, for example `OPEXP`, `OPASN`, `OPEXC`, `FILL` or `FEE` |
| `status` | `state`, `broker_stream: up \| down{since}`, `resync?{from_ts, to_ts, orders_checked}`, `kill?{level, reason}`, `cap?{name}` |
| `foreign` | `broker_raw`. A broker order with no journal entry, such as one placed in the dashboard. |
| `client` | `client`, `phase: connected \| disconnected`, `resume_after_seq` |
| `day` | `day_open{format, previous_file, previous_last_hash}`. The first line of each journal file. |

**The normalized `event` vocabulary for `order`:**
- `new`, `accepted`, `pending_new`
- `partial_fill`, `fill`
- `pending_cancel`, `canceled`, `expired`, `done_for_day`, `replaced`
- `rejected`, `stopped`, `suspended`, `calculated`
- `cancel_rejected`, `replace_rejected`
- `other`

`broker_event` keeps the exact broker string, so an unknown broker event arrives as `other` with its raw string. Nothing is dropped.

**Fill fields:**
- `fill.qty` and `fill.price` are the increment for that execution.
- `filled_qty` and `avg_price` are cumulative.
- For a combo:
  - `fill.price` is the net price per unit.
  - Each element of `legs` gives that leg's cumulative quantity and average price, plus `fill` for the increment.
  - How Alpaca reports MLEG fills in `trade_updates` (per leg or aggregate) is **UNVERIFIED**. Until the paper qualification captures it, `legs` comes from the nested `order.legs[]` in `broker_raw`.
- During reconciliation, if the broker's `execution_id` is missing, the gateway emits the aggregate change with `fill` absent. It never fabricates an execution id.

## 8. Rate budget and caps

- **Request budget:** one token bucket per account process, 150 requests/min (Alpaca allows 200/min per account), with two priorities.
  - `critical`: submit, cancel and unknown resolution.
  - `poll`: reconciliation, the account and position pollers, and client queries.
  - Polls are shed first.
- **When accounts may share a limit:** if several accounts share one user-level limit (**UNVERIFIED**), the operator sets `--budget` on each process so that the total stays at or below 150.
- **On a 429:**
  - The gateway sets `blocked_until = max(Retry-After, x-ratelimit-reset, backoff)`.
  - A 429 on a POST whose bytes left the socket is `unknown{http_429_after_send}`.
  - A throttle that happens on the client side, before any send, is `not_sent{throttled}`.
  - GETs may retry with bounded backoff. POSTs and DELETEs never retry.

**Per-account caps.** These are defaults from the operator profile. They are mechanical access limits, not portfolio risk. A violation is `not_sent{cap}` with the cap's name in `detail`.

| Cap | Default |
|---|---|
| submits per minute | 60 |
| submits per day | 500 |
| open orders | 4 |
| qty per order | 10 contracts (`--max-qty`) |
| shares per stock order | 1,000 (`--max-shares`) |
| legs | 4 |
| notional per order: \|limit_price\| × qty × multiplier (100 an option, 1 a share) | $2,500 |
| consecutive broker rejects | 5, which trips `halt_opens` |

## 9. Safety rules

| Rule | Behavior |
|---|---|
| Paper/live isolation | Endpoints are compiled constants selected by `--environment`. A paper process cannot build a live URL (enforced by the type system), and tests inject transports instead of overriding URLs. Live trading needs separate operator authorization and is outside v1. |
| Start gate | The process starts `reconciling` and becomes `ready` only when: the account is active and not blocked; `options_trading_level` ≥ the profile minimum (3); the journal chain verifies; and broker reconciliation ([section 11](#11-order-journal)) is complete. Until then a submit is `not_sent{not_ready}`. Foreign orders or non-option positions are reported as `foreign` events; they do not block unless the profile says so. |
| Flatten | An admin's `flatten` closes everything: it sets `kill{halt_opens}` (unless `halt_all` is already set), cancels every open order as `halt_all` does, then asks the broker to close every position at market in **one request that is never resent**. Alpaca: `DELETE /v2/positions?cancel_orders=true`, which places a market order per position; the response lists each position's `{symbol, status, broker_raw}` (its closing order, or why not). An `unknown` outcome means the request may have reached the broker: look at `positions` before asking again. The closing orders are the broker's, so they arrive as `foreign` order events, with their fills. Options close at market, across whatever spread there is. Opens stay refused until `unkill`. The request and its outcome are journaled as `flatten` events (phase `requested` / `outcome`), visible to the requesting admin and to observers. The simulated broker closes each position at its touch (else the scripted quote's mid, else its average cost). |
| Kill switch | `kill{halt_opens}` refuses any submit with an opening leg. `kill{halt_all}` refuses every submit and cancels every open order: first `DELETE /v2/orders`, then a check of each order. The default level is `halt_all`. Triggers: an admin request, the CLI through the admin role, or a `KILL` sentinel file in the state root. The kill state is journaled and **persists across restarts**; only an explicit `unkill` clears it. |
| Broker stream down | If `trade_updates` has been down for more than 5 s, the state becomes `degraded` and submits are `not_sent{not_ready}`. Cancels are still allowed. While down, the gateway polls open orders at 1 Hz. After reconnecting it resyncs through REST before returning to `ready`. |
| Client disconnect | **No automatic cancel.** A client can opt in with `hello.cancel_on_disconnect: true`: after a 2 s grace period, its open orders (by prefix) are cancelled. A gateway restart leaves broker orders alone and reconciles them. |
| Expiry-day opening cutoff | The gateway refuses (`not_sent{cutoff}`) any intent with an `effect: open` leg on a contract that expires today, from `close − 30 min − 30 s` America/New_York onward. On a normal day that is 15:29:30. `close` comes from the broker calendar. If the clock or calendar is unknown, the gateway refuses (fail closed). This rule is for SPY/QQQ-class ETFs; other underlyings use `close − 45 min − 30 s`. Stock legs never expire and are not subject to it; Alpaca takes limit-on-close orders until 10 minutes before the close. Closing legs pass the gateway; whether Alpaca accepts them after its cutoff is **UNVERIFIED**. Client policy, such as Reactor's 15:30 − 5 s rule, stays in the client. |
| Index options | Not in the v1 allowlist. |

## 10. Simulated broker

`SimBroker` implements the same `Broker` seam and the same protocol. A client cannot tell it apart except through `welcome.broker = "sim"` and `pins`. The simulator is a broker: its simulated cash, positions and activities are broker state, not a Hyphae portfolio.

### Scripted mode (`--sim-script FILE`)

This mode provides parity with Reactor's FakeBroker and needs no market data. The script is JSON and contains rules. Each rule matches the nth submit or a `client_order_id`, and gives:
- `submit`: one of `accept`, `reject{http, code, message}`, `fill`, `partial{n}`, `error_before_send`, `timeout_after_accept` or `http_429{headers}`
- `cancel`: one of `ok`, `ignore` or `reject`
- `fill_on_query`
- per-leg `skew` (unequal leg fills)
- a stream `drop{after_events}`, which exercises resync

The script also sets the account fields (status, options level, cash, blocked flags) and any foreign positions and orders. The simulator counts the sends it receives, so tests can assert that the gateway never resent.

### Market mode (`--sim-quotes SOURCE`)

**Sources.** A source is a DBN file, a `replay dbn` TCP stream, or a live relay port (7500 OPRA, 7499 IEX). Records are `cmbp-1`, `mbp-1` or `bbo-*`, with definitions; the instrument is keyed by `raw_symbol` with spaces removed.

**Book state.** For each instrument the simulator keeps the last bid and ask price and size from `levels[0]`. Rules:
- `UNDEF_PRICE` makes that side absent.
- A crossed quote (bid > ask) makes the instrument **unusable** until a non-crossed quote arrives. A locked quote (bid = ask) is usable.

**Market time.** `t_mkt` is the largest authoritative timestamp seen so far: `ts_recv` for OPRA, `ts_event` for Alpaca. It never decreases. The simulator never reads the wall clock in replay; `ts_gateway` equals `t_mkt` in replay.

**Client-clocked replay.** This is the mode that guarantees determinism.
- The simulator applies quote records only up to the most recent `advance{to_ts}` from the client.
- A request is stamped with the `t_mkt` at the moment it is processed.
- With a live relay source, the simulator advances as records arrive. That mode is **not** reproducible, and `pins` says so.

**Latency.** An order activates at `t_act = t_submit + submit_latency`. A cancel takes effect at `t_cancel + cancel_latency`. Both are constants from the profile, 5 ms by default.

**Fill model.**
- *Single buy limit L:* fillable when the ask is present and ask ≤ L. It fills at the **ask**, which is the price improvement up to the touch.
- *Single sell:* fillable when the bid is present and bid ≥ L, at the bid.
- *Combo:* the net cost per unit is
  `N = Σ_buy ratio·ask − Σ_sell ratio·bid`
  over the current state of every leg. Every needed side must be present and every leg usable. The combo is fillable when N ≤ `limit_price`; this one rule covers both debits and credits, because credits are negative. It fills **atomically**, with each leg at its touch and the parent `fill.price = N`.
- *Quantity:* the default is atomic full quantity. With `size_cap: displayed`, the fill is min(remaining, displayed size) for a single, or min over legs of ⌊size / ratio⌋ for a combo. The rest stays working. A displayed size, once consumed, is not consumed again until a new quote record arrives for that instrument.
- *Expiry:* working `day` orders expire at the session close in the profile calendar, with an early close where the calendar lists one.
- *Fees:* each fill carries `fees` from the fee profile. The default is $0 commission plus a configurable per-contract regulatory fee.

**Rejects.**
- An instrument with no definition seen gets `rejected{code:"sim_unknown_instrument"}`.
- A submit outside the session gets `rejected{code:"sim_market_closed"}`. This is a v1 simplification; Alpaca queues day orders submitted outside the session.

**Deterministic ordering rules.**
1. Quote records are applied in input order. Records with equal timestamps keep their input order.
2. An order action (activation or cancel effect) at time `t` is applied **after** every quote record with `ts ≤ t` and before any record with `ts > t`.
3. On activation, an order is evaluated against the book as of `t_act`. After that it is re-evaluated after each quote record that touches one of its legs.
4. When several working orders are fillable on the same record, they are evaluated in order of (`t_act`, admission `seq`). Under `size_cap: displayed` they consume the displayed size in that order.
5. If a fill and a cancel take effect at the same time, the fill wins, because the cancel effect waits for rule 2. A cancel response is `accepted` whenever the order is open when the request arrives. The terminal event is `canceled`, or `fill` if a fill came first.

**Reproducibility pin.** `pins` contains:
- the quote source and its manifest sha256 or relay stream id
- the simulator model version
- the latencies
- the fee profile
- `size_cap`
- the calendar
- the script sha256

The same pin and the same request sequence produce a **byte-identical event transcript**.

**Deferred:** expiry settlement (`OPEXP`, `OPASN`, `OPEXC`), queue and L2 models, and stochastic latency. Each one changes the pin, not the protocol.

## 11. Order journal

The journal is the minimal correlation state that OBJ-8 asks for. It serves four needs:
- idempotency across restarts
- lossless client resume
- reconciliation after a restart
- audit evidence

It is **not** an order book and not a source of truth for order state; the broker is. It is a new durable format, accepted on 2026-10-04.

### Layout

```text
<state-root>/                        e.g. ~/.local/state/order-gateway/alpaca-paper-1/
  lock                               exclusive flock; a second process refuses to start
  KILL                               optional operator sentinel
  journal/<YYYY-MM-DD>.jsonl         one file per UTC day, append-only
```

The state root is a durable root. It needs exact-path authorization per task, is never `/tmp` or `/Volumes/Data`, is never deleted recursively, and tests use disposable roots.

### Line grammar

Each line is the complete wire event ([section 7](#7-events)), including `schema` and `type`, with `prev` appended as its last field:

```json
{"schema":"tinytrader.gateway/1","type":"event","seq":1302,"ts_gateway":1791221400123456789,"account":"alpaca-paper-1","kind":"order", …, "prev":"9f2c…(64 hex)"}
```

- `prev` is the lowercase hex SHA-256 of the complete previous line's bytes, including its `\n`.
- The account's first line uses 64 zeros.
- The first line of each later day is `kind:"day"`, carrying `format:"tinytrader.gateway-journal/1"`, the previous file name and its last line's hash. Its `prev` is that hash, so the chain crosses days.
- The line's bytes are exactly the event bytes sent to clients plus the trailing `prev` field. Clients receive the line without `prev`.

### Write rule

Each line is written and fsync'd (`F_FULLFSYNC` on macOS) **before** the side effect or delivery it announces:
- `submit{phase:"sent"}` is fsync'd before the HTTP bytes leave.
- Every event is fsync'd before it is queued to any client.

Creating a new file also fsyncs the directory.

### Recovery at start

1. Take the lock.
2. Read the files in order. A torn final line, with no `\n`, is truncated; this is the only repair allowed.
3. Verify the hash chain and the `seq` continuity. A mismatch anywhere else is fatal: the process refuses to start and leaves the files untouched.
4. Rebuild the map `client_order_id → last known state, intent_sha256` and the reserved-id set from the last 30 days.
5. Enter `reconciling`.
   - Fetch every order that is non-terminal or `unknown` by its client id.
   - Then `GET /v2/orders?status=all&after=<last event ts − 60 s>&nested=true`, plus `FILL` activities since then.
6. Emit `order` events with `source: rest_reconcile` for every state change not yet journaled, then `status{resync}`, then `ready`.

The same resync runs after every `trade_updates` reconnect. An order in phase `sent` without an outcome is treated exactly like `unknown`: the gateway looks it up and never resends it.

### Retention

- Every day stays on disk. The gateway never deletes journal files; the volume is a few KB per day.
- The resume window is the current UTC day plus the previous trading day. Older resumes get `resume: truncated`.
- Ids are reserved for 30 days.
- Shipping sealed days to an archive is deferred.

## 12. Error codes

**Connection errors** (`type:"error"`, then the connection closes):

| Code | Cause |
|---|---|
| `bad_json`, `line_too_long` | Framing error |
| `hello_required`, `hello_timeout` | The first line was not `hello`, or did not arrive within 5 s |
| `schema_unsupported` | Unknown `schema` |
| `unauthorized` | Bad token, or a client not bound to this account |
| `slow_consumer` | The outbound queue overflowed. Resume to recover. |
| `idle_timeout` | Peer silent for 30 s |
| `shutting_down` | The process is stopping |

**Request errors** (`response.outcome:"error"`; the connection stays open):

| Code | Cause |
|---|---|
| `invalid_request` | Unknown type or field, or a wrong JSON type, with `detail.path` |
| `duplicate_request_id` | Reused within the connection |
| `forbidden` | Outside the client's role or prefix (`kill`, `unkill` and `flatten` need the admin role) |
| `not_found` | `cancel` target unknown to the journal and the broker |
| `broker_error` | A query failed, with `http_status`, `code` and `message` |
| `throttled`, `not_ready` | A query was refused by the budget or the state |
| `unsupported` | The operation is not available for this broker kind |

**`not_sent.reason`** for `submit` and `cancel`:
- `validation` (with `detail.path`)
- `unsupported`
- `prefix`
- `cap` (with `detail.cap`)
- `killed`
- `not_ready`
- `throttled`
- `cutoff`
- `duplicate_conflict`

## 13. Worked examples

These follow Reactor's two exact shapes. Lines are wrapped here for reading; on the wire each one is a single line.

**Hello, with resume:**

```json
{"schema":"tinytrader.gateway/1","type":"hello","client":"reactor","token":"<redacted>","resume_after_seq":1288}
{"schema":"tinytrader.gateway/1","type":"welcome","account":"alpaca-paper-1","broker":"alpaca","environment":"paper",
 "state":"ready","last_seq":1301,"resume":"full","capabilities":{"shapes":["single","combo"],"max_legs":4,
 "client_order_id_max":128,"idempotency":"broker_client_id"},"pins":{"profile_sha256":"…"},"limits":{"budget_per_min":150}}
```

**A single limit that opens and then fills.** The Alpaca body sent is `{"symbol":"SPY261005C00580000","qty":"1","side":"buy","type":"limit","limit_price":"1.25","time_in_force":"day","client_order_id":"wse-20261005-0001","position_intent":"buy_to_open"}`.

```json
{"schema":"tinytrader.gateway/1","type":"submit","request_id":"r-17","intent":{"client_order_id":"wse-20261005-0001",
 "shape":"single","instrument":{"class":"option","symbol":"SPY261005C00580000"},"side":"buy","effect":"open",
 "qty":1,"type":"limit","limit_price":"1.25","time_in_force":"day"}}
{"schema":"tinytrader.gateway/1","type":"response","request_id":"r-17","op":"submit","outcome":"accepted",
 "client_order_id":"wse-20261005-0001","broker_order_id":"b3c1…","status":"accepted","broker_raw":{…}}
{"schema":"tinytrader.gateway/1","type":"event","seq":1305,"ts_gateway":…,"account":"alpaca-paper-1","kind":"order",
 "client_order_id":"wse-20261005-0001","broker_order_id":"b3c1…","event":"new","status":"new","ts_broker":…,
 "filled_qty":0,"avg_price":null,"source":"stream","broker_event":"new","broker_raw":{…}}
{"schema":"tinytrader.gateway/1","type":"event","seq":1306,…,"kind":"order","client_order_id":"wse-20261005-0001",
 "event":"fill","status":"filled","fill":{"execution_id":"…","qty":1,"price":"1.24"},"filled_qty":1,
 "avg_price":"1.24","position_qty":1,"source":"stream","broker_event":"fill","broker_raw":{…}}
```

**A two-leg MLEG credit spread, partly filled and then cancelled.** The Alpaca body is `{"order_class":"mleg","qty":"3","type":"limit","limit_price":"-0.40","time_in_force":"day","client_order_id":"wse-20261005-0002","legs":[{"symbol":"SPY261005P00570000","ratio_qty":"1","side":"sell","position_intent":"sell_to_open"},{"symbol":"SPY261005P00568000","ratio_qty":"1","side":"buy","position_intent":"buy_to_open"}]}`.

```json
{"schema":"tinytrader.gateway/1","type":"submit","request_id":"r-18","intent":{"client_order_id":"wse-20261005-0002",
 "shape":"combo","legs":[
   {"instrument":{"class":"option","symbol":"SPY261005P00570000"},"ratio":1,"side":"sell","effect":"open"},
   {"instrument":{"class":"option","symbol":"SPY261005P00568000"},"ratio":1,"side":"buy","effect":"open"}],
 "qty":3,"type":"limit","limit_price":"-0.40","time_in_force":"day"}}
{"schema":"tinytrader.gateway/1","type":"response","request_id":"r-18","op":"submit","outcome":"accepted",…}
{"schema":"tinytrader.gateway/1","type":"event","seq":1310,…,"kind":"order","client_order_id":"wse-20261005-0002",
 "event":"partial_fill","status":"partially_filled","fill":{"execution_id":"…","qty":1,"price":"-0.41"},
 "filled_qty":1,"avg_price":"-0.41","legs":[
   {"symbol":"SPY261005P00570000","filled_qty":1,"avg_price":"0.93","fill":{"qty":1,"price":"0.93"}},
   {"symbol":"SPY261005P00568000","filled_qty":1,"avg_price":"0.52","fill":{"qty":1,"price":"0.52"}}],
 "source":"stream","broker_event":"partial_fill","broker_raw":{…}}
{"schema":"tinytrader.gateway/1","type":"cancel","request_id":"r-19","client_order_id":"wse-20261005-0002"}
{"schema":"tinytrader.gateway/1","type":"response","request_id":"r-19","op":"cancel","outcome":"accepted"}
{"schema":"tinytrader.gateway/1","type":"event","seq":1312,…,"kind":"order","client_order_id":"wse-20261005-0002",
 "event":"canceled","status":"canceled","filled_qty":1,"avg_price":"-0.41",…}
```

**Unknown, then resolved.** The POST times out after the bytes were sent. The gateway does not send a second POST.

```json
{"schema":"tinytrader.gateway/1","type":"response","request_id":"r-20","op":"submit","outcome":"unknown","reason":"timeout",
 "client_order_id":"wse-20261005-0003"}
{"schema":"tinytrader.gateway/1","type":"event","seq":1320,…,"kind":"resolution","client_order_id":"wse-20261005-0003",
 "outcome":"found","window_ms":2000}
{"schema":"tinytrader.gateway/1","type":"event","seq":1321,…,"kind":"order","client_order_id":"wse-20261005-0003",
 "event":"new","status":"new","source":"rest_reconcile",…}
```

**Refusals before any I/O:**

```json
{"schema":"tinytrader.gateway/1","type":"response","request_id":"r-21","op":"submit","outcome":"not_sent","reason":"cutoff",
 "detail":{"expiry":"2026-10-05","cutoff_et":"15:29:30","leg":"SPY261005C00580000"}}
{"schema":"tinytrader.gateway/1","type":"response","request_id":"r-22","op":"submit","outcome":"duplicate",
 "current":{"client_order_id":"wse-20261005-0001","status":"filled",…}}
```

### Reactor interface coverage

| Reactor method or contract item | Gateway |
|---|---|
| `account`, `positions`, `open_orders`, `clock`, `calendar`, `activities` | Requests with the same names. `broker_raw` keeps Reactor's parsers. |
| `order_by_client_id` | `order{client_order_id}` |
| `submit`, `cancel` | `submit`, `cancel`, with the outcomes in [section 5](#5-requests-and-responses) |
| 4 Hz order polling | `order` events (it can be retired) |
| B1 shapes, B2 idempotency, B3 events | Sections 6, 5 and 7 |
| B4 cancel | `cancel` (replace is reserved) |
| B5 account state | Queries plus `account` and `activity` events |
| B6 venue rules | Sections 6 and 9 |
| B7 safety | Sections 8 and 9 |
| B8 credentials | Section 1. Reactor deletes its own key file at cut-over. |
| FakeBroker scenarios | Simulator scripted mode (section 10) |
| C1 versioning | Section 2 |
| C2 test harness (a loopback fake Alpaca) | Simulator scripted mode through the real gateway, plus a loopback fake Alpaca (REST and `trade_updates`) in the gateway's tests (O3 dispatch). |

## Running `order-gateway`

```text
order-gateway --account sim-1 --broker sim --listen 127.0.0.1:7601 --state ~/.local/state/order-gateway/sim-1 \
  --client reactor,trade,wse-,~/.config/tinytrader/gateway-clients/reactor.token \
  --client watch,observe,,~/.config/tinytrader/gateway-clients/watch.token \
  [--sim-script FILE] [--sim-quotes DBN_FILE|tcp:HOST:PORT] \
  [--underlyings SPY] [--any-underlying-client NAME ...] [--equities AAPL,..] [--allow-short] [--max-shares 1000] [--max-qty 10] [--max-open 4] [--max-legs 4] [--max-notional 2500] \
  [--submits-per-min 60] [--submits-per-day 500] [--max-rejects 5] [--budget 150] [--min-options-level 3] \
  [--resolve-ms 1000,2000,4000]
```

- Exit codes: 0 after SIGINT/SIGTERM (clients receive `shutting_down`), 1 for a runtime failure (lock held, journal chain broken, bind failure), 2 for usage (including a non-loopback `--listen`, a state root under `/tmp` or `/Volumes/Data`, or a token file that is not mode 600 or is a symlink).
- The process needs no feed, relay, replay or data-gateway process. Market mode reads only the quote source it is given.
- To stop new orders without a client, create `<state>/KILL`; the gateway enters `halt_all` within a second. Only an admin `unkill` clears it, and the sentinel must be removed first.
- `--sim-script` is JSON. Every key is optional: `clock_ts` (a fixed clock moved only by `advance`; omitted means the wall clock), `calendar` (omitted: weekdays 09:30 to 16:00 ET; `null`: none, so opening cutoffs fail closed; or a list of `{date, open, close}`), `account` (`status`, `options_trading_level`, `cash`, `blocked`), `positions`, `orders` (preloaded broker orders), `quotes` (scripted fill prices), `default` and `rules` (each matching `nth` or `client_order_id`, with `submit`, `cancel`, `fill_on_query` and `skew`), `stream` (`drop_after_events`, `down_ms`), `fee_per_contract`, `latency_ms` and `size_cap`.
- `--broker alpaca-paper --credentials env` serves one Alpaca paper account with keys from `APCA_API_KEY_ID` and `APCA_API_SECRET_KEY` (or their `*_FILE` forms), decrypted by sops into the process only, as `tinytrader-ops`' `local-up` does. `--credentials-file ~/.config/tinytrader/alpaca-paper-N.env` serves one from a file. The file is mode 600, a regular file, not a symlink, and holds `APCA_API_KEY_ID`/`APCA_API_SECRET_KEY` (or the `ALPACA_*` spellings); values are never printed. The endpoints are compiled in (`paper-api.alpaca.markets`); there is no URL option and no live endpoint: live trading needs a separate operator decision. `--sim-*` options are refused with it.
- `--sim-quotes` takes a DBN v1/v3 file or a `tcp:` stream such as `replay dbn --listen`. Definitions, `cmbp-1` and `mbp-1` are read; other records are skipped. Market time starts at the first record and moves only with `advance`.

## Simulated stock orders

The simulated broker fills a stock leg like an option leg, at ×1: in market mode against `mbp-1` quotes for the stock. It does not model the closing auction, so a `cls` order there behaves as a `day` limit order.

## Remaining work

- **Paper qualification** on Paper 3 (authorized by the operator 2026-10-08): a single far from the market cancelled; a 2-leg spread cancelled; a 1-share SPY limit-on-close round trip; `halt_all` with a resting order; a restart while an order rests (reconciliation, no resend); capture of the MLEG fill shape.
- **Clients:** the post-earnings drift strategy (`tinytrader-ops/strategies/pead`) moves onto the gateway; Reactor's planned cut-over targets this gateway instead of Hyphae's.
- **Live:** no live endpoint exists. Adding one needs a separate operator decision.

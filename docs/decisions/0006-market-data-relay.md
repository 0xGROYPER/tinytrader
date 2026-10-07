# 0006: The market data relay

- **Status:** Accepted
- **Date:** 2026-10-05

## Context

A provider usually allows few live connections. Alpaca allows one market data
stream per account; Databento allows 10 sessions per dataset on Standard
plans, 50 on Plus and Unlimited. Several local programs want the same live
data at once: strategies, research notebooks and the console. A **relay**
holds the provider connection and serves any number of local clients.

Hyphae has a working relay. This decision describes tinytrader's replacement,
informed by it but simpler:

- It **only forwards.** It keeps no recording; durable capture belongs to the
  store service ([0005](0005-store-service-and-supervision.md)) and to
  historical downloads.
- A client that joins late **catches up from the provider,** not from the
  relay. Alpaca serves the current day's IEX trades and quotes through its
  historical API without delay, and Databento's live API replays up to the
  last 24 hours from a requested start.
- It **reuses Databento's live conventions** (DBN v3, `SystemMsg` and
  `ErrorMsg` codes, symbol mapping records) instead of a private grammar, so a
  client written for Databento's live API needs few changes.

## Scope

First version: **Alpaca IEX** (US equities trades and quotes) and **Databento
OPRA** (`OPRA.PILLAR`, CMBP-1 and definitions). Crypto venues come later, with
their capture work.

Each relay instance serves one port and holds one provider connection. The
first deployment runs:

| Relay | Symbols | Provider sessions |
|---|---|---|
| IEX | `SPY`, `QQQ` | Alpaca's single stream |
| OPRA SPY | `SPY.OPT` | 1 Databento session |
| OPRA QQQ | `QQQ.OPT` | 1 |
| OPRA XSP | `XSP.OPT` | 1 |
| OPRA index | `SPX.OPT`, `SPXW.OPT` | 1 |

Splitting OPRA by root lets a client take only the roots it wants without the
relay filtering anything. That is four Databento sessions in all, however
many clients connect, within Databento's 10 per dataset on Standard plans.
Symbols are configuration, so adding more is a deployment change.

Out of scope for now: recording, caching market data, filtering by
instrument or expiry, and quote conditions.

## Protocol (`tinytrader.relay/1`)

### Connection

TCP, one port per relay. The client speaks first, with one line of JSON:

```json
{"protocol": "tinytrader.relay/1", "start": "2026-10-05T13:30:00Z", "conditions": true}
```

| Field | Meaning |
|---|---|
| `protocol` | Required. A relay refuses versions it doesn't serve. |
| `start` | Optional. Replay from this time, then continue live. Absent means live from now. |
| `conditions` | Optional, IEX only. Interleave a trade conditions record after each trade. |
| `client` | Optional. The client's name, such as `xsp-strategy` (up to 64 letters, digits, `.`, `_`, `-` or spaces). Added 2026-10-07. |
| `account` | Optional. The Alpaca account the client trades, by its name in the deployment, such as `paper-1`. Added 2026-10-07. |

Each relay lists who is connected at `/clients` (name, account, address, since when, live or replaying, with or without conditions), and the console draws named clients and the accounts they trade.

Requiring the client to speak first removes Hyphae's 250 ms wait on every
connection. A malformed or refused request gets an `ErrorMsg` and the
connection closes.

### Stream

The relay answers with a DBN v3 stream: a metadata header (dataset, schema,
`start` set to the effective start, symbols), then records.

- **Symbols.** After the header, one `SymbolMappingMsg` per symbol, as
  Databento's live API sends. For IEX, the relay assigns instrument ids, stable
  for a symbol across connections and restarts.
- **Records.** OPRA: the provider's bytes unchanged. IEX: `MBP-1`, encoded
  the way Databento normalizes a single exchange's top of book, which is the
  reference wherever another source differs:
  - Quotes update the book; trades are `MBP-1` records with action `T`
    carrying the book as it stood before the trade.
  - `ts_event` is the exchange's timestamp; `ts_recv` is when the relay
    received the websocket frame, the same for every message in a frame.
  - `F_LAST` marks the last record of each event.
  - Sizes are in shares and prices are fixed-point as DBN defines them; the
    units Alpaca reports are checked, not assumed.
  - Rows replayed from history have no true receive time: they carry
    `ts_recv = ts_event` and DBN's `F_BAD_TS_RECV` flag. The same encoder
    serves live and history.
- **Trade conditions.** Standard DBN has no field for trade condition codes.
  A client that asks with `"conditions": true` receives a tinytrader extension
  record, `tinytrader.trade-conditions/1`, immediately after each trade, holding
  that trade's conditions. Other clients receive only standard DBN.
- **Catch-up.** When `start` is in the past:
  - IEX: the relay pages Alpaca's historical trades and quotes from `start`,
    merges them by event time and encodes them exactly as live data, then
    switches to the live tail. Rows before the first live message come from
    history; that message and everything after comes from the live stream.
  - OPRA: nothing is replayed per client. Databento refuses intraday replay
    for `cmbp-1` (tested 2026-10-05; it does replay `trades`, `tcbbo`,
    `cbbo-1s`, `cbbo-1m`, `definition` and `statistics`), so missed quotes
    come from the next day's historical data. If that becomes a problem, this
    decision is reopened: replaying `tcbbo` and `cbbo-1s` for the missed
    stretch is the first option.
- **Reference data.** The relay keeps the latest instrument definitions and
  statistics (open interest and the like, some published before the open) in
  memory: it replays them from Databento when its own session starts, keeps
  them current from the live feed, and drops instruments once they expire.
  Every client receives them as soon as it joins, before any live data, so
  any number of clients share one provider session. Market data itself is
  never cached.
- A `SystemMsg` with code `ReplayCompleted` marks the end of what the relay
  sends on joining (reference data, and for IEX any catch-up) and the start of
  live data.

### System and error records

| Situation | Record |
|---|---|
| No data for 10 s | `SystemMsg`, code `Heartbeat` |
| Catch-up finished, now live | `SystemMsg`, code `ReplayCompleted` |
| Provider connection lost | `SystemMsg`, text `upstream-down since=<ns>`; heartbeats continue |
| Provider connection restored | `SystemMsg`, text `upstream-up gap_from=<ns> gap_to=<ns>` |
| Provider skipped records (the relay read too slowly; the connection stayed up) | `SystemMsg`, text `upstream-gap gap_from=<ns> gap_to=<ns>`, before the first record after the skip. The provider's own slow-reader error and warning are not passed on: they use the codes the relay sends a client it drops. |
| Outside active hours (provider disconnected on purpose) | `SystemMsg`, text `off-schedule until=<ns>`; heartbeats continue |
| Active hours begin | `SystemMsg`, text `on-schedule` (not a gap) |
| Client falling behind (queue half full) | `SystemMsg`, code `SlowReaderWarning` |
| Client too slow (queue full) | `ErrorMsg`, then the connection closes |
| Malformed or unsupported request | `ErrorMsg`, code `InvalidSubscription`, then close |

All codes are the ones DBN v3 defines for Databento's live API.

After `upstream-up`, a client that needs the gap filled reconnects with
`start` set to `gap_from`. The relay does not fill gaps by itself.

### Flow control

Each client has a bounded queue (default 64 MiB). A client that falls behind
is disconnected rather than slowing the provider connection or other clients.

## Operation

- A tinytrader service with `/health`, `/metrics` and `/describe`. The console
  shows the relay, its provider, and its clients.
- **Active hours** (decision 0005): a daily window in New York time set
  relative to the regular session, so early closes shorten it (this
  deployment: OPRA 09:25–16:20, IEX 07:55–17:05, IEX itself trading
  08:00–17:00). Trading days and early closes come from the market calendar
  the accounts service serves. Outside the window the relay disconnects from
  its provider, reports itself idle ("off schedule until …"), and is left
  out of the console's graph.
- Configuration by flags and environment, with credentials from files
  (`*_FILE`), like tinytrader's other services.
- Implementation: tokio, the official `databento` and `dbn` crates (including
  their reconnect handling), and `apca` for Alpaca's stream and history.

## Migration

1. Build and test the relay against recorded data and the providers.
2. Run OPRA side by side with Hyphae's relay on different ports (Databento
   allows several sessions) and compare.
3. Hyphae and Reactor update their clients to `tinytrader.relay/1`.
4. Switch IEX in one move, since Alpaca allows a single stream: stop Hyphae's
   IEX relay, start tinytrader's.
5. Retire Hyphae's relays and their recordings.

## Resolved

- Databento intraday replay is assumed to cost nothing extra on the plan.
- OPRA quotes are not replayed, since Databento can't replay `cmbp-1`; the
  relay holds definitions and statistics and sends them to every joining
  client. Reopen if missed quotes become a problem.
- Trade conditions are an opt-in tinytrader extension record.
- IEX encoding follows Databento's MBP-1 conventions, with replayed rows
  flagged `F_BAD_TS_RECV`.
- IEX instrument ids are the relay's own and need not match Hyphae's.
- Symbols as in the table above.

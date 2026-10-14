# Market data relay API

A relay holds one connection to a market data provider and serves it to any number of local clients. It has two interfaces:

- **The stream** (`tinytrader.relay/1`): DBN records over TCP, for programs that consume market data. One per relay, on `RELAY_LISTEN` (for example `7600` for SPY options, `7602` for XSP).
- **HTTP** on `RELAY_OBSERVE_LISTEN` (`9110` inside the deployment): health, what it carries, and views derived from the records it forwards. The console reaches these as `/api/stream/{relay}/{stream}` (bars also as `/api/bars/{relay}`).

The relay forwards and keeps no recording ([decision 0006](decisions/0006-market-data-relay.md)). Everything below the stream is held in memory and is gone after a restart.

| Relay | Source | Carries | Stream port |
|---|---|---|---|
| `relay-opra-spy` | Databento `OPRA.PILLAR`, `cmbp-1` | `SPY.OPT` (every SPY option) | 7600 |
| `relay-opra-xsp` | Databento `OPRA.PILLAR`, `cmbp-1` | `XSP.OPT` | 7602 |
| `relay-iex` | Alpaca IEX, as `ALPACA.IEX` `mbp-1` | SPY, QQQ | 7610 |

Relays run during their hours (`RELAY_HOURS`, such as `09:25-16:20` New York on trading days for options); outside them the provider session is closed and clients get heartbeats only.

## The stream: `tinytrader.relay/1`

Connect over TCP and send one line of JSON:

```json
{"protocol": "tinytrader.relay/1", "name": "xsp-strategy", "account": "paper-1", "start": "2026-10-12T13:30:00Z", "conditions": false}
```

| Field | | |
|---|---|---|
| `protocol` | required | Exactly `tinytrader.relay/1`; another version is refused. |
| `name` | optional | Who is connecting, shown on the console: up to 64 letters, digits, `.`, `_`, `-` and spaces. |
| `account` | optional | The account the client trades, by its deployment name, shown beside it. |
| `start` | optional | RFC 3339. Replay from then, then continue live; absent means live from now. |
| `conditions` | optional | Interleave a `tinytrader.trade-conditions/1` record after each trade (IEX). |

The relay answers with a DBN v3 stream:

1. The metadata header.
2. Reference records: instrument definitions, and statistics where the source has them.
3. Catch-up from `start`, where the source supports it:
   - **IEX:** Alpaca's historical trades and quotes from `start`, merged by event time and encoded exactly like live data.
   - **OPRA:** none. Databento won't replay `cmbp-1` intraday, so the client is live from joining and the header's `start` says so. Today's quotes for one contract are available from [`/session`](#session-todays-replay-of-one-contract).
4. A `SystemMsg` with code `ReplayCompleted`.
5. Live records.

**In-stream markers** (`SystemMsg` text):

| Marker | Meaning |
|---|---|
| `upstream-down since=<ns>` | The provider connection dropped. |
| `upstream-up gap_from=<ns> gap_to=<ns>` | It's back; records between the two times were missed. |
| `upstream-gap gap_from=<ns> gap_to=<ns>` | The provider skipped records because the relay read too slowly. |
| Heartbeat (`SystemMsg` code) | Sent after `RELAY_HEARTBEAT` seconds without data. |

A client that falls behind is warned at half its queue (`SystemMsg` code `SlowReaderWarning`) and, when it falls `RELAY_CLIENT_QUEUE` chunks of up to 64 KiB behind (default 1024), sent `ErrorMsg` `SkippedRecordsAfterSlowReading` and disconnected, so it never slows the provider connection or other clients.

## HTTP

All answers are JSON. Times are Unix seconds unless noted; prices are in dollars.

### `GET /health`, `/metrics`, `/describe`

The common service endpoints: state (`ready`, `degraded`, ...), Prometheus metrics, and what the relay is (role, upstream, the streams listed here, its symbols and schedule).

### `GET /events?after=<n>&limit=<n>`

The relay's recent events (the last 500): provider up and down, gaps, schedule changes. Newest last; `after` is the last sequence number seen.

### `GET /recent`

The most active instruments' trade prices, sampled once a second, for the console's price panels.

### `GET /clients`

Who is connected: `{"clients": [{client, account, peer, since, replay, conditions}]}`: the name and account it gave, its address, when it joined, whether it asked to replay, and whether it takes trade conditions.

### `GET /bars?symbol=<symbol>&days=<1-10>`

One-minute bars, `{"symbol", "interval": 60, "bars": [[t, o, h, l, c, v], ...], "note"}`, `t` the minute's start.

- **The relay's own symbols** (such as `SPY` on the IEX relay): built from the trades it relays, with history before the relay saw them where the source keeps it (Alpaca's).
- **An option on an OPRA relay** (`symbol` in compact OCC, such as `SPY261012C00777000`): today's minutes of the contract's **mid**, from [`/session`](#session-todays-replay-of-one-contract)'s `cbbo-1m`. Each bar closes at the mid at the end of its minute and opens at the previous minute's, so high and low are the larger and smaller of the two; volume is 0. `days` is ignored.

### `GET /quotes`

The latest consolidated quote of each contract the relay carries, from the live `cmbp-1` (or IEX's `mbp-1`).

- `?symbols=SPY261016C00775000,...` for those contracts, or `?underlying=SPY&from=YYYY-MM-DD&to=YYYY-MM-DD` for a root's contracts expiring in that range.
- `{"quotes": {OCC: {bid, ask, bid_size, ask_size, ts}}, "as_of", "last_record"}`: compact OCC symbols, `null` for an empty side, `ts` the provider's receive time.
- A quote changes only when its book does, so a quiet contract's can be old and still current. `last_record` (null before any) says whether the feed itself is live; alpaca-data uses quotes only while it's within 120 seconds.

### `GET /stats`

Each contract's **open interest**, from the day's `statistics` the relay holds for joining clients (OPRA publishes it once a day, before the open, for the previous close). Same selection as `/quotes` (`symbols=` or `underlying=` with `from`/`to`).

```json
{"stats": {"SPY261016C00775000": {"open_interest": 1234, "date": "2026-10-12"}}, "as_of": 1791830000.1}
```

`date` is the trading day the figure is for. alpaca-data puts it on SPY and XSP chain contracts (`open_interest`, and the chain's `open_interest_date`), which Candles' GEX levels use.

### `GET /tape`

Time and sales for the relay's own stock symbols (IEX's `mbp-1`; OPRA's quotes aren't kept): the last 4,000 trades and 4,000 changes to the top of book of each, in memory.

- `?symbol=SPY&after=<seq>` for the ticks after a sequence number, or `?symbol=SPY&limit=<n>` (at most 1000, default 500) for the latest.
- `{"symbol", "ticks": [{s, t, k, b, a, bz, az, p?, z?}], "last", "symbols", "last_record"}`, oldest first: `s` the sequence number, `t` the event time, `k` `T` (a trade, with `p` and `z`, its price and size) or `Q` (a quote change); `b`, `a`, `bz`, `az` the book, before the trade for a trade. `last` is the newest sequence number held, for the next `after`; a smaller one than before means the relay restarted.

### `GET /session`: today's replay of one contract

```
GET /session?symbol=SPY261012C00777000&schema=cbbo-1m&start=2026-10-12T13:30:00Z
```

Today's data for one option contract, from Databento's intraday replay. Each request opens a short second live session for just that contract, subscribed from `start`, reads until Databento reports the replay complete, and closes it.

| Parameter | | |
|---|---|---|
| `symbol` | required | Compact OCC, on one of this relay's roots (SPY options on `relay-opra-spy`). Anything else is `404`. |
| `schema` | `cbbo-1m` | `cbbo-1m` or `cbbo-1s` (consolidated quote each minute or second), `tcbbo` (every trade with the quote before it) or `trades`. These are what Databento replays intraday for OPRA; `cmbp-1` it won't. |
| `start` | today's 09:30 New York | RFC 3339, within the last 24 hours (Databento's replay limit). Before 09:30 the default is the last 24 hours. |

```json
{"symbol": "SPY261012C00777000", "schema": "cbbo-1m", "start": 1791811800,
 "fields": ["t", "bid", "ask", "bid_size", "ask_size", "last"],
 "rows": [[1791811860, 2.77, 2.78, 21, 1, 2.73], ...], "note": null}
```

| `schema` | `fields` |
|---|---|
| `cbbo-1m`, `cbbo-1s` | `t` (end of the interval), `bid`, `ask`, `bid_size`, `ask_size`, `last` (the session's last trade) |
| `tcbbo` | `t` (seconds with nanoseconds), `price`, `size`, `side`, `bid`, `ask` |
| `trades` | `t`, `price`, `size`, `side` |

`side` is `A` (sell aggressor), `B` (buy) or `N` (unknown, usual for OPRA). `note` is set when Databento ended the session before the replay completed.

- **Limits:** one replay runs at a time per relay, so a relay holds at most two of the dataset's live sessions (Databento allows 10 per dataset on Standard). Answers are kept for a minute. A replay taking over 30 seconds fails with `502`.
- **Sizes:** a busy at-the-money 0DTE contract at midday is about 250 `cbbo-1m` rows and 20,000 `tcbbo` rows.
- **Session and history:** this serves the current session, held in memory. Past days are history: bought from Databento as batch files, filed in the store and read from disk (the Data page's jobs, which quote a price before any purchase).
- **Cost:** intraday replay is assumed to be included in the live plan (decision 0006); confirm against Databento's usage page.

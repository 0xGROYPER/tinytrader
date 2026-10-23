# Observe endpoints (`tinytrader.observe/1`)

| | |
|---|---|
| **Purpose** | What every service serves about itself, so a console, a terminal UI, Prometheus or a supervisor can find, draw and check it without configuration. |
| **Transport** | HTTP/1.1, JSON (Prometheus text for `/metrics`), on the service's listen address: its own variable (such as `STORE_LISTEN`), else `TINYTRADER_LISTEN`, else its default. |
| **Versioning** | `"schema": "tinytrader.observe/1"` in `/health` and `/describe`, and the `tinytrader_observe_schema_version` gauge. Additive only: fields and metrics are never renamed or removed. |
| **Auth** | None. These are reads. |
| **Errors** | None of their own; `/health` answers `503` when the service isn't `ready`. |
| **Health** | This is it. |
| **Stability** | **Stable**: `/health`, `/metrics` and the `/describe` fields below. `/events` and `/recent` are **stable** in shape; their contents are each service's own. |

Implemented once, in [`libs/observe`](../../libs/observe/src/lib.rs). Every service in `services/` serves `/health`, `/metrics` and `/describe`; the order gateway doesn't yet ([decision 0017](../decisions/0017-client-contracts.md)).

## `GET /health`

```json
{"schema": "tinytrader.observe/1", "service": "store", "state": "ready", "detail": "2 volumes; fullest ok",
 "build": "0.1.0", "started_at": 1791200000.12, "uptime_seconds": 3600.5,
 "last_error": null, "last_error_at": null}
```

| Field | |
|---|---|
| `state` | `starting`, `ready`, `degraded`, `recovering`, `stopping`, `failed` or `idle` (deliberately inactive, such as outside its hours: not a fault). |
| `detail` | One human-readable line on what it's doing. |
| `started_at`, `last_error_at` | Unix seconds (float). |
| `last_error` | The most recent error, kept after the service recovers. |

The status is `200` only when `state` is `ready`, and `503` otherwise, including `idle`. A client that wants to tell "off on purpose" from "broken" reads `state`, not the status code (the console and a deployment's yield-to-capture checks do).

## `GET /metrics`

Prometheus text (`text/plain; version=0.0.4`). Every service has:

| Metric | |
|---|---|
| `tinytrader_observe_schema_version` | `1`. |
| `process_start_time_seconds` | Start time, Unix seconds. |
| `tinytrader_ready{service}` | `1` when `ready`, else `0`. |

Service metrics are named `tinytrader_<service>_<what>` with units as suffixes (`_seconds`, `_bytes`, `_total` for counters). Labels stay low cardinality: service, venue, product, account, environment; never order or instrument ids.

## `GET /describe`

What the service is and what else it serves, so it can be drawn and used without configuration. `schema`, `service` and `build` are always present; the rest is optional:

| Field | |
|---|---|
| `role` | One sentence on what this instance does. |
| `upstream` | Sources it reads: `{id, label, sub, kind, via}` for something outside the deployment, `{service, kind, via}` for another service in it. `kind` is `flow` (data) or `read`. |
| `streams` | Paths of its other endpoints, by **capability name**, such as `{"events": "/events", "series": "/recent", "chain": "/chain"}`. |
| `stores` | Places it writes: `{id, label, sub, metric}`, `metric` naming a bytes-written metric. |
| `panels` | Suggested charts over `series`: `{heading, note, decimals, footer}`. |
| `schedule` | What it runs and when: `{name, kind, when}`, `kind` one of `always`, `window`, `interval`, `daily`, `event`, `yield`, optionally with `every_seconds`, `hours`, `class`, `yields_to`. |
| `relay` | A market data relay's stream: `{protocol, listen, symbols}`. |

### Capability names in `streams`

A front end or client finds a service by the capability it declares, never by its id. The names in use:

| Name | Served by | Endpoint |
|---|---|---|
| `events` | most services | the event log (below) |
| `series` | relays, ticker, alpaca-account | `/recent` (below) |
| `catalog` | store | [`/catalog`](store.md) |
| `accounts`, `calendar`, `performance`, `trades` | alpaca-account | [account endpoints](alpaca-account.md) |
| `news` | alpaca-news | [`/news`](alpaca-news.md) |
| `bars`, `daily`, `snapshots`, `chain`, `chain_index`, `news_search`, `option_bars`, `option_contracts`, `screener` | alpaca-data | [market data endpoints](alpaca-data.md) |
| `clients` | relays | [`/clients`](relay.md#get-clients) |
| `econ`, `earnings`, `earnings_history`, `tickers` | econ-calendar | [calendar endpoints](econ-calendar.md) |
| `jobs` | databento-jobs | [`/jobs`](databento-jobs.md) |
| `runs` | alpaca-history | [`/runs`](alpaca-history.md) |

A new capability is a new name; an existing name keeps its response shape.

## `GET /events?after=<seq>&limit=<n>`

A bounded log of recent events (by default the last 500), for live tails.

```json
{"events": [{"seq": 41, "received_ms": 1791200000123.4, "summary": "upstream down", "raw": "..."}], "next": 41}
```

Newest last, at most `limit` (default 100). Pass the last `seq` seen as `after`; `next` is the newest sequence number, so a smaller one than before means the service restarted. `received_ms` is Unix **milliseconds**. `raw` is the source message as text (JSON for some services).

econ-calendar's `/events` is different: its calendar of releases (declared as `econ`, not `events`).

## `GET /recent`

Live series for charts: `{"<key>": [[unix_ms, value], ...]}`, the most active keys first, each a bounded number of samples. Times are Unix **milliseconds**.

## Being discovered

A service is found either way, and both lead to `/describe`:

- **Docker:** a container label `tinytrader.observe: "<port>"` ([fleet](fleet.md#docker-labels)).
- **Anything else:** an `observe` URL in the topology file ([fleet](fleet.md#the-topology-file)).

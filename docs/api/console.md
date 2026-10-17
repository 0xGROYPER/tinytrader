# Console API (`/api/*`)

| | |
|---|---|
| **Purpose** | What the console's page (and the terminal UI's console mode) reads and does: the deployment and its state, services' data passed through, and the few actions the console takes (order tickets, flatten, drive approval, jobs). |
| **Transport** | HTTP/1.1, JSON, on `CONSOLE_LISTEN` (default `127.0.0.1:8080`). |
| **Versioning** | `/api/topology` and `/api/state` carry `"schema": "tinytrader.fleet/1"` ([fleet](fleet.md)). The rest is unversioned and additive. |
| **Auth** | **None of its own**: anyone who can reach the port can use it, so bind to loopback or a private network. Writes must be JSON and same-origin (a page on another site gets `403`). Tokens for the services behind it (gateways, store) stay in the console and are never returned. |
| **Errors** | `{"error": "..."}` with a 4xx or 5xx status. Passed-through answers keep the upstream's status and body. |
| **Health** | The console's own [observe](observe.md) endpoints. |
| **Stability** | `/api/topology`, `/api/state`, `/api/events`, `/api/stream` are **stable**. The order, account, jobs and queue routes are **experimental**. Prefs, reminders and profiles are optional modules, **experimental**. |

## The deployment

| Endpoint | |
|---|---|
| `GET /api/topology` | The deployment as drawn ([fleet](fleet.md#apitopology)). |
| `GET /api/state` | The latest poll ([fleet](fleet.md#apistate)). With `?since=G&epoch=E`, only what changed since generation `G`: each service keeps only its changed fields plus `_fields` (all its field names), and the answer adds `generation`, `epoch` (the console's start, Unix ms) and `full`. |
| `GET /api/events/{service}?after=&limit=` | A service's [event log](observe.md), passed through. `404` if it declares none, `502` if it doesn't answer. |

## Data from services

| Endpoint | |
|---|---|
| `GET /api/stream/{service}/{stream}?...` | One of a service's on-demand streams, the query passed through, the answer returned as it is (30 s timeout). `stream` is one of `bars`, `daily`, `snapshots`, `chain`, `chain_index`, `news_search`, `earnings_history`, `option_bars`, `option_contracts`, `screener`, `tickers`, `quotes`, `session`, `stats`, `tape`, `performance`, `trades`. `404` if that service doesn't declare it. |
| `GET /api/bars/{service}?...` | The same as `/api/stream/{service}/bars`. |

Pick `{service}` by capability from `/api/topology` (the first service with a `chain` field serves `chain`), never by a fixed id.

## Orders and accounts (experimental)

The console reaches order gateways it's told about (`CONSOLE_GATEWAYS`: `account=host:port;...`), one connection per request, as two [gateway](order-gateway.md) clients:

| Client | Name (default) | Token | Used for |
|---|---|---|---|
| admin | `CONSOLE_GATEWAY_CLIENT` (`ops`) | `CONSOLE_GATEWAY_TOKEN` (or `_FILE`) | gateway state, Flatten, Resume |
| trade | `CONSOLE_TRADE_CLIENT` (`console`) | `CONSOLE_TRADE_TOKEN` (or `_FILE`) | the order ticket; ids start `CONSOLE_TRADE_PREFIX` (`tt-`) |

Names and prefix must match the gateway's `--client` options. Without the trade token the console sends no orders.

| Endpoint | |
|---|---|
| `GET /api/accounts/gateways` | `{"gateways": [{account, state, environment} \| {account, error}]}` |
| `GET /api/orders` | `{"trading": bool, "prefix": "tt-", "accounts": [{account, state, environment, broker, orders: [{client_order_id, status, filled_qty, avg_price, legs, limit_price, qty, submitted_at}]} \| {account, error}]}`: the console's own open orders. |
| `POST /api/orders/{account}` | `{"order": {legs: [{symbol, side, ratio?}], qty, type?, limit_price \| ref_price, effect?}, "confirm": "<account>", "live"?: true}`. Answers the gateway's `response` as it is (`accepted`, `rejected`, `not_sent`, `unknown`). A live account needs `live: true`. |
| `POST /api/orders/{account}/{client_order_id}/cancel` | `{}`. One of the console's own orders. |
| `POST /api/accounts/{account}/flatten` | `{"confirm": "<account>", "reason"?}`. Paper accounts only. |
| `POST /api/accounts/{account}/resume` | `{"reason"?}`: the gateway's `unkill`. Paper accounts only. |

`404` when the account has no gateway; `400` for a bad or unconfirmed order; `403` for flatten or resume on a live account (or one whose gateway can't say); `502` when the gateway fails.

## Jobs, queue and storage (experimental)

| Endpoint | |
|---|---|
| `GET\|POST /api/jobs/{path}` | Passed to `CONSOLE_JOBS` ([databento-jobs](databento-jobs.md)). |
| `GET\|POST\|PATCH /api/queue/{path}` | Passed to `CONSOLE_QUEUE` ([queue](queue.md)). |
| `POST /api/store/volumes/approve` | `{path, name}`, passed to `CONSOLE_STORE` with `CONSOLE_STORE_TOKEN` ([store](store.md)). |

`404` when the service isn't configured.

## Documents

| Endpoint | |
|---|---|
| `GET /api/collections` | `[{id, name, docs: [{index, path, group, title}]}]`, from `CONSOLE_COLLECTIONS` (`Name=dir,dir;Name=dir`). |
| `GET /api/collections/{id}/{index}` | `{index, path, title, html}`. |

## Optional modules: prefs, reminders, profiles

Each is off unless configured, and answers `404` then. They are per-deployment conveniences, not platform contracts ([decision 0017](../decisions/0017-client-contracts.md)).

| Endpoint | Configured by | |
|---|---|---|
| `GET /api/prefs`, `PUT /api/prefs/{key}` | `CONSOLE_PREFS` (and `CONSOLE_PREFS_DEFAULTS`) | The page's settings that follow a user between devices. `PUT` needs `If-Match` (the SHA-256 of the current value, or `absent`); `409` with `current` when it changed. |
| `GET /api/reminders`, `POST /api/reminders` | `CONSOLE_REMINDERS` (and `CONSOLE_REMINDERS_SEED`) | Lists and items; `POST` applies one change (`{op: add_item \| update_item \| delete_item \| add_list \| update_list \| delete_list, ...}`). |
| `GET /api/profiles`, `POST /api/profiles`, `GET /api/profiles/{id}` | `CONSOLE_PROFILES` | [Profile mode](../profile-mode.md) recordings. |

## Configuration

| Variable | |
|---|---|
| `CONSOLE_TOPOLOGY` | The [topology file](fleet.md#the-topology-file) (default `topology.json`). |
| `CONSOLE_DOCKER` | Docker Engine API base URL (a read-only proxy), for discovery. Optional. |
| `CONSOLE_COMPOSE_PROJECT` | Only containers of this Compose project. Optional; also the default deployment name. |
| `CONSOLE_NOTIFY` (or `_FILE`) | An ntfy-style URL to post down and back notices to. |
| `CONSOLE_PAGE` | Serve the page from disk instead of the built-in copy. |

The page's static files are `/`, `/lib/*`, `/widgets/*` and `/vendor/*`.

# Console API (`/api/*`)

| | |
|---|---|
| **Purpose** | What the console's page (and the terminal UI's console mode) reads and does: the deployment and its state, services' data passed through, and the few actions the console takes (order tickets, flatten, drive approval, jobs). |
| **Transport** | HTTP/1.1, JSON, on `CONSOLE_LISTEN` (default `127.0.0.1:8080`). |
| **Versioning** | `/api/topology` and `/api/state` carry `"schema": "tinytrader.fleet/1"` ([fleet](fleet.md)). The rest is unversioned and additive. |
| **Auth** | Reads are open: anyone who can reach the port can read, so bind to loopback or a private network. Every write goes through the [write gate](#the-write-gate): a `Host` and a Tailscale login on allowlists, and for a live account's orders (and any flatten or resume) also the order token. With either allowlist unconfigured, every write is refused. Writes must also be JSON and same-origin (a page on another site gets `403`). Tokens for the services behind it (gateways, store) stay in the console and are never returned. |
| **Errors** | `{"error": "..."}` with a 4xx or 5xx status. Passed-through answers keep the upstream's status and body. |
| **Health** | The console's own [observe](observe.md) endpoints. |
| **Stability** | `/api/topology`, `/api/state`, `/api/events`, `/api/stream` are **stable**. The order, account, jobs and queue routes are **experimental**. Prefs, reminders and profiles are optional modules, **experimental**. |

## The deployment

| Endpoint | |
|---|---|
| `GET /api/topology` | The deployment as drawn ([fleet](fleet.md#apitopology)). |
| `GET /api/state` | The latest poll ([fleet](fleet.md#apistate)). With `?since=G&epoch=E`, only what changed since generation `G`: each service keeps only its changed fields plus `_fields` (all its field names), and the answer adds `generation`, `epoch` (the console's start, Unix ms) and `full`. Each account in a service's `accounts.accounts` carries `age_ms`: the console's clock now minus the account's `updated_at`, or `null` without one. |
| `GET /api/events/{service}?after=&limit=` | A service's [event log](observe.md), passed through. `404` if it declares none, `502` if it doesn't answer. |

## Data from services

| Endpoint | |
|---|---|
| `GET /api/stream/{service}/{stream}?...` | One of a service's on-demand streams, the query passed through, the answer returned as it is (30 s timeout). `stream` is one of `bars`, `daily`, `snapshots`, `chain`, `chain_index`, `news_search`, `earnings_history`, `option_bars`, `option_contracts`, `screener`, `tickers`, `quotes`, `session`, `stats`, `tape`, `performance`, `trades`. `404` if that service doesn't declare it. |
| `GET /api/bars/{service}?...` | The same as `/api/stream/{service}/bars`. |

Pick `{service}` by capability from `/api/topology` (the first service with a `chain` field serves `chain`), never by a fixed id.

## The write gate

Reads (`GET`) are open. Every write route is behind the write gate, checked before anything is forwarded to a service or sent to a gateway:

1. **Host.** The request's `Host` must be one of `CONSOLE_HOSTS`: comma-separated host names, each with or without a port (without one, any port matches; case and a trailing dot don't matter). This stops a web page that points its own DNS name at the console's address from making changes through the viewer's browser. Refused with `403`. With `CONSOLE_HOSTS` empty or unset, **every write is refused** (`403`, saying so).
2. **Login.** The request's `Tailscale-User-Login` header must be one of `CONSOLE_ORDER_LOGINS` (comma-separated, exact match). `tailscale serve` sets that header on what it forwards from a tailnet user. Refused with `403`. With `CONSOLE_ORDER_LOGINS` empty or unset, **every write is refused** (`403`, saying the console has no allowed logins configured), and the page shows a banner saying changes are off.
3. **Order token.** An order, cancel, flatten or resume on a live account also needs `X-Order-Token` equal to the console's order token, read from the file `CONSOLE_ORDER_TOKEN_FILE` names, or else from `CONSOLE_ORDER_TOKEN`. So does a flatten or resume on a paper account, unless `CONSOLE_TOKEN_FOR_PAPER_FLATTEN` is `false` (default `true`). It is compared in constant time and never logged. A missing or wrong token gets `401`. With no token configured, those writes are refused (`403`, saying so) and everything else still works. Paper orders and cancels need only the login.

Every refusal by the gate carries the header `X-Write-Gate: refused` and `"gate": true` in its JSON body, next to `error`. A `401` or `403` without them came from the service behind the console (the jobs service, the queue or the store), not from the gate.

An account is live unless both the accounts service's `environment` for it and its gateway's welcome say `paper`; an account the accounts service doesn't report (or reports differently from two services) counts as live. A live account's flatten and resume also need `confirm` naming the account.

The login header is trusted only because `tailscale serve` removes any copy a client sends and sets its own. Anything that reaches the console's port without going through `tailscale serve` can send whatever login it likes: any process on the host, and any other container on the same Docker network. So keep `CONSOLE_LISTEN` on loopback, and keep the console off Docker networks it shares with other services. The order token is what still stops such a request from trading a live account, or flattening any account.

The write routes: `POST /api/orders/{account}`, `POST /api/orders/{account}/{id}/cancel`, `POST /api/accounts/{account}/flatten` and `/resume`, `POST /api/jobs/...`, `POST` and `PATCH /api/queue/...`, `POST /api/store/volumes/approve`, `POST /api/reminders`, `PUT /api/prefs/{key}`, `POST /api/journal`, `PUT` and `DELETE /api/journal/{id}`, and `POST /api/profiles`.

| Endpoint | |
|---|---|
| `GET /api/write-gate` | `{"writes": bool, "hosts": bool, "allowed": bool, "live_orders": bool}`: whether any logins are configured, whether any host names are configured, whether this request's Host and login may write, and whether an order token is configured. Never the logins, the host names or the token. |

To turn it on: set `CONSOLE_ORDER_LOGINS` to the Tailscale logins allowed to make changes, set `CONSOLE_HOSTS` to the name the console is opened at (for example its Tailscale name), serve the console through `tailscale serve`, and (for live accounts and flatten) put one line of random text, for example 64 hex characters, in a file the console reads as `CONSOLE_ORDER_TOKEN_FILE`. The page asks for the token at the first write that needs it in a tab and keeps it for that tab only.

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
| `GET /api/orders` | `{"trading": bool, "prefix": "tt-", "accounts": [{account, state, environment, broker, unresolved, orders: [{client_order_id, status, filled_qty, avg_price, legs, limit_price, qty, submitted_at}]} \| {account, error}]}`: the console's own open orders. `unresolved` is the gateway's count of orders not yet resolved (`null` when it doesn't say). |
| `POST /api/orders/{account}` | `{"order": {legs: [{symbol, side, ratio?}], qty, type?, limit_price \| ref_price, effect?}, "confirm": "<account>", "live"?: true}`. Answers the gateway's `response` as it is (`accepted`, `rejected`, `not_sent`, `unknown`). A live account needs `live: true`. |
| `POST /api/orders/{account}/{client_order_id}/cancel` | `{}`. One of the console's own orders. |
| `POST /api/accounts/{account}/flatten` | `{"confirm": "<account>", "reason"?}`. Needs the order token (on a paper account, unless `CONSOLE_TOKEN_FOR_PAPER_FLATTEN` is `false`). |
| `POST /api/accounts/{account}/resume` | `{"confirm"?: "<account>", "reason"?}`: the gateway's `unkill`. A live account needs `confirm`. Needs the order token as flatten does. |

All four are behind the [write gate](#the-write-gate). `404` when the account has no gateway; `400` for a bad or unconfirmed order; `401` or `403` when the write gate refuses; `502` when the gateway fails or can't be reached.

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
| `GET /api/journal?from=&to=`, `POST /api/journal`, `PUT /api/journal/{id}`, `DELETE /api/journal/{id}` | `CONSOLE_JOURNAL` | The trading journal's entries. |

Their writes are behind the [write gate](#the-write-gate) (the login only).

## Configuration

| Variable | |
|---|---|
| `CONSOLE_TOPOLOGY` | The [topology file](fleet.md#the-topology-file) (default `topology.json`). |
| `CONSOLE_DOCKER` | Docker Engine API base URL (a read-only proxy), for discovery. Optional. |
| `CONSOLE_COMPOSE_PROJECT` | Only containers of this Compose project. Optional; also the default deployment name. |
| `CONSOLE_NOTIFY` (or `_FILE`) | An ntfy-style URL to post down and back notices to. |
| `CONSOLE_ORDER_LOGINS` | The Tailscale logins allowed to write, comma-separated. Empty or unset: every write is refused. |
| `CONSOLE_HOSTS` | The host names the console is opened at, comma-separated, ports optional. A write with any other `Host` is refused. Empty or unset: every write is refused. |
| `CONSOLE_ORDER_TOKEN_FILE` (or `CONSOLE_ORDER_TOKEN`) | The order token a live account's order, cancel, flatten or resume needs, and a paper account's flatten or resume. Unset: those are refused. |
| `CONSOLE_TOKEN_FOR_PAPER_FLATTEN` | `true` (default) or `false`: whether a paper account's flatten and resume need the order token too. |
| `CONSOLE_PAGE` | Serve the page from disk instead of the built-in copy. |

The page's static files are `/`, `/lib/*`, `/widgets/*` and `/vendor/*`. They are sent with `Content-Security-Policy: script-src 'self'; frame-ancestors 'none'` and `X-Frame-Options: DENY`: scripts only from the console, and no other site can frame the page. Links and images in rendered documents lose `javascript:`, `data:` and `vbscript:` targets.

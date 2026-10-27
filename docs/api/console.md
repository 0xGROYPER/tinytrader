# Console API (`/api/*`)

| | |
|---|---|
| **Purpose** | What the console's page (and the terminal UI's console mode) reads and does: the deployment and its state, services' data passed through, and the few actions the console takes (order tickets, flatten, drive approval, jobs). |
| **Transport** | HTTP/1.1, JSON, on `CONSOLE_LISTEN` (default `127.0.0.1:8080`), and optionally read-only on `CONSOLE_READ_LISTEN` (see [The read-only port](#the-read-only-port)). |
| **Versioning** | `/api/topology` and `/api/state` carry `"schema": "tinytrader.fleet/1"` ([fleet](fleet.md)). The rest is unversioned and additive. |
| **Auth** | Reads are open: anyone who can reach the port can read, so bind to loopback or a private network. Every write goes through the [write gate](#the-write-gate): a `Host` and a login (set by an authenticating reverse proxy) on allowlists, and for a live account's orders (and any flatten or resume) also the order token, unless `CONSOLE_LIVE_NEEDS_TOKEN` is `false`. With either allowlist unconfigured, every write is refused. Writes must also be JSON and same-origin (a page on another site gets `403`). Tokens for the services behind it (gateways, store) stay in the console and are never returned. |
| **Errors** | `{"error": "..."}` with a 4xx or 5xx status. Passed-through answers keep the upstream's status and body. |
| **Health** | The console's own [observe](observe.md) endpoints. |
| **Stability** | `/api/topology`, `/api/state`, `/api/events`, `/api/stream` are **stable**. The order, account, jobs and queue routes are **experimental**. Prefs, reminders and profiles are optional modules, **experimental**; so are the external lists. |

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
2. **Login.** The request's login header (the one `CONSOLE_LOGIN_HEADER` names, default `Tailscale-User-Login`) must be one of `CONSOLE_WRITE_LOGINS` (comma-separated, exact match). The authenticating reverse proxy in front of the console sets that header to the signed-in user (see [The login header](#the-login-header)). Refused with `403`. With `CONSOLE_WRITE_LOGINS` empty or unset, **every write is refused** (`403`, saying the console has no allowed logins configured), and the page shows a banner saying changes are off.
3. **Order token.** An order, cancel, flatten or resume on a live account also needs `X-Order-Token` equal to the console's order token, read from the file `CONSOLE_ORDER_TOKEN_FILE` names, or else from `CONSOLE_ORDER_TOKEN`. So does a flatten or resume on a paper account, unless `CONSOLE_TOKEN_FOR_PAPER_FLATTEN` is `false` (default `true`). It is compared in constant time and never logged. A missing or wrong token gets `401`. With no token configured, those writes are refused (`403`, saying so) and everything else still works. Paper orders and cancels need only the login. With `CONSOLE_LIVE_NEEDS_TOKEN` set to `false` (default `true`), none of these needs the token, on a live account or a paper one: an allowed login (and the typed confirmation, where one is asked for) is enough, and the page doesn't ask for a token. Do that only with the deployment described in [Behind a proxy in the console's namespace](#behind-a-proxy-in-the-consoles-namespace); the console logs a warning at startup when it is set. It refuses to start that way off loopback unless `CONSOLE_TRUST_NETWORK` is `true`.

Every refusal by the gate carries the header `X-Write-Gate: refused` and `"gate": true` in its JSON body, next to `error`. A `401` or `403` without them came from the service behind the console (the jobs service, the queue or the store), not from the gate.

An account is live unless both the accounts service's `environment` for it and its gateway's welcome say `paper`; an account the accounts service doesn't report (or reports differently from two services) counts as live. A live account's flatten and resume also need `confirm` naming the account.

The write routes: `POST /api/orders/{account}`, `POST /api/orders/{account}/{id}/cancel`, `POST /api/accounts/{account}/flatten`, `/resume` and `/read`, `POST /api/jobs/...`, `POST /api/job-quotes/{id}/approve` and `/decline`, `POST` and `PATCH /api/queue/...`, `POST /api/store/volumes/approve`, `POST /api/reminders`, `PUT /api/prefs/{key}`, `POST /api/journal`, `PUT` and `DELETE /api/journal/{id}`, and `POST /api/profiles`.

| Endpoint | |
|---|---|
| `GET /api/write-gate` | `{"writes": bool, "hosts": bool, "allowed": bool, "live_orders": bool, "live_token": bool, "read_only": bool}`: whether any logins are configured, whether any host names are configured, whether this request's Host and login may write (never on the read-only port), whether live orders can be made (no token is needed, or one is configured), whether a live account's write needs the order token, and whether this is the read-only port. Never the logins, the host names or the token. |

To turn it on: serve the console through an authenticating reverse proxy, set `CONSOLE_LOGIN_HEADER` to the header it sets (unless it is `Tailscale-User-Login`), set `CONSOLE_WRITE_LOGINS` to the logins allowed to make changes, set `CONSOLE_HOSTS` to the name the console is opened at through the proxy, and (for live accounts and flatten) put one line of random text, for example 64 hex characters, in a file the console reads as `CONSOLE_ORDER_TOKEN_FILE`. The page asks for the token at the first write that needs it in a tab and keeps it for that tab only.

### The login header

The console doesn't authenticate anyone itself. It trusts a login header set by an authenticating reverse proxy in front of it, and `CONSOLE_LOGIN_HEADER` says which header that is (default `Tailscale-User-Login`, so existing deployments are unchanged). The console refuses to start if the name isn't a valid HTTP header name. Some proxies and the headers they set:

| Proxy | `CONSOLE_LOGIN_HEADER` |
|---|---|
| `tailscale serve` | `Tailscale-User-Login` (the default) |
| Cloudflare Access | `Cf-Access-Authenticated-User-Email` |
| oauth2-proxy | `X-Forwarded-Email` (or `X-Auth-Request-Email`, depending on how it is set up) |
| Authelia | `Remote-User` |

Only the configured header is read: with another one set, a `Tailscale-User-Login` a request carries is ignored. `CONSOLE_WRITE_LOGINS` holds the values that proxy sets (usually email addresses or user names).

The header is trusted only because two things hold, and both are up to the deployment:

- **The proxy strips any copy a client sends** and sets its own. Check this for the proxy you use; a proxy that passes a client's header through lets anyone choose their login.
- **The console's port is reachable only through the proxy.** Anything that reaches the port directly can send whatever login it likes: any process on the host, and any other container on the same Docker network. So keep `CONSOLE_LISTEN` on loopback (or on a network only the proxy shares), and keep the console off Docker networks it shares with other services.

The order token is what still stops a request that gets around the proxy from trading a live account, or flattening any account.

### The read-only port

With `CONSOLE_READ_LISTEN` set (for example `0.0.0.0:8081`), the console serves a second listener with the same routes and the same state as `CONSOLE_LISTEN`. Reads there answer exactly as on the main port. Every request that isn't a `GET` or `HEAD` is refused with `403` (`"this port is read-only"`, marked as the gate's refusal) before it reaches a route, the write gate or a service, whatever login, token, `Host` or other headers it carries: the listener that accepted the connection decides, not anything in the request. `GET /api/write-gate` there says `"read_only": true` and `"allowed": false`, and the page shows that changes are off.

### Behind a proxy in the console's namespace

A deployment where nothing but the authenticating proxy can reach the write port:

- Run an authenticating reverse proxy that shares the console's network namespace: for example a Tailscale sidecar container running `tailscale serve` (the console's container joins the sidecar's network, e.g. Compose's `network_mode: service:<sidecar>`), or oauth2-proxy in the same pod or namespace. Point it at `http://127.0.0.1:8080`.
- Set `CONSOLE_LISTEN=127.0.0.1:8080`. Only processes in that namespace (the console and the proxy) can reach it, so every request on it came through the proxy, which sets the login header.
- Set `CONSOLE_READ_LISTEN=0.0.0.0:8081` for the other services that read from the console (dashboards, scripts, other containers on the Docker network). Nothing reaching that port can write.
- Set `CONSOLE_LOGIN_HEADER`, `CONSOLE_WRITE_LOGINS` and `CONSOLE_HOSTS` for the proxy as usual.
- If the proxy is a Tailscale sidecar, run it in kernel networking mode (`TS_USERSPACE=false`, which needs `/dev/net/tun` and the `NET_ADMIN` capability). In userspace mode, tailscaled forwards inbound tailnet TCP that `serve` doesn't handle to the namespace's loopback, so any tailnet peer allowed to reach the node could connect to `127.0.0.1:8080` directly and send any login header it likes.

With that in place the order token adds little (the proxy has already authenticated the user, and nothing else can reach the write port), so `CONSOLE_LIVE_NEEDS_TOKEN=false` is reasonable: live orders, cancels, flatten and resume then need only an allowed login and the typed confirmation. Without it (the write port on a shared network, or published on the host), keep the default. The console refuses to start (exit code `2`) with `CONSOLE_LIVE_NEEDS_TOKEN=false` unless `CONSOLE_LISTEN` is a loopback IP address such as `127.0.0.1:8080` or `[::1]:8080`; a host name, even `localhost`, isn't accepted. An operator who trusts every network that can reach `CONSOLE_LISTEN` (for example a private network in front of a container listening on `0.0.0.0`, with the authenticating proxy on the host) can set `CONSOLE_TRUST_NETWORK=true` to skip that check; writes then rely on the login header alone, and the console logs that at startup.

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
| `POST /api/accounts/{account}/read` | `{}`. The account read from the broker now by the accounts service that serves it ([`alpaca-account`](alpaca-account.md)'s `POST /accounts/{name}/read`, which only reads): its `/accounts` entry with `age_ms`, as in `/api/state`. Needs only an allowed login, no order token. `404` when no service serves the account; Alpaca's failure comes back as the service's `502`. Liquidate reads this way before each order, rather than the polled copy in `/api/state`, whose positions are read only on order events and every `ALPACA_ACCOUNT_REFRESH` seconds. |

All of these are behind the [write gate](#the-write-gate). `404` when the account has no gateway; `400` for a bad or unconfirmed order; `401` or `403` when the write gate refuses; `502` when the gateway fails or can't be reached.

## Jobs, queue and storage (experimental)

| Endpoint | |
|---|---|
| `GET\|POST /api/jobs/{path}` | Passed to `CONSOLE_JOBS` ([databento-jobs](databento-jobs.md)), without any approval: a paid `/submit` sent this way is refused. |
| `POST /api/job-quotes/{id}/approve` | `{"confirm_usd": <the quoted price>, "again"?: [...]}`: a person's approval of a paid purchase, sent to the jobs service's `/submit` with `CONSOLE_JOBS_APPROVE_TOKEN` (or `_FILE`) as its bearer token and the login as `approved_by`. The Jobs widget sends the jobs service's quoted value exactly as listed, and shows it in cents, marked ≈ when rounding changed it. Behind the write gate, and needs the order token as a live order does (unless `CONSOLE_LIVE_NEEDS_TOKEN` is `false`). The jobs service's answer is passed back. `403` when no approval token is configured. |
| `POST /api/job-quotes/{id}/decline` | `{}`: drops the quote (the jobs service's `DELETE /quotes/{id}`), with the same token and gate. |
| `GET\|POST\|PATCH /api/queue/{path}` | Passed to `CONSOLE_QUEUE` ([queue](queue.md)). |
| `POST /api/store/volumes/approve` | `{path, name}`, passed to `CONSOLE_STORE` with `CONSOLE_STORE_TOKEN` ([store](store.md)). |

`404` when the service isn't configured.

## Documents

| Endpoint | |
|---|---|
| `GET /api/collections` | `[{id, name, docs: [{index, path, group, title}]}]`, from `CONSOLE_COLLECTIONS` (`Name=dir,dir;Name=dir`). |
| `GET /api/collections/{id}/{index}` | `{index, path, title, html}`. |

## External lists (experimental)

Read-only tables whose rows come from JSON endpoints the deployment names, shown by the Lists widget. Off unless `CONSOLE_LISTS` names a JSON file of sources:

```json
[
  {
    "id": "builds",
    "label": "Builds",
    "url": "http://127.0.0.1:9300/builds.json",
    "columns": [{ "key": "name", "label": "Name" }, { "key": "state", "label": "State" }]
  }
]
```

`id` is letters, digits, `-` and `_`, unique; `url` is `http://` or `https://`; `columns` (at least one) are shown in order. A file that can't be read or doesn't check out turns the lists off and says why in the log; the console still starts.

Each source's URL must answer `200` with a JSON array of objects, at most 1 MiB. The console keeps each row's configured keys, values as they came, and drops the rest. A source is asked when its list is read, at most every 3 s whatever the number of readers, and has 3 s to answer. A failure (no answer, an error status, anything but an array of objects) keeps the rows of the last success and is reported with them.

| Endpoint | |
|---|---|
| `GET /api/lists` | `{"lists": [list, ...]}`, in the file's order; `{"lists": []}` with none configured. A failing source never fails the request. |
| `GET /api/lists/{id}` | One `list`. `404` for an id that isn't configured. |

A `list` is `{id, label, columns: [{key, label}], rows: [{key: value}], updated_at, age_ms, error}`: `updated_at` is when the rows were read (Unix ms, the console's clock) and `age_ms` the console's clock now minus that, both `null` before any success; `error` is why the last attempt failed, or `null`. Rows with `error` set and an `age_ms` are the last success's, kept.

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
| `CONSOLE_LISTEN` | The address to serve on, reads and writes (default `127.0.0.1:8080`). |
| `CONSOLE_READ_LISTEN` | A second address to serve on, read-only: every request but `GET` and `HEAD` is refused (`403`). Optional. See [The read-only port](#the-read-only-port). |
| `CONSOLE_TOPOLOGY` | The [topology file](fleet.md#the-topology-file) (default `topology.json`). |
| `CONSOLE_DOCKER` | Docker Engine API base URL (a read-only proxy), for discovery. Optional. |
| `CONSOLE_COMPOSE_PROJECT` | Only containers of this Compose project. Optional; also the default deployment name. |
| `CONSOLE_NOTIFY` (or `_FILE`) | An ntfy-style URL to post down and back notices to. |
| `CONSOLE_LOGIN_HEADER` | The header the login is read from, set by the authenticating reverse proxy in front of the console (default `Tailscale-User-Login`). Not a valid header name: the console refuses to start. See [The login header](#the-login-header). |
| `CONSOLE_WRITE_LOGINS` | The logins allowed to write, comma-separated. Empty or unset: every write is refused. `CONSOLE_ORDER_LOGINS`, its old name, is still read when this is unset, with a deprecation note in the log. |
| `CONSOLE_HOSTS` | The host names the console is opened at, comma-separated, ports optional. A write with any other `Host` is refused. Empty or unset: every write is refused. |
| `CONSOLE_ORDER_TOKEN_FILE` (or `CONSOLE_ORDER_TOKEN`) | The order token a live account's order, cancel, flatten or resume needs, and a paper account's flatten or resume. Unset: those are refused. |
| `CONSOLE_TOKEN_FOR_PAPER_FLATTEN` | `true` (default) or `false`: whether a paper account's flatten and resume need the order token too. |
| `CONSOLE_LIVE_NEEDS_TOKEN` | `true` (default) or `false`: whether a live account's order, cancel, flatten or resume needs the order token. `false` drops the token everywhere (a paper flatten too, whatever `CONSOLE_TOKEN_FOR_PAPER_FLATTEN` says); an allowed login, and the typed confirmation where asked, are enough. Safe only when `CONSOLE_LISTEN` is reachable solely through the authenticating proxy, so with `false` the console refuses to start unless `CONSOLE_LISTEN` is a loopback IP address (`127.0.0.1` or `[::1]`, not a host name); see [Behind a proxy in the console's namespace](#behind-a-proxy-in-the-consoles-namespace). |
| `CONSOLE_TRUST_NETWORK` | `false` (default) or `true` (also `1`, `yes`, `on`): the operator trusts every network that can reach `CONSOLE_LISTEN`. With `true`, `CONSOLE_LIVE_NEEDS_TOKEN=false` no longer needs a loopback `CONSOLE_LISTEN`, and writes rely on the login header alone. |
| `CONSOLE_LISTS` | A JSON file of [external lists](#external-lists-experimental). Optional. |
| `CONSOLE_PAGE` | Serve the page from disk instead of the built-in copy. |

The page's static files are `/`, `/lib/*`, `/widgets/*` and `/vendor/*`. They are sent with `Content-Security-Policy: script-src 'self'; frame-ancestors 'none'` and `X-Frame-Options: DENY`: scripts only from the console, and no other site can frame the page. Links and images in rendered documents lose `javascript:`, `data:` and `vbscript:` targets.

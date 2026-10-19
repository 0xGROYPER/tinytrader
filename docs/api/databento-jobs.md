# databento-jobs

| | |
|---|---|
| **Purpose** | Buy historic DBN from Databento's batch API, with a quoted price and an exact confirmation before any charge, then download each finished job and file it in the [store](store.md). |
| **Transport** | HTTP/1.1, JSON, on `JOBS_LISTEN` (default `127.0.0.1:9121`). |
| **Versioning** | Unversioned paths; additive changes only. |
| **Auth** | Writes are refused from a web page of another origin; no token of its own yet, so bind to loopback or a private network. A charge needs a fresh quote and `confirm_usd` equal to its price. It writes to the store with `JOBS_STORE_TOKEN` (or `_FILE`). |
| **Errors** | `{"error": "..."}`: `400` invalid request, `409` stale or changed quote, `428` confirmation needed, `503` no ledger configured, `502` Databento failed. |
| **Health** | [Observe](observe.md) endpoints and `/events`. `idle` while it yields to live capture. |
| **Stability** | **Experimental.** |

## Configuration

| Variable | |
|---|---|
| `DATABENTO_API_KEY` (or `_FILE`) | Required. |
| `JOBS_STORE` | The store (default `http://127.0.0.1:9120`). |
| `JOBS_LEDGER` | The submissions ledger file; without it nothing can be submitted. |
| `JOBS_YIELD_TO`, `JOBS_CAPTURE_HOURS`, `JOBS_CALENDAR` | Hold downloads while capture is live (as the [queue](queue.md) does). |
| `JOBS_REFRESH_SECONDS` | How often Databento's job list is read (default 30). |

## Endpoints

| Endpoint | |
|---|---|
| `GET /jobs` | `{"jobs": [...], "store_reachable", "updated_at"}`. Each job: `id`, `state` (`queued`, `processing`, `done`, `expired`), `dataset`, `schema`, `symbols`, `stype_in`, `start`, `end`, `cost_usd`, `bytes`, `received`, `expires`, `held` (where the store has it), `transfer` (download progress), `unfileable` (why the store would refuse it). Times are integer Unix seconds. |
| `GET /datasets` | `{"datasets": [...]}` |
| `GET /schemas?dataset=` | `{"schemas": [...]}` |
| `POST /price` | `{dataset, schema, symbols (one), stype_in?, start, end (dates, end inclusive)}` → `{"quote", "cost_usd", "billable_bytes", "warnings", "valid_seconds", "held_days", "bought"}`. `held_days`: the requested days the store already holds; `bought`: earlier purchases the request overlaps. |
| `POST /submit` | `{quote, confirm_usd, again?}` → `{"job", "cost_usd"}`. `428` when `confirm_usd` doesn't match the price; a quote is used once. Never buys what's had already, checked at submission against the store's catalog and Databento's job list as they are then: `409` when the store holds any of the days, or the request overlaps a job on Databento's list that isn't expired (queued, processing, or done and waiting to download, wherever it was bought); `409` when it overlaps a purchase in the ledger whose job is no longer listed, unless `again` is true; `503` when either can't be read. |
| `POST /download/{job}` | Start downloading a finished job into the store → `202`. |

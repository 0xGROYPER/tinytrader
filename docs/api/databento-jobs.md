# databento-jobs

| | |
|---|---|
| **Purpose** | Buy historic DBN from Databento's batch API, with a quoted price and an exact confirmation before any charge, then download each finished job and file it in the [store](store.md). |
| **Transport** | HTTP/1.1, JSON, on `JOBS_LISTEN` (default `127.0.0.1:9121`). |
| **Versioning** | Unversioned paths; additive changes only. |
| **Auth** | Writes are refused from a web page of another origin; no token of its own yet, so bind to loopback or a private network. A charge needs a fresh quote and `confirm_usd` equal to its price. It writes to the store with `JOBS_STORE_TOKEN` (or `_FILE`). |
| **Errors** | `{"error": "..."}`: `400` invalid request, `409` stale or changed quote, or overlaps what's had, `428` confirmation needed, `503` no ledger configured, or what's had can't be read in full, `502` Databento failed or quoted a price that isn't a number of dollars. |
| **Health** | [Observe](observe.md) endpoints and `/events`. `idle` while it yields to live capture. |
| **Stability** | **Experimental.** |

## Configuration

| Variable | |
|---|---|
| `DATABENTO_API_KEY` (or `_FILE`) | Required. |
| `JOBS_STORE` | The store (default `http://127.0.0.1:9120`). |
| `JOBS_LEDGER` | The submissions ledger file (JSON lines, appended); without it nothing can be submitted. |
| `JOBS_YIELD_TO`, `JOBS_CAPTURE_HOURS`, `JOBS_CALENDAR` | Hold downloads while capture is live (as the [queue](queue.md) does). |
| `JOBS_REFRESH_SECONDS` | How often Databento's job list is read (default 30). |

## Endpoints

**Paid purchases are paused** ([decision 0019](../decisions/0019-purchase-guards.md)): `/submit` and `/submit-missing` refuse anything not priced at exactly $0 with `403`. Free requests, pricing and downloads work as below.

**What's had already** (decision 0019, section 2) is one set, used by both submit paths, per day, per contract and per time window: the store's Databento holdings (`/coverage`, and any day `/catalog` holds that `/coverage` doesn't explain, as held in full); every job on Databento's list that isn't expired (the list read in full: every state, all history); and every purchase in the ledger, whether or not its job is still listed, including submissions whose outcome isn't known. Job and ledger symbols are parsed: a list naming the parent or `ALL_SYMBOLS` covers it all, contracts of the parent count one by one, and instrument IDs or anything unreadable cover every day they overlap. Every purchase is refused (`503`) while the store answers anything but `200`, reports `missing_volumes` (or doesn't report them), or Databento's job list or any listed job can't be read. Datasets are compared in Databento's spelling and must be on its dataset list. One purchase runs at a time: both submit paths hold one lock from their last check through the ledger's last write.

| Endpoint | |
|---|---|
| `GET /jobs` | `{"jobs": [...], "store_reachable", "updated_at"}`. Each job: `id`, `state` (`queued`, `processing`, `done`, `expired`), `dataset`, `schema`, `symbols`, `stype_in`, `start`, `end`, `cost_usd`, `bytes`, `received`, `expires`, `held` (where the store has it), `transfer` (download progress), `unfileable` (why the store would refuse it). Times are integer Unix seconds. |
| `GET /datasets` | `{"datasets": [...]}` |
| `GET /schemas?dataset=` | `{"schemas": [...]}` |
| `POST /price` | `{dataset, schema, symbols (one), stype_in? (parent or raw_symbol), start, end (dates, end inclusive)}` → `{"quote", "cost_usd", "billable_bytes", "warnings", "valid_seconds", "held_days", "bought"}`. `held_days`: the requested days the store holds some of; `bought`: earlier purchases the request overlaps. |
| `POST /submit` | `{quote, confirm_usd, again?}` → `{"job", "cost_usd"}`. `428` when `confirm_usd` doesn't match the price (only an exact $0 is free); a quote is used once. Never buys what's had already, checked at submission: `409` when the request overlaps anything in it. A ledger purchase whose job is expired or no longer listed may be bought again only by naming it: `again: ["OPRA-…"]`, exactly the jobs replaced (`400` for any other, or for `again: true`). A submission whose outcome is unknown blocks what it overlaps until found on Databento's list or resolved. |
| `POST /price-missing` | `{dataset, schema, symbol (an option parent, e.g. SPY.OPT), start, end, rule?}` → `{"quote", "cost_usd", "billable_bytes", "requests", "days", "notes", "valid_seconds"}`. For each day the store holds the parent's `definition` for: the contracts `rule` wants (`expiries`: the first N expiries from that day; `dte_max`; `strike_min`/`strike_max` in dollars; none: the whole chain), less what's had already, priced as `raw_symbol` requests (split at 2,000 contracts): one for the contracts missing all day, and one for each stretch of hours still missing of contracts partly had (decision 0019, section 6). Each entry of `days` has its `window` (start and end, a date at midnight UTC or RFC 3339), `hours` (true for missing hours of a partial day), and counts skipped as held or as pending or bought; `notes` names days with nothing to buy or no definitions held. |
| `POST /submit-missing` | `{quote, confirm_usd}` → `{"submitted": [{day, job, contracts, cost_usd}], "cost_usd"}`. Plans again under the purchase lock and submits only if every request's contracts and window and the total price are unchanged (`409` otherwise), with `confirm_usd` equal to it when it costs anything (`428`). Each request is written to the ledger before it is sent; a failure stops there and lists what was submitted. Jobs bought this way are filed as partial days of the parent (`/incorporate?parent=`, decision 0018). |
| `POST /ledger/resolve` | `{request, at, job, note}` → `{"request", "state", "job"}`. The operator's word on a submission whose outcome isn't known (its latest ledger step `sending` or `uncertain`, identified by its `request` fingerprint and `at`): `job` is the job it made (written `submitted`), or `null` for none (`not-submitted`). Nothing else ever writes `not-submitted`. `404` when there's no such unresolved step. |

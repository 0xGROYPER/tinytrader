# Store API

| | |
|---|---|
| **Purpose** | The one process that touches the data disks ([decision 0005](../decisions/0005-store-service-and-supervision.md)): a catalog of historic DBN, files served by logical name, and verified uploads filed into a standard layout. |
| **Transport** | HTTP/1.1 on `STORE_LISTEN` (default `127.0.0.1:9120`). JSON, except file bodies. |
| **Versioning** | Unversioned paths; changes are additive (new fields, new endpoints). |
| **Auth** | Reads are open. Writes need `Authorization: Bearer <token>` when `STORE_WRITE_TOKEN` (or `STORE_WRITE_TOKEN_FILE`) is set, and are refused from a web page of another origin ([write token](#write-token)). |
| **Errors** | `{"error": "..."}` with the HTTP status (`400`, `401`, `403`, `404`, `409`, `422`, `500`, `507`). |
| **Health** | `/health`, `/metrics`, `/describe`, `/events` ([observe](observe.md)); `degraded` when a volume is missing or past its critical threshold. |
| **Stability** | `/catalog`, `/files` and `/files/{volume}/{path}` are **stable**; the write endpoints are **experimental** (their clients are tinytrader's own services). |

Shared conventions are in the [API overview](README.md).

| Endpoint | | |
|---|---|---|
| `GET /catalog` | read | Each volume's datasets, schemas and days, and how full it is. Each schema lists its `partial` days. |
| `GET /coverage?dataset=&symbol=&schema=&start=&end=` | read | Each day held: `complete`, or `partial` with its contracts and time window ([below](#partial-days)). |
| `GET /files` | read | DBN files the volumes hold, by logical name (below). |
| `GET /files/{volume}/{path}` | read | One DBN file, as it is on the volume. |
| `GET /incoming` | read | Batches uploaded and waiting to be filed. |
| `GET /batches/{provider}` | read | Which batches the volumes hold, and how. |
| `PUT /incoming/{provider}/{batch}/{file}?size=&sha256=` | write | Upload one file of a batch, verified on arrival. |
| `POST /incorporate/{provider}/{batch}` | write | File a complete batch into the standard layout. |
| `POST /import/{provider}/{id}` | write | File DBN that didn't come from a provider batch, uploaded with an `import.json` ([below](#partial-days)). |
| `POST /adopt/{provider}` | write | File a download folder already on a volume, where it is. |
| `POST /volumes/approve` | write | Approve a drive found in the mount directory. |

Writes are refused from a web page of another origin.

## Write token

When the store is given a token (`STORE_WRITE_TOKEN`, or better
`STORE_WRITE_TOKEN_FILE` naming a file that holds it, as a systemd credential
or a decrypted secret provides), every write must carry it:

```text
Authorization: Bearer <token>
```

A write without it, or with another, is refused with `401`. Reads stay open:
limit who can reach the store at all with the network (loopback, or a tailnet
policy). With no token configured, writes are open as before, so a deployment
can roll the token out client by client. A token that is configured but
can't be read, or is empty, stops the store from starting.

The clients that write send it when they are given it:

| Client | Writes | Its token |
|---|---|---|
| `databento-jobs` | uploads, filing | `JOBS_STORE_TOKEN` (or `_FILE`) |
| `alpaca-history` | uploads, filing | `HISTORY_STORE_TOKEN` (or `_FILE`) |
| `console` | approving a drive (Storage) | `CONSOLE_STORE_TOKEN` (or `_FILE`) |

`POST /adopt` has no client; send the header by hand
(`curl -H "Authorization: Bearer $(cat token-file)" …`).

Bind the store beyond loopback (`STORE_LISTEN`, or the module's
`observe.bind`) only with a token configured.

## Reading files

A file is named logically, `<volume>/<path within the volume>`, where the
volume is a name from `STORE_VOLUMES` or an approved drive, never a mount
point:

```text
newdrive/Databento/OPRA.PILLAR/SPY.OPT/cmbp-1/2026-09-01.dbn.zst
```

Only finished DBN files are served: names ending `.dbn` or `.dbn.zst`, never a
`.partial` being written, nothing in a hidden folder (such as a provider's
`.incoming`), and nothing a link leads to outside the volume. Sealed files are
immutable, so a file once read can be kept.

### `GET /files?pattern=&limit=`

```json
{ "files": [{ "name": "newdrive/Databento/…/2026-09-01.dbn.zst", "volume": "newdrive", "path": "Databento/…/2026-09-01.dbn.zst", "bytes": 1234 }], "truncated": false }
```

- `pattern` (default `*`) is matched against the whole name: `*` any run of
  characters, `/` included, `?` any one character. `*cmbp-1*` finds every
  `cmbp-1` file in either layout.
- `limit` (default and most 100,000) caps the list; `truncated` says whether
  more matched.
- Sorted by volume, then path. The volumes are walked on each request.

### `GET /files/{volume}/{path}`

The file's bytes (`application/octet-stream`, with its length), streamed.
`404` for an unknown volume or file, `400` for a name that isn't served.

Each request reads one whole file. Ranges of records, bulk reads across many
days, and running work beside the data are later steps.

## Partial days

A day file holding a list of contracts (`stype_in = raw_symbol`), for part
of a day, is a **partial day** ([decision 0018](../decisions/0018-partial-coverage.md)).
Its coverage is in the schema folder's `manifest.json`; a day without an entry
there is complete.

### `POST /import/{provider}/{id}`

Upload each day file, named `YYYY-MM-DD.dbn.zst`, and an `import.json`, with
`PUT /incoming/{provider}/{id}/{file}`, then file them:

```json
{
  "dataset": "OPRA.PILLAR", "schema": "cbbo-1m", "symbol": "SPY.OPT",
  "source": { "api": "timeseries.get_range", "tool": "research/cbbo_fetch.py", "commit": "61f984a", "fetched": "2026-10-08" },
  "files": [ { "filename": "2026-10-02.dbn.zst", "size": 1169420, "hash": "sha256:…" } ]
}
```

Each file is checked against its size and SHA-256 and against its own header
(dataset, schema, day). Every record must map to a listed contract of
`symbol` and fall inside the header's window. Then, for each day:

- **Not held:** filed as a partial day.
- **Held, byte-identical:** the upload is dropped as a duplicate.
- **Held partly, with other contracts:** the two are merged in timestamp
  order, checked record for record, and swapped in. The day's coverage is the
  union.
- **Held complete, or held partly with any of the same contracts:** `409`.
  Nothing is filed.

As with batches, every check runs before anything moves, `import.json` is
kept in `provenance/{id}/`, and each step is recorded in `migrations/`.

### `GET /coverage`

`provider` (default `databento`), `dataset`, `symbol` (the symbol level, such
as `SPY.OPT`), `schema`, and `start` and `end` (`YYYY-MM-DD`, inclusive):

```json
{ "days": [
  { "day": "2026-10-01", "volume": "data", "kind": "complete" },
  { "day": "2026-10-02", "volume": "data", "kind": "partial", "records": 68851, "sha256": "…",
    "coverage": { "kind": "contracts", "stype": "raw_symbol", "symbols": ["SPY   261002C00728000", "…"], "window": [1790946000000000000, 1790985600000000000] },
    "pending": false }
] }
```

`pending` is true while a merge into that day is unfinished. It is settled
the next time that day is filed.

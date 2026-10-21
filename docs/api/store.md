# Store API

| | |
|---|---|
| **Purpose** | The one process that touches the data disks ([decision 0005](../decisions/0005-store-service-and-supervision.md)): a catalog of historic DBN, files served by logical name, and verified uploads filed into a standard layout. |
| **Transport** | HTTP/1.1 on `STORE_LISTEN` (default `127.0.0.1:9120`). JSON, except file bodies. |
| **Versioning** | Unversioned paths; changes are additive (new fields, new endpoints). |
| **Auth** | Reads are open. Writes need `Authorization: Bearer <token>` when `STORE_WRITE_TOKEN` (or `STORE_WRITE_TOKEN_FILE`) is set, and are refused from a web page of another origin ([write token](#write-token)). |
| **Errors** | `{"error": "..."}` with the HTTP status (`400`, `401`, `403`, `404`, `409`, `422`, `500`, `503`, `507`). |
| **Health** | `/health`, `/metrics`, `/describe`, `/events` ([observe](observe.md)); `degraded` when a volume is missing or past its critical threshold. |
| **Stability** | `/catalog`, `/files` and `/files/{volume}/{path}` are **stable**; the write endpoints are **experimental** (their clients are tinytrader's own services). |

Shared conventions are in the [API overview](README.md).

| Endpoint | | |
|---|---|---|
| `GET /catalog` | read | Each volume's datasets by provider, their schemas and days, and how full it is. Each schema lists its `partial` and `unfiled` days ([below](#what-the-store-holds)). |
| `GET /coverage?provider=&dataset=&symbol=&schema=&start=&end=` | read | Each day held: `complete`, or `partial` with its contracts and time window ([below](#partial-days)). |
| `GET /files` | read | DBN files the volumes hold, by logical name (below). |
| `GET /files/{volume}/{path}` | read | One DBN file, as it is on the volume. |
| `GET /incoming` | read | Batches uploaded and waiting to be filed. |
| `GET /batches/{provider}` | read | Which batches the volumes hold, and how. |
| `PUT /incoming/{provider}/{batch}/{file}?size=&sha256=` | write | Upload one file of a batch, verified on arrival. |
| `POST /incorporate/{provider}/{batch}?parent=` | write | File a complete batch into the standard layout. With `parent` (such as `SPY.OPT`), a batch bought as a list of that parent's contracts is filed as partial days, as an import. |
| `POST /import/{provider}/{id}` | write | File DBN that didn't come from a provider batch, uploaded with an `import.json` ([below](#partial-days)). |
| `POST /adopt/{provider}` | write | File a download folder already on a volume, where it is. |
| `POST /volumes/approve` | write | Approve a drive found in the mount directory. |
| `POST /volumes/{name}/retire` | write | Forget a remembered volume that is away for good ([below](#remembered-volumes)). |
| `POST /volumes/{name}/accept-empty` | write | Accept that a mounted volume which held datasets is now empty ([below](#post-volumesnameaccept-empty)). |

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

## What the store holds

Purchases are decided from `/catalog` and `/coverage` (a day held is not
bought again), so they say what the store holds or say they can't
([decision 0019](../decisions/0019-purchase-guards.md)):

- Both answer `503 {"error": "the store is still scanning"}` until the first
  scan after the store starts is done.
- Both carry `missing_volumes`: the remembered volumes that aren't there now
  ([below](#remembered-volumes)). In `/catalog` a volume whose provider or
  dataset folders couldn't all be read, or that has a malformed manifest, is
  missing too, with the reasons in its root's `errors`. `/coverage` answers
  `500` instead. An empty list means every volume was read.
- Every dataset has a `provider`, the store's name for the volume's top-level
  folder it sits in, case ignored (`Databento/` and `databento/` are both
  `databento`; `Alpaca/` is `alpaca`). Data outside every provider's folder
  (a batch download at the top of a volume, a backup, `tmp`, a crypto
  recording) has no provider (`null`) in the catalog, with the dataset,
  schema and symbols its files say, and counts as Databento's in
  `/coverage`, so a copy of Databento's data anywhere blocks buying it again,
  while a recording of another dataset never matches. Datasets are listed by
  provider, id and symbol, never merged across providers: each vendor's ids
  stay its own.
- `filed` is true when every day of the dataset is in the store layout.
  Days in a batch download, or in the older layout of a folder per day, are
  listed in their schema's `unfiled`.
- A batch file named for a range of days (`…-20261001-20261031.cbbo-1m…`)
  counts every day of it, both ends included. When a batch is split by
  symbol, or a file's name doesn't say its days, the batch counts every day of
  its query (`start` and `end` in `metadata.json`); without them it is an
  error.
- No DBN file on a volume goes uncounted. Every `.dbn` or `.dbn.zst` file
  the layouts above don't account for (named some other way, deeper than the
  layouts are read, loose) is counted from its own header (dataset, schema,
  symbols, start and end), under the provider folder it is in, if any, as not
  filed. A file of a list of contracts (`stype_in
  raw_symbol`) holds those contracts, partially; otherwise the header's
  symbols, whole. Hidden folders, such as a provider's `.incoming`, are
  skipped. Headers are read once and kept while a file's size and last change
  stay the same. A file whose header can't be read yet and that changed in
  the last two minutes is taken as still being written: it is listed in its
  root's `pending_files` and read again on the next scan.
- A download folder with `metadata.json` but no `manifest.json` (one
  extracted by hand) counts by its files' headers, or, when it has no DBN
  files or has data in other encodings, by every day of its query.
- A volume is missing, with the reason in its root's `errors`, when a folder
  on it can't be read; when a DBN file's header, a manifest or a batch's
  `metadata.json` can't be read; when its path must be a mount point and
  isn't (an empty folder left in `/Volumes` by a drive that wasn't unmounted
  cleanly); or when it held datasets when last read in full and now holds
  none ([accept it as empty](#post-volumesnameaccept-empty) if it really
  is).

### `GET /catalog`

```json
{ "generated_at": 1792500000.0, "missing_volumes": [],
  "roots": [{ "name": "data", "path": "/Volumes/Data", "present": true, "writable": true, "errors": [],
    "datasets": [{ "provider": "databento", "id": "OPRA.PILLAR", "symbol": "SPY.OPT", "filed": true,
      "sources": ["store: /Volumes/Data/Databento/OPRA.PILLAR"],
      "schemas": { "cbbo-1m": { "files": 2, "bytes": 1234, "symbols": [], "encodings": ["dbn.zst"],
        "days": { "2026-10-01": 600, "2026-10-02": 634 }, "partial": ["2026-10-02"], "unfiled": [] } } }],
    "volume": { "…": "…" }, "dataset_bytes": 1234, "other": [], "other_bytes": 0, "unrecognised": [] }],
  "thresholds": { "…": "…" }, "candidates": [], "discover": "/Volumes" }
```

### Remembered volumes

The store keeps every volume it has used, configured or approved, name and
path, in its state file: `STORE_STATE` (or `--state`), by default
`~/Library/Application Support/tinytrader/store-state.json`. It is written
whole (beside the old one, flushed, then renamed over it), with what each
volume held when last read in full. A remembered volume that isn't mounted
when the store starts, or is unplugged later, stays in `/catalog` with
`"present": false` and in `missing_volumes`, so its days are never taken as
not held. An approved drive mounted under a new path is used there.

Approved drives, and volumes in `STORE_VOLUMES` under `/Volumes`, must be
mount points (on another device than the folder they sit in), and nothing is
written to one that isn't. Add `:anydir` to a `STORE_VOLUMES` entry
(`NAME=PATH:anydir`, with or before `:ro`) to take a plain folder.

#### `POST /volumes/{name}/retire`

For a volume that is away for good. It is forgotten, so it no longer counts
as missing, and the state file keeps a record of it under `retired`:

```json
{ "name": "archive8", "path": "/Volumes/Archive8", "retired_at": 1792500000.0,
  "held": { "databento/OPRA.PILLAR/SPY.OPT": { "cbbo-1m": { "days": 21, "first": "2026-09-01", "last": "2026-09-30", "bytes": 123456 } } } }
```

`held` is what the volume held when it was last read in full. `409` while the
volume is mounted, `404` for a volume the store doesn't know. A volume named
in `STORE_VOLUMES` comes back when the store restarts; take it out of the
configuration too.

#### `POST /volumes/{name}/accept-empty`

A volume that held datasets when last read in full, and now holds none, is
missing: most often the wrong drive, or an empty folder, is mounted there.
When it really has been emptied, say so, optionally with who you are:

```json
{ "by": "dawson" }
```

It is accepted only while it is mounted and scans with no datasets and no
errors (`409` otherwise, `404` for a volume the store doesn't know). It is
then no longer missing, and the state file keeps a record under
`accepted_empty`:

```json
{ "name": "newdrive", "path": "/Volumes/NewDrive", "accepted_at": 1792500000.0, "by": "dawson",
  "held_before": { "databento/OPRA.PILLAR/SPY.OPT": { "cbbo-1m": { "days": 21, "first": "2026-09-01", "last": "2026-09-30", "bytes": 123456 } } } }
```

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

### `GET /files?pattern=&provider=&limit=`

```json
{ "files": [{ "name": "newdrive/Databento/…/2026-09-01.dbn.zst", "volume": "newdrive", "path": "Databento/…/2026-09-01.dbn.zst", "bytes": 1234 }], "truncated": false }
```

- `pattern` (default `*`) is matched against the whole name: `*` any run of
  characters, `/` included, `?` any one character. `*cmbp-1*` finds every
  `cmbp-1` file in either layout.
- `provider` (optional) keeps only files in that provider's top-level
  folder, case ignored: `?pattern=*/OPRA.PILLAR/SPY.OPT/definition/*&provider=databento`.
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

`provider` (required, `400` without it: `databento` or `alpaca`), `dataset`,
`symbol` (the symbol level, such as `SPY.OPT`), `schema`, and `start` and
`end` (`YYYY-MM-DD`, inclusive):

```json
{ "days": [
  { "day": "2026-10-01", "volume": "data", "volumes": ["data"], "kind": "complete", "filed": true, "size": 1234, "conflict": false },
  { "day": "2026-10-02", "volume": "data", "volumes": ["data"], "kind": "partial", "filed": true, "size": 4567,
    "records": 68851, "sha256": "…",
    "coverage": { "kind": "contracts", "stype": "raw_symbol", "symbols": ["SPY   261002C00728000", "…"], "window": [1790946000000000000, 1790985600000000000] },
    "pending": false, "conflict": false }
], "missing_volumes": [] }
```

Every day the catalog counts for that provider is there: days in the store
layout, complete or partial by the schema folder's manifest, and days in a
batch download or a day folder of the older layout, as `complete` with
`"filed": false`. A batch bought as a list of contracts (`stype_in
raw_symbol`) holds those of the symbol's contracts, for its whole day, as
`partial`. A dataset is matched ignoring case. Data whose symbols can't be
told apart (requested by instrument ID, or with no symbol type) is taken to
hold every symbol of its dataset, whole, for the days it covers.

A day held on several volumes, or in several forms, is one row. `volumes`
names them all. If any copy is complete the day is complete; otherwise its
coverage is the union of the copies' contracts and the span of their
windows, with no single `size`, `sha256` or `records`. `conflict` is true
when the copies differ. `filed` is true when any copy is in the store
layout.

`pending` is true while a merge into that day is unfinished. It is settled
the next time that day is filed.

An unreadable folder, a malformed manifest or a batch's unreadable
`metadata.json` is a `500`, never an answer with days left out.

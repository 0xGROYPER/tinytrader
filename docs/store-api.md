# Store API

The store ([decision 0005](decisions/0005-store-service-and-supervision.md)) is
the one process that touches the data disks; everything else reaches data
through it over HTTP, on `STORE_LISTEN` (by default `127.0.0.1:9120`). It
meets the service contract (`/health`, `/metrics`, `/describe`) and serves
`/events`.

| Endpoint | | |
|---|---|---|
| `GET /catalog` | read | Each volume's datasets, schemas and days, and how full it is. |
| `GET /files` | read | DBN files the volumes hold, by logical name (below). |
| `GET /files/{volume}/{path}` | read | One DBN file, as it is on the volume. |
| `GET /incoming` | read | Batches uploaded and waiting to be filed. |
| `GET /batches/{provider}` | read | Which batches the volumes hold, and how. |
| `PUT /incoming/{provider}/{batch}/{file}?size=&sha256=` | write | Upload one file of a batch, verified on arrival. |
| `POST /incorporate/{provider}/{batch}` | write | File a complete batch into the standard layout. |
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

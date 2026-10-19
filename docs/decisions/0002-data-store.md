# 0002: The tinytrader data store

- **Status:** Accepted
- **Date:** 2026-10-05
- **Amended:** 2026-10-05: provider roots, provenance and migration
  ledgers, raw pages from request/response sources, the intraday store,
  incorporating provider batches, and keeping roots clean
- **See also:** [0005](0005-store-service-and-supervision.md), which makes
  the store a service reached over the network;
  [0018](0018-partial-coverage.md), which adds partial days (a list of
  contracts for part of a day), their coverage in each schema folder's
  `manifest.json`, imports of DBN not from a batch, and merging


> **Built so far (2026-10-07):** provider roots and the standard layout,
> filing provider batches (Databento) with SHA-256 checks against the
> manifest, DBN header checks, provenance and migration ledgers, and the
> catalog. **Not yet built:** raw capture, the intraday store, segments and
> sealing, compaction, and reading data back through the store. Those parts
> below are the design, not a description of the code.

## Context

Every tinytrader deployment collects, keeps and replays data: live market data
first, later signals, fills and research outputs. Without a shared store,
each service invents its own layout, recovery and verification, and every
consumer learns a different set of files.

The store should be standard, boring and convenient. Where an established
convention exists, adopt it. Databento's file conventions are the main
reference, because they are well designed, widely consumed, and already used
by tinytrader operators who buy historical data:

- datasets named `VENUE.FEED` (for example `OPRA.PILLAR`) and lower-case
  schema names (`definition`, `trades`, `mbp-1`);
- one zstd-compressed DBN file per dataset, schema and UTC day;
- a `manifest.json` listing every file with its size and SHA-256, a
  `metadata.json` describing how the data was produced, and a
  `condition.json` reporting per-day availability.

Crypto venues complicate this. Their feeds don't map cleanly onto existing
DBN schemas (sizes need wider integer fields, among other things), so data
must be normalized into custom schemas, and normalization code will have bugs.
The original bytes must therefore be kept, so data can be normalized again.

## Goals

1. **One layout** for every dataset, so any tool can find data without asking
   the service that wrote it.
2. **Never lose or corrupt committed data,** including across crashes, full
   disks and interrupted transfers.
3. **Verifiable at every step:** every file has a recorded size and hash, and
   copies are checked before an original is released.
4. **Convenient to read:** closed days look like a Databento download, and
   one library reads open and closed data alike.
5. **Fits small and large machines** through configuration, not forks.

Non-goals for now: a query engine, multi-writer datasets, encryption at rest
(left to the filesystem, such as ZFS), and cross-machine replication beyond
the tiering described below.

## Design

### What is stored

Each capture keeps two forms:

| Form | Purpose | Format |
|---|---|---|
| **Raw** | Exactly what the venue sent, with receive timestamps. Source of truth for re-normalization, audits and debugging. | Packet-capture style message log (see [Raw capture](#raw-capture)) |
| **Normalized** | Typed records for replay, research and strategies. | DBN, zstd-compressed, with tinytrader's schema extensions |

Derived forms, such as Parquet copies for research, are produced from the
normalized data, can be regenerated, and are not part of the source of truth.

The store itself is format-agnostic: a dataset declares its encoding and
schema, and segments are sealed, hashed and catalogued the same way whatever
they contain. Market data uses DBN; other datasets may use Arrow IPC,
Parquet or JSON Lines.

### Layout

Each provider's data lives under its own root, shaped like a Databento
download that has been incorporated into a long-lived store:

```text
<root>/                                one provider, e.g. Databento, Alpaca, Coinbase
├── <DATASET>/                         ... (below)
├── provenance/
│   └── <batch-or-capture-id>/         how the data was obtained: the provider's
│                                      manifest.json, metadata.json and condition.json
│                                      for a purchased batch; requests and receipts
│                                      for a capture
└── migrations/
    ├── <YYYY-MM-DD>-<name>.json       every move or release: source, destination,
    └── <YYYY-MM-DD>-<name>.events.jsonl  size and SHA-256, and each step as it happened
```

Within a provider root:

```text
<root>/
└── <DATASET>/                         e.g. COINBASE.EXCHANGE
    ├── dataset.json                   name, publisher, encoding, schemas, store version
    ├── raw/
    │   └── <YYYY-MM-DD>/
    │       ├── <HHMMSS>-<seq>.pcapng.zst        sealed raw segments
    │       └── manifest.json
    └── <schema>/                      e.g. trades, mbp-1, definition
        ├── <YYYY-MM-DD>.dbn.zst       closed day, compacted (Databento-style)
        ├── <YYYY-MM-DD>/              open or not-yet-compacted day
        │   ├── <HHMMSS>-<seq>.dbn.zst            sealed segments
        │   ├── <HHMMSS>-<seq>.dbn.partial        the one segment being written
        │   └── manifest.json
        ├── manifest.json              closed days: file, size, sha256, records, time range
        └── condition.json             per-day availability and gaps
```

- Days are UTC, as in Databento.
- Dataset names follow Databento's `VENUE.FEED`. For venues Databento doesn't
  cover, the feed part is the venue's own name for the feed:
  `COINBASE.EXCHANGE`, `KRAKEN.SPOT`, `KRAKEN.FUTURES`. tinytrader keeps a short
  registry of these names (`docs/datasets.md`) so deployments agree.
- Files hold every symbol by default, one file per dataset, schema and day.
  A dataset that captures only some symbols can opt into a symbol level,
  `<DATASET>/<symbol>/<schema>/` (and `<DATASET>/<symbol>/raw/`), declared in
  its `dataset.json`. Databento batches requested by parent symbol, such as
  `OPRA.PILLAR/SPY.OPT/cmbp-1/`, use this level.
- A closed day is a file, never a folder. Per-day receipts and manifests go
  to `provenance/`, not beside the data.
- A deployment refers to a store by its logical data root, never by a path.

### Segments and sealing

Data is written to one open segment per dataset, schema and day
(`*.partial`). **Sealing** closes it: flush, fsync, record size, SHA-256,
record count and first/last timestamps, rename atomically to its final name,
fsync the directory, then add it to the day's `manifest.json` (itself written
to a temporary file and renamed).

**Sealed segments are immutable.** They are never appended to or rewritten.
This is what makes hashes meaningful, copies safe, and backups incremental.

**When to seal is policy, configured per deployment and per dataset:**

```json
{ "seal": { "max_age": "1h", "max_bytes": "512MiB", "at_day_end": true } }
```

Whichever limit is reached first seals the segment; the UTC day boundary
always does. A small machine might seal every 500 MiB; a server might seal
hourly. Shorter seals reduce how much data exists only in an open segment and
how much a crash can affect.

### Compaction: Databento-shaped days

A day full of small segments is awkward to consume. Once a day is closed and
all its segments are sealed, **compaction** merges them into a single
`<YYYY-MM-DD>.dbn.zst`, matching Databento's per-day files:

1. Decode the day's segments in order and write one DBN stream with a single
   metadata header. (DBN files can't simply be concatenated, because each
   carries its own header.)
2. Verify the result: record count and a hash of the decoded records must
   match the segments'.
3. Record the day file in the schema's `manifest.json`, then release the
   segments.

This answers the question of "resealing by appending": instead of mutating
sealed files, a new immutable file replaces many small ones, after
verification. Raw segments are not compacted; they stay as captured.

### Raw capture

TLS-encrypted websocket traffic is useless as literal network packets, so raw
capture records application-level messages after decryption, each with its
receive timestamp, the connection it arrived on, and its direction.

The container is **pcapng**: a published, block-based, append-friendly format
with nanosecond timestamps, per-connection interface descriptions, comments,
and existing readers in Rust, Python, Wireshark and others. Each connection is
an interface whose description records the endpoint and subscription; each
message is a packet using a user-defined link type. A custom framed log would
be simpler to write but has no existing tooling.

### Raw pages from request/response sources

Some sources are historical APIs answered page by page, not streams; Alpaca's
historical trades and quotes are an example. For these, the raw form is the
pages exactly as received, each with a receipt (the request, when it was
made, the response status and the page's SHA-256), bundled one archive per
day: `<DATASET>/[<symbol>/]raw/<YYYY-MM-DD>.tar`. The day's page list and
receipts are recorded in `provenance/`. A day is complete only when the
source confirms the last page.

### Incorporating provider batches

A purchased batch (a Databento download, for example) is incorporated, not
left where it was downloaded:

1. Download it to a named incoming directory on the same volume as its
   provider root.
2. Verify every file's size and SHA-256 against the provider's manifest.
3. Move each data file to `<DATASET>/[<symbol>/]<schema>/<YYYY-MM-DD>.dbn.zst`
   (on the same volume, by a link that cannot replace an existing file). A
   day already held on any of the store's volumes must be byte-identical
   (SHA-256 recomputed on both sides), in which case the incoming copy is
   dropped as a duplicate; otherwise the whole batch is refused and nothing
   moves. A batch for several symbols is refused, since only a file for all
   symbols may sit at `<DATASET>/<schema>/`.
4. Move the provider's manifest, metadata and condition files to
   `provenance/<batch-id>/`.
5. Record the batch in a `migrations/` ledger, then remove the incoming
   directory once it is empty. Nothing is removed recursively.

An interrupted incorporation resumes from its ledger.

### The intraday store

Some authoritative data arrives only after the day ends (historical
downloads, for example), while recovering from a disconnect during the day
needs data now. A deployment may therefore record a provider's live feed to
an **intraday store**: a separate root, never mixed into a provider root,
with the same hierarchy (`<DATASET>/[<symbol>/]<schema>/<YYYY-MM-DD>.dbn.zst`,
`.partial` while open).

Intraday data is a stopgap, not a source of truth. Readers use it only when
asked to or when the authoritative day doesn't exist yet. Once the
authoritative day is sealed and verified, the intraday day is released: a
recorded entry in `migrations/`, never an unrecorded deletion. A deployment
that wants to compare live and historical data can keep intraday days
instead.

### Keeping roots clean

A provider root holds only its datasets, `provenance/`, `migrations/` and the
store's own files. Everything else goes elsewhere:

- Test runs, pilots, soaks and partial experiments write to disposable roots
  outside provider roots, each named for its purpose and with a stated
  lifetime (for example `tmp/<purpose>/` on a large volume when it doesn't
  fit on the local disk).
- Writers create a directory when they write its first file, and remove
  directories they created and didn't use. No empty directories are left
  behind.
- Catalogs ignore operating-system metadata (`.DS_Store`, `.Spotlight-V100`,
  `.fseventsd`, `.Trashes`) and report anything else they don't recognize, so
  litter is visible rather than silently tolerated.

### Recovery

On start, a writer looks for `*.partial` files. Normalized partials are
truncated to the last complete record and sealed with a `recovered` flag; raw
partials are truncated to the last complete block. Nothing is discarded
silently: recovery and any truncated bytes are recorded in the manifest and
in `condition.json`.

### Timestamps

Every normalized record carries two UTC nanosecond timestamps, following DBN:
`ts_event`, the venue's time for the event, and `ts_recv`, when the capturing
host received it. Each capture records the host's clock source and estimated
offset in its metadata, so latency and ordering analysis knows how far to
trust `ts_recv`. Raw captures keep every timestamp the venue sent, inside the
original messages.

### Schema extensions

tinytrader's custom schemas carry a distinct, versioned name, such as
`tinytrader.mbp-1/1`, recorded in each file's metadata. A reader that doesn't
know a schema version refuses the file rather than misreading it.

They are implemented by a `dbn-ext` library that wraps the official `dbn`
crate and adds the extension records, with a matching CLI wrapper, so
consumers use one API for standard and extended data. Because raw captures
are kept, normalized data in an extension schema can be regenerated if DBN
later supports the same data officially; the extensions are a convenience
over the raw record, not the only copy of it.

### Retention

Raw captures are kept indefinitely. After a configurable age they move from
fast storage to the archive tier; deleting them is always an explicit,
manual operation, never a policy. Normalized and derived data follow the same
rule by default, and derived data (such as Parquet exports) may be pruned and
rebuilt freely.

### Disk thresholds

Each data root's volume is classified by how full it is: at **80%** a
warning is raised; at **90%** writers other than capture pause; at **97%**
capture stops cleanly and the stop is recorded as a gap. Nothing is ever
deleted to make room. The thresholds are defaults that can be changed, not
settings anyone must choose. Where a volume's free space cannot be measured
(a folder shared into a container by Docker Desktop, for example), it is
reported as unknown rather than guessed.

### Coverage and gaps

`condition.json` follows Databento's per-day availability (`available`,
`degraded`, `missing`) and adds the reason and intervals of any gap: a
disconnect, a venue outage, a recording failure. A gap in the source is
recorded as a fact about the data, distinct from a writer's own failure.

### Catalog

Manifests are the authority. A **SQLite index** built from them answers
questions quickly: which datasets exist, what ranges they cover, where the
gaps are, how large they are. The index can be deleted and rebuilt at any
time with `tinytrader-store index`, so it needs no backup. The console's Data
view reads it.

### Tiering

Moving data from a hot root (for example on a VPS) to an archive root is a
store operation, not a script:

1. Copy a sealed segment or day file to the destination.
2. Verify size and SHA-256 at the destination.
3. Record it in the destination manifest.
4. Only then release the source copy.

Each step is idempotent, so an interrupted transfer resumes rather than
starting over, and nothing is released before it is verified.

### Reading

- **`tinytrader-store` (Rust library):** list datasets, read a time range as one
  record stream across compacted days and sealed segments, verify files.
- **`tinytrader-store` (CLI):** `ls`, `verify`, `seal`, `compact`, `sync`,
  `index`, `export` (to Parquet).
- **Bindings for Python and Julia** over the same library, so no consumer
  parses files on its own.

Closed days are ordinary `.dbn.zst` files, so the standard DBN tools and the
Databento client libraries can read them too, apart from tinytrader's custom
schemas.

### Configuration

Each deployment configures its stores as data; nothing about a particular
deployment lives in tinytrader. One format, `store.toml`, holds:

- **roots:** logical names mapped to paths on that machine;
- **seal policy:** `max_age`, `max_bytes` and `at_day_end`, with defaults per
  store and overrides per dataset;
- **compaction:** when closed days are compacted;
- **tiering:** destination roots and the age at which data moves.

On NixOS, the `tinytrader.store` module options write `store.toml`; with Docker,
the file is mounted into the container. Both use the same format and the same
documentation.

### Viewing

Each store root runs a small read-only catalog service beside it, serving
`/catalog`, `/health` and `/metrics`. The console aggregates those services,
as it does every other service, so it never needs filesystem access to the
machines that hold data. The console's Data page shows, for every dataset:
schemas, range, size and where it is stored; a per-day coverage calendar from
`condition.json`; for a chosen day, its files with state, records, size, time
range, hash, location and copy status; and a preview of up to 100 decoded
records from normalized schemas.

The viewer is read-only. Store operations (verify, compact, sync) stay in the
CLI until there is a reason to add them, behind confirmation and an audit log.

## Consequences

- One library owns durability, verification and layout; services only append
  records and choose a seal policy.
- Storage cost roughly doubles for captured data, because raw and normalized
  forms are both kept. Raw data compresses well and can move to cheaper
  storage first.
- Closed-day files look like Databento downloads, so purchased and captured
  data sit side by side and are read the same way.
- Compaction adds a background step per day; until it runs, readers see
  segments instead of a single file, transparently through the library.

## Resolved

- Raw captures use pcapng.
- Datasets are named `VENUE.FEED`, with a registry in [datasets.md](../datasets.md).
- One file per dataset, schema and day, with an optional symbol level.
- Normalized records carry `ts_event` and `ts_recv`.
- Custom schemas have prefixed, versioned names, implemented by `dbn-ext`
  over the official `dbn` crate.
- Raw captures are kept indefinitely and tiered down, never pruned by policy.
- Each provider has its own root with `provenance/` and `migrations/`, as a
  Databento download incorporated into a store looks.
- Closed days are files, never folders; per-day receipts live in `provenance/`.
- Raw data from request/response sources is the pages as received, one
  archive per day.
- Provider batches are incorporated by verified, recorded renames.
- Live recordings kept only to bridge until authoritative data arrives live in
  a separate intraday store and are released, with a record, once superseded.
- Provider roots contain nothing but data, provenance and migration ledgers.

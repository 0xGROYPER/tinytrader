# 0002: The Glacier data store

- **Status:** Proposed
- **Date:** 2026-10-05

## Context

Every Glacier deployment collects, keeps and replays data: live market data
first, later signals, fills and research outputs. Without a shared store,
each service invents its own layout, recovery and verification, and every
consumer learns a different set of files.

The store should be standard, boring and convenient. Where an established
convention exists, adopt it. Databento's file conventions are the main
reference, because they are well designed, widely consumed, and already used
by Glacier operators who buy historical data:

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
| **Normalized** | Typed records for replay, research and strategies. | DBN, zstd-compressed, with Glacier's schema extensions |

Derived forms, such as Parquet copies for research, are produced from the
normalized data, can be regenerated, and are not part of the source of truth.

The store itself is format-agnostic: a dataset declares its encoding and
schema, and segments are sealed, hashed and catalogued the same way whatever
they contain. Market data uses DBN; other datasets may use Arrow IPC,
Parquet or JSON Lines.

### Layout

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
- Dataset names follow `VENUE.FEED`. Glacier-defined datasets use the
  venue's own name for the feed (see [Open questions](#open-questions)).
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

The proposed container is **pcapng**: a published, block-based, append-friendly
format with nanosecond timestamps, per-connection interface descriptions,
comments, and existing readers in Rust, Python, Wireshark and others. Each
connection is an interface whose description records the endpoint and
subscription; each message is a packet using a user-defined link type. The
alternative is a small custom framed log, which is simpler to write but has
no existing tooling. See [Open questions](#open-questions).

### Recovery

On start, a writer looks for `*.partial` files. Normalized partials are
truncated to the last complete record and sealed with a `recovered` flag; raw
partials are truncated to the last complete block. Nothing is discarded
silently: recovery and any truncated bytes are recorded in the manifest and
in `condition.json`.

### Coverage and gaps

`condition.json` follows Databento's per-day availability (`available`,
`degraded`, `missing`) and adds the reason and intervals of any gap: a
disconnect, a venue outage, a recording failure. A gap in the source is
recorded as a fact about the data, distinct from a writer's own failure.

### Catalog

Manifests are the authority. A **SQLite index** built from them answers
questions quickly: which datasets exist, what ranges they cover, where the
gaps are, how large they are. The index can be deleted and rebuilt at any
time with `glacier-store index`, so it needs no backup. The console's Data
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

- **`glacier-store` (Rust library):** list datasets, read a time range as one
  record stream across compacted days and sealed segments, verify files.
- **`glacier-store` (CLI):** `ls`, `verify`, `seal`, `compact`, `sync`,
  `index`, `export` (to Parquet).
- **Bindings for Python and Julia** over the same library, so no consumer
  parses files on its own.

Closed days are ordinary `.dbn.zst` files, so the standard DBN tools and the
Databento client libraries can read them too, apart from Glacier's custom
schemas.

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

## Open questions

1. **Raw container:** pcapng, or a custom framed log?
2. **Dataset naming for crypto venues:** for example `COINBASE.EXCHANGE`,
   `KRAKEN.SPOT`. Should Glacier keep a registry of names so deployments agree?
3. **Partition by symbol:** one file per dataset, schema and day containing
   every symbol (Databento's default), or an optional symbol level, as in
   `<DATASET>/<symbol>/<schema>/`?
4. **Schema extensions:** how Glacier's DBN extensions are named and
   versioned so readers can tell them apart from standard schemas.
5. **Timestamps:** which receive and exchange timestamps every normalized
   record carries, and how clock quality is recorded.
6. **Retention:** per-dataset rules for how long raw data stays on fast
   storage before moving or being pruned.

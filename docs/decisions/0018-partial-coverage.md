# 0018: Partial days, and buying only what's missing

- **Status:** Proposed
- **Date:** 2026-10-19
- **Builds on:** [0002](0002-data-store.md) (the layout, filing provider
  batches, coverage and the catalog), [0005](0005-store-service-and-supervision.md)
  (the store is the only process that writes the data disks),
  [0007](0007-storage-pool-and-automations.md) (purchased data is kept,
  refetchable data is not), and the `databento-jobs` service
  ([API](../api/databento-jobs.md))

## Context

The store files a day of a dataset as one file holding every contract the
provider sent: `OPRA.PILLAR/SPY.OPT/cbbo-1m/2026-10-02.dbn.zst` is the whole
SPY chain. `databento-jobs` buys whole parent symbols to match. With
[#31](https://github.com/dawson-shay/tinytrader/pull/31) it refuses a purchase
that overlaps days the store holds or jobs on Databento's list.

Data that covers only part of a chain has no place in that model, and it
already exists:

- A research repository streamed SPY `cbbo-1m` from Databento's timeseries
  API, not as batch jobs: same-day and next-day expiries, strikes within about
  5% of the day's price, 2022-09-01 to 2026-10-02, 1,016 days, 1.9 GB. Each
  file is Databento's DBN as delivered. Its header names the request:
  `OPRA.PILLAR`, `cbbo-1m`, `stype_in = raw_symbol`, the contract list (192
  contracts on 2022-09-01, 340 on 2026-10-02) and the time window (13:00 to
  24:00 UTC).
- Streamed data never appears on Databento's job list, so nothing records
  that it was bought, and those files are the only copy, on one Mac's internal
  disk.
- The trailing 12 months of `cbbo-1m` are free on the current plan. Older
  months are not, so the 2022 to 2025 files may cost money to replace.

If the store can't hold partial days, three things go wrong:

- the only copy of purchased data stays outside the store;
- consumers can't find it;
- a later purchase of the full chain pays again for the busiest contracts
  already held. Databento prices by volume, and near-the-money contracts are
  most of it.

Databento's batch API accepts an explicit contract list (`stype_in =
raw_symbol`), so buying only the missing contracts is possible. The limits
are the store's file model and `databento-jobs`, which takes one parent symbol
per request.

## Decision

The store records **which contracts, and which hours, each day holds**.
`databento-jobs` buys **only the contracts and days not held**, as contract
lists.

### 1. Coverage per day

Each filed day carries its coverage in the schema's `manifest.json` (0002),
beside the file's size and SHA-256:

```json
"2022-09-01": {
  "file": "2022-09-01.dbn.zst", "size": 812912, "sha256": "…",
  "coverage": {
    "kind": "contracts",
    "stype": "raw_symbol",
    "symbols": ["SPY   220901C00370000", "SPY   220901P00370000", "…"],
    "window": ["2022-09-01T13:00:00Z", "2022-09-02T00:00:00Z"]
  }
}
```

- `"kind": "complete"` means every contract of the parent for the whole UTC
  day, as a parent-symbol batch delivers it. Days filed before this record
  are complete.
- `"kind": "contracts"` lists exactly the contracts the file holds, from its
  DBN header, and the time window it covers. The store checks this when it
  files the day: every record's instrument maps to a listed symbol, and every
  record falls in the window.
- A partial day stays under the provider's dataset name, `OPRA.PILLAR`. Its
  records are Databento's, unmodified; it is fewer of them, not a different
  dataset. The catalog and every reader say "partial" wherever they say
  what's held, so a partial day is never taken for the full chain.

### 2. Filing data that didn't come from a batch

A new filing path, **import**, for DBN obtained some other way (a timeseries
request, a file from another machine):

1. Upload to `incoming/` as now, each file with its size and SHA-256.
2. The uploader names the source: provider, the request (dataset, schema,
   `stype_in`, symbols or rule, window), the tool and its commit, and when it
   was fetched. This becomes `provenance/<import-id>/import.json`, since there
   is no provider manifest.
3. The store reads each file's DBN header and refuses a file whose dataset,
   schema or day disagrees with where it would go. Coverage comes from the
   header, not from the uploader.
4. Filing, ledgers and recovery are as for a batch (0002).

### 3. A day filled in later

A later purchase for a day already partly held holds only the missing
contracts. The store **merges** it with the day's file:

1. Write the merged file as a new file: the two streams interleaved in
   timestamp order. The header symbol list and window are the union.
2. Check it: its record count is the sum of the two, every record of each
   input appears once, and the coverage is the union.
3. Record the swap in the `migrations/` ledger, with both inputs' hashes and
   the merged file's hash. Then link the merged file in place of the day's
   file. The old files are released only after the merged one is verified, as
   in 0002's moves.

A day stays one file, Databento-shaped, so readers don't change. When the
union covers every contract in that day's definitions for the whole day,
coverage becomes `complete`.

### 4. The catalog

`GET /catalog` reports each day as `complete` or `partial`, and gains
`GET /coverage?dataset=&symbol=&schema=&start=&end=`, which returns each
day's coverage in full. `databento-jobs` and research read this; nothing
reads `manifest.json` directly.

### 5. Buying only what's missing

`POST /price` and `/submit` accept either a parent symbol (`SPY.OPT`, the
whole chain) or a **rule**: the expiries (days to expiry from each trading
day) and the strike band (a percentage of that day's price, or a count of
strikes around the money). For each day of the request, the service works out:

```text
wanted   = the parent's contracts that day (from the store's `definition` day),
           filtered by the rule
held     = the store's coverage of that day
pending  = contracts in jobs on Databento's list that aren't expired
to buy   = wanted − held − pending
```

- Each day with something to buy becomes one batch request with
  `stype_in = raw_symbol` and that day's list, split where it exceeds
  Databento's per-request symbol limit. Days differ, so one list across many
  days would buy contracts already held on some of them.
- The quote prices exactly those requests and lists, per day, what's skipped
  as held or pending. `/submit` re-checks all three at submission, as now.
- With no `definition` day for a day, it is refused for that day until the
  definitions are bought. Definitions are cheap, and buying them first is the
  only way to know a day's contracts.
- A parent-symbol request whose days are partly held becomes contract lists
  for those days, and a parent-symbol request for the rest.

### 6. Fetching goes through the service

Research and other clients buy Databento data only through `databento-jobs`,
never through Databento's API directly, so every purchase is priced, checked
against what's held, written to the ledger and filed. Streaming requests are
the gap this record closes. Clients that need data quickly can still use the
timeseries API for **free** requests, but must then import what they keep
(section 2) the same day.

## The existing research data

1. **Back it up first,** as it stands, to a second disk: `research/out/cbbo1m/SPY`
   (1.9 GB) and `research/out/chain1m/SPY` (827 MB).
2. **Import `cbbo1m/SPY`** (section 2) as partial days of
   `OPRA.PILLAR/SPY.OPT/cbbo-1m`. Provenance: the fetch scripts
   (`research/cbbo_fetch.py`, `cbbo_gapfill.py`, `forward_fetch.py`) and their
   commit.
3. **Leave `chain1m/SPY` out.** Its files are Parquet extracts made from the
   full chain: research output, regenerable, not provider data. They belong
   with research's own outputs, not in a provider root.

## Alternatives considered

- **A separate dataset name for subsets** (such as `RESEARCH.SPY-0DTE-5PCT`).
  Readers would have to know about it, the records are Databento's, and the
  jobs service still couldn't subtract held contracts from a full-chain
  request. Rejected in favour of coverage on the provider's own name.
- **Several files per day** (`2022-09-01.part-1.dbn.zst`). No merge needed,
  but every reader would have to union files, and a day would stop being a
  file. Rejected: 0002 keeps a closed day a single Databento-shaped file.
- **Buy the full chain and drop the subsets.** Simplest, but it pays again
  for the busiest contracts, and before 2025-10 it isn't free.

## Plan and size

1. **This record**, then amend 0002 (coverage in manifests, import, merge).
2. **Store:** coverage in manifests and the catalog, `GET /coverage`, the
   import path, header checks. About 600 lines of production code and 400 of
   tests, on disposable roots.
3. **Import the research `cbbo1m/SPY` days,** after the backup, on the store's
   M2 volumes (about 1.9 GB of the roughly 120 GiB left before the 97%
   threshold).
4. **Merge** (section 3). About 300 lines and 300 of tests, with a crash test
   at each step.
5. **databento-jobs:** rules, per-day subtraction and contract-list requests.
   About 400 lines and 300 of tests, plus a live pricing check (free) against
   the imported days.

Stop and report if the store's part grows well past 1,000 lines.

## Open questions

- Databento's per-request symbol limit for batch jobs, and whether a request
  per day is acceptable for long ranges. Check against their documentation and
  a free quote before step 5.
- The research files cover 13:00 to 24:00 UTC. Does a full-day purchase for a
  partly held day buy the missing hours of the held contracts as well? This
  record says yes: coverage is contracts × window, and the missing part is
  bought.
- Whether `cbbo-1m` is the schema to standardise on for SPY research, or
  `cmbp-1` (already held for 29 days). This affects which purchases come
  next, not the design.

## Consequences

- The store can hold the only copy of purchased partial data, with provenance,
  and say exactly what it holds.
- Purchases never pay twice for a contract-day the store holds or a job
  already bought, whether the earlier data was a full chain or a subset.
- Readers see one file per day as before, but must check the catalog's
  `complete` or `partial` before treating a day as the whole chain.
- Clients lose direct Databento access for anything they keep. Buying goes
  through one place.

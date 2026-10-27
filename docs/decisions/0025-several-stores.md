# 0025: Several stores, for the jobs service

- **Status:** Proposed
- **Date:** 2026-10-27
- **Builds on:** [0019](0019-purchase-guards.md) (purchase guards),
  [0023](0023-composable-interfaces.md) (section 6, one store per data
  node), [0024](0024-crate-layout.md) (section 8, filing throughput)

## Context

A deployment has more than one data node, each running its own store over
its own drives (0023, section 6), and `databento-jobs` knows only one
store. What it buys next is many small jobs (hundreds of option parents in
`cbbo-1m`, newest months first): each fits one store, but not all of them
fit the same one. 0023 describes the full design (volume claims,
handovers, every reader merging every node); this is the part the jobs
service needs now, and nothing more.

## Decision

1. **The stores are configuration.** `databento-jobs` takes several stores
   (`--store`, repeated, or `JOBS_STORES`), the first the default. The
   inventory names them; nothing is discovered, so a store that is missing
   is known to be missing.
2. **Every read of what's held asks every store** and merges the answers:
   `/catalog` and `/coverage` for pricing and the purchase guards (each
   store's catalog explained by its own coverage, the pieces the union),
   the definitions' `/files`, and `/batches/databento` for what's filed (a
   batch filed on any store is filed). Each is one read per store per
   price, purchase or download, the stores asked at once.
3. **A purchase is refused while any store can't say what it holds** (0019,
   and 0023 section 6): it doesn't answer, answers anything but `200`
   (`503` until its first scan), or reports missing volumes. A download
   whose stores can't all say whether it is filed fails, and is tried again
   on the usual schedule.
4. **A job is filed whole, on one store.** The store holding the days a
   contract job merges into, if one does; else the first, in the order
   configured, with room for the whole job net of the room the downloads in
   flight to it hold (0024, section 8a), preferring one whose incoming
   folder already holds some of the job's files. No store with room now but
   one that may have once its downloads land: the job waits, as before. No
   store with room at all: it fails with each store's reason.
5. **The store doesn't change.** Each store answers for its own volumes, as
   today.

## Not taken

- **Splitting one job's days across stores**, and filing part of a batch.
  Considered for a job bigger than any store; the jobs now planned are
  small, so a job too big for every store is refused, as before. Request
  smaller jobs instead.
- **Volume claims and handovers** (0023, section 6): each store still owns
  the volumes configured on it.
- **Merged reads in other clients** (`store-client`, `tt store`, the
  console, the store producer): they still read one store.
- **A day held on two stores** is merged for the purchase guards (held if
  either holds it), never moved or deduplicated.

## Consequences

- Several stores fill side by side: a job lands where there's room, and
  what's held anywhere is never bought again.
- One store down stops every purchase and every download, by design.
- The day conditions are sent to each store for the days it holds.

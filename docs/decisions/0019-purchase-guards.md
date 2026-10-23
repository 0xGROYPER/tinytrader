# 0019: Purchase guards that fail closed, vendors by name, and a human yes

- **Status:** Accepted (2026-10-22)
- **Date:** 2026-10-20
- **Builds on:** [0002](0002-data-store.md) (the layout and catalog),
  [0007](0007-storage-pool-and-automations.md) (purchased data is kept),
  [0018](0018-partial-coverage.md) (partial days, buying only what's missing),
  and the `databento-jobs` service ([API](../api/databento-jobs.md))

## Context

`databento-jobs` refuses to buy what the store holds or what is on
Databento's job list, and `/price-missing` subtracts held and pending
contracts day by day. Two read-only reviews of that code (2026-10-20) found
ways it can still pay twice, or lose track of what was bought:

**Paying twice**

1. **No purchase lock.** Two `/submit` or `/submit-missing` calls can both
   pass the checks before either is sent. The ledger blocks only the
   identical request, and only while it is `sending` or `uncertain`, not once
   it is `submitted`. Overlapping requests never block each other.
2. **`/submit-missing` never reads the ledger.** A job bought here that has
   expired unfiled, or that doesn't show on Databento's list yet, isn't
   subtracted. `/submit` would have refused it.
3. **Jobs are matched by their whole symbols string.** A job for
   `SPY.OPT,QQQ.OPT`, for `ALL_SYMBOLS`, or for a list of SPY contracts doesn't
   count as overlapping a `SPY.OPT` request.
4. **"Holds nothing" when the store can't say.** Right after a store restart,
   `/catalog` answers 200 with no roots until the first scan. A drive that is
   unplugged, or not mounted when the store starts, drops out of the catalog
   and `/coverage` silently. Its days then look unheld.
5. **`/coverage` sees less than the catalog.** It reads only the filed layout,
   while the catalog also counts raw batch folders and the older layout.
   `/submit` refuses those days, but `/price-missing` would buy them.
6. **The same day on two volumes:** `/coverage` reports whichever volume it
   read last, so a partial copy can hide a complete one.
7. **A price that isn't a number skips the confirmation.** The check is
   `cost > 0`, so a NaN or negative price from the cost call needs no
   `confirm_usd`.

**Losing track of a purchase**

8. **Reconcile on a partial job list.** If some job details can't be read, an
   `uncertain` submission older than 10 minutes is still written as
   `not-submitted`. Its job is then never followed or downloaded, and after
   it expires nothing records it as bought.

**Gaps (never buys, doesn't fill)**

9. A partial day's time window is ignored: a held contract counts as held all
   day, so the research days' missing 00:00–13:00 UTC is never bought.
10. "Held" means a file with the right name exists. Size and hash aren't
   checked, and in the older layout an empty date folder counts.

**Vendors.** The catalog keys datasets by dataset id and symbol, not by
vendor. Alpaca's data today is `ALPACA.IEX` and `ALPACA.SIP` under an Alpaca
folder, and Databento's ids are its own (`OPRA.PILLAR`), so nothing collides
yet. But when a second vendor sells the same venue, its data would count as
held for the other vendor's purchases, or the reverse.

**Approval.** A paid submit must repeat the exact quoted cost
(`confirm_usd`). That proves the caller read the quote, not that a person
agreed. Any agent with a shell can do both steps.

None of the async purchase paths (`had`, `plan`, `live_jobs`, `reconcile`,
`submit`, `submit_missing`) has a test today. Only the pure helpers do.

## Decision

### 1. Until this is built, paid purchases are off

`databento-jobs` refuses any submit not priced at exactly $0 (`403`).
Free requests still go through. Pricing and downloads are unaffected. This
ships first, as its own change
([#41](https://github.com/dawson-shay/tinytrader/pull/41)).

### 2. One purchase at a time, against everything known

- **One lock** is held across the whole purchase: re-check, ledger write,
  send, ledger write. `/submit` and `/submit-missing` share it.
- **One "already bought" set** is used by both paths, per day and per
  contract. It is the union of:
  - the store's holdings (section 3);
  - every job on Databento's list that isn't expired;
  - every ledger purchase (`submitted`, `sending`, `uncertain`), whether or
    not its job is still listed.
- **Job symbols are parsed, not compared as strings.** A job's list is split.
  It covers the whole parent when the parent is a member, or when it is
  `ALL_SYMBOLS`. Its listed contracts of that parent count contract by
  contract. A job by instrument ID, or one whose symbols can't be read,
  blocks every day it overlaps.
- **Buying again what expired** names the exact job IDs it replaces
  (`again: ["OPRA-…"]`). A blanket flag is no longer accepted.
- **Reconcile never decides "not submitted" on its own.** An uncertain
  submission stays uncertain, blocking every overlapping purchase, until it
  is found on Databento's list, or until the operator resolves it (section
  5).
- **Names are canonical.** Dataset, schema and symbol are compared in
  Databento's spelling. A request is refused unless its dataset is on
  Databento's dataset list.
- **The job list is complete or we stop.** Each refresh asks for every state
  over the account's whole history. If the list can't be read in full, every
  purchase is refused (`503`).

### 3. The store says what it holds, or says it can't

- `/catalog` and `/coverage` answer `503` until the first scan is done.
- **Known volumes are remembered** in the store's own state, so a drive
  that's missing at startup shows as missing, not as absent from the list.
  Both answers carry `missing_volumes`. The jobs service refuses every
  purchase while that list isn't empty. A drive that's away for good is
  retired explicitly, which records what it held.
- **A drive is there only when it's mounted.** A remembered volume whose
  folder isn't a mount point, or that held data and now scans empty, is
  missing. An operator who really emptied one says so
  (`POST /volumes/{name}/accept-empty`).
- **`/coverage` covers everything the catalog counts.** Raw batch folders
  and the older layout appear as `complete`, with `filed: false`. A day on
  two volumes is merged: complete wins, otherwise the union of contracts and
  windows. A conflict is flagged.
- **No data file goes uncounted.** Every `.dbn` or `.dbn.zst` file on a
  volume that the known layouts don't claim is counted from its DBN header
  (dataset, schema, symbols, start, end), wherever it is: a backup, a
  scratch folder, a hand-extracted download. A header that can't be read
  makes the volume missing, unless the file changed in the last two minutes
  (still being written). The store's own receipts (`provenance/`,
  `migrations/`) hold nothing.
- **An operator can set a path aside.** One that can't be read and isn't
  data blocks every purchase until it's fixed or set aside, with a recorded
  reason (`POST /volumes/{name}/ignore`).
- **Answers are quick, and fresh when it matters.** `/coverage` answers from
  the last scan. A purchase asks with `fresh=1`, which waits for a scan
  started after the request, so data copied onto a drive by hand counts.
- **Held means verified** (not built yet). A day counts only if its file is
  in the store layout with a manifest record whose size matches. Until then,
  a file counts as held whatever its size, which never causes a purchase.

### 4. Vendors by name

- **Every dataset has a provider:** the store's provider name, from its
  top-level folder (case ignored: `Databento/` and `databento/` are the same
  provider). The catalog keys datasets by (provider, dataset, symbol). Data
  outside any provider folder has no provider. For a Databento purchase it
  counts as held whenever its dataset and symbol match, so data in a backup
  is never bought again, while another vendor's recordings never match.
- **Dataset ids stay the vendor's own.** Databento's IEX is
  `databento / IEXG.TOPS`, Alpaca's is `alpaca / ALPACA.IEX`. They are told
  apart by provider, never by renaming the vendor's ids.
- `/coverage` and `/files` take a **required** `provider`. `databento-jobs`
  asks only about `databento`.
- A future vendor's purchasing service uses the same contract: the store's
  holdings for its provider, its vendor's live job list, and the shared
  ledger and lock rules above.

### 5. A person says yes

- **Every paid purchase needs your approval;** free requests don't. A quote
  is listed in the console's jobs widget with the dataset, schema, days,
  contracts, windows, bytes and cost, and what was skipped as held, pending
  or bought here. You tap Approve or Decline, from any device.
- **Approve goes through the console's write gate,** the same as a live
  order: an allowed login, plus the order token when live orders need it.
  The console then submits the quote to `databento-jobs` with an approval
  token only it holds (`CONSOLE_JOBS_APPROVE_TOKEN`), your login, and the
  price you were shown. `databento-jobs` refuses a paid submit without that
  token, a login and the exact price.
- **The ledger records each approval** (who, when, the quote, the cost)
  before the purchase is sent.
- **The Databento key belongs to the services that use it.** Only the host
  running `databento-jobs` and the relays can decrypt it, not the operator
  key or an agent's shell. A purchase made around the service (by streaming
  from Databento's API) leaves no job and no ledger entry, so nothing could
  ever subtract it. That's how 15 research sessions were bought and then
  lost on 2026-10-20 to 22. The mini session is building this part.

### 6. The hours of a partial day

Coverage is contracts × window. For a held contract whose window misses part
of the day, `/price` buys the missing hours: one extra request per
day, for those contracts and that time range.

## Tests

Before paid purchases are turned back on, every finding above has a failing
test that then passes. The tests run against a fake Databento (job list, job
details, cost, submit, with delays and failures) and a store on disposable
roots:

- two concurrent submits;
- a submit lost after Databento accepted it;
- multi-symbol, `ALL_SYMBOLS` and contract-list jobs;
- a store restart and an unplugged volume;
- a day on two volumes;
- an unread job during reconcile;
- a ledger purchase that has expired;
- a paid submit without the approval token, a login or the exact price;
- a partial window.

## Built

- Pause on paid purchases: #41.
- Store (sections 3 and 4): #43.
- databento-jobs and the console (sections 2, 5 and 6): #44. Pricing and
  buying are one pair, `POST /price` (always only what's missing; a whole
  chain, a rule or explicit contract lists, with optional New York session
  hours) and `POST /submit`.
- Not built yet: the size check behind "held means verified", and keeping
  the Databento key from the operator key (with the mini session).

Paid purchases come back on once both are deployed and the approval token
is wired on the mini; the first one is a small, real request approved in the
console.

## Consequences

- No purchase goes through while anything about holdings or jobs is
  unknown. Some purchases will be refused that could have been safe; that's
  the trade.
- Agents can no longer complete a paid purchase on their own.
- A second vendor can be added without its data being mistaken for
  Databento's, or Databento's for its.

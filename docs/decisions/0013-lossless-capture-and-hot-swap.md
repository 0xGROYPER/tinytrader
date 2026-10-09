# 0013: Lossless capture and hot-swapped feeds

- **Status:** Proposed
- **Date:** 2026-10-09
- **Builds on:** [0006](0006-market-data-relay.md) (relays), [0011](0011-bundles-and-work-classes.md) (work classes)

## Context

On 2026-10-09 a Docker Desktop restart (to change its VM's memory) stopped
every container, the OPRA relays included, for about 9½ minutes in the
middle of a session: the VM stopped and the app hung while quitting. Live
readers (a 0DTE trader) lost their feed until the relays came back, and
anything recorded from them has that gap.

OPRA can be bought back from Databento, so this was costly but recoverable.
A centralised exchange's (CEX) live feed is not: its full order-book history
can't be had afterwards at any price. Capture must then survive deploys,
restarts and failures without a gap, and live traders must keep their feed
while the services behind it are replaced.

Hyphae has parts of this: feed and replay are peer publishers of one
consumer protocol, an aggregation layer merges feeds by each source's own
ordering, a feed serves its clients while one upstream connection
reconnects, the recorder is an independent subscriber, and reads splice
history to live with no gap or duplicate at the handoff. Its feed ports end
with the process, though: after a restart, clients reconnect.

## Decision

### Capture

1. **Capture lives outside anything restarted for other reasons.** A capture
   process is small, rarely changed and supervised on its own (launchd,
   systemd), not a container in a shared VM. Restarting Docker, rebuilding
   the console or deploying a strategy never touches it. Irrecoverable
   capture runs on the VPS (0001), with a twin elsewhere.
2. **Spool first.** A recorder appends every message to a local spool before
   anything else, and moves only sealed, verified files on (Hyphae's hot
   root). A crash loses at most what wasn't yet flushed.
3. **Two independent recorders for irrecoverable feeds,** on different
   machines and network paths, merged by the source's sequence numbers
   (Coinbase `sequence`, Binance update IDs, Kraken checksums): either can
   restart or fail while the other covers.
4. **Make before break.** A new version of a recorder starts beside the old
   one; both record until the new one is healthy and caught up; then the old
   one stops. The overlap merges by sequence number, so nothing is missing
   or doubled.
5. **Gap repair from the source where it allows.** On a sequence jump a
   recorder backfills trades from the exchange's paginated recent trades
   (by ID) and resyncs books from a REST snapshot plus the resumed stream,
   marking what was reconstructed. This shrinks small gaps; it doesn't
   replace 1-4.
6. **Planned windows.** Anything that takes down a capture domain waits for
   a window when the twin covers, or the feed's quiet hours.

### Hot-swapped feeds for live traders

7. **A stable edge that traders connect to,** separate from the relays that
   hold provider connections. Traders keep one connection to the edge; the
   edge subscribes to a relay.
8. **Handoff when ready.** To replace a relay, a new one starts beside the
   old, connects upstream and catches up. When it is healthy, the edge
   subscribes to it, splices the two streams at a sequence point (no gap, no
   duplicate, as Hyphae splices history to live) and drops the old one.
   Traders see one continuous stream and never reconnect.
9. **The edge itself changes rarely** and, when it must restart, offers
   resume: a trader reconnects with its last sequence and gets the missed
   suffix from a short buffer (the order gateway's `resume_after_seq`, for
   market data).

## Plan

1. Move the OPRA relays out of Docker Desktop to native services (and to the
   Mac Mini), so Docker restarts stop touching capture.
2. The edge with handoff (7-8) for the relays, then resume (9).
3. Twin recorders and sequence merge for the first CEX feed; gap repair.
4. Make-before-break deploys (4) in the deploy script.

## Consequences

- Deploys and restarts stop costing data or interrupting traders.
- Capture needs a second machine for anything irrecoverable.
- A new component (the edge) sits between relays and traders; it must stay
  simpler and more stable than what it shields.

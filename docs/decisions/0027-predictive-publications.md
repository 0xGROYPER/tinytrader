# 0027: Predictive publications

- **Status:** Proposed
- **Date:** 2026-10-19
- **Revised:** 2026-10-20, after the mailbox-topology review: delivery and ordering (rules 8 to 12, Plan step 5, Consequences).
- **Builds on:** [0001](0001-platform-and-deployment.md) (platform and deployment),
  [0002](0002-data-store.md) (the store), [0009](0009-trading-runtime.md) (the inbound port)
- **See also:** [0017](0017-client-contracts.md) (client contracts)

## Context

Models and strategies live in deployments, not in the platform (decision 0001, the scope table in CONTRIBUTING, and
decision 0009). A deployment that runs a model produces predictive distributions: quantiles, moments, a horizon, and a
target such as a future quantity. Other clients need to read them: a console view, a scorer, a strategy client in another
deployment. Today nothing defines how such a distribution is described, stored, or labelled.

Without a shared definition, two problems arise. A consumer cannot tell a distribution under the risk-neutral measure (a
pricing input) from one under the real-world measure (an expectation of outcomes). And it cannot tell a model whose forecasts
have been promoted on held-out evidence from one still under test. Both distinctions decide whether a number may be used to
size a position.

The platform does not need to understand any model. It needs to store the record, pin its inputs, and make its status
visible.

## Decision

The platform defines a publication schema, `tinytrader.predictive/1`, for predictive distributions produced by
deployments. A publication is written through the inbound port of the trading runtime (decision 0009) and stored through the
store service (decision 0002), like run results: immutable, pinned by hash, and readable by time.

The platform computes no predictive distribution. It validates the record, stores it, and serves it.

Each publication carries:

- the producer's name and version, and the hashes of its code and of its inputs;
- the time it is valid as of (`as_of`), and optionally until when;
- the target, in the producer's declared vocabulary, with its units;
- a **lane**: `risk_neutral` or `real_world`;
- a **status**: `unpromoted`, or `promoted` with a reference to the evidence that promoted it;
- quantiles and, optionally, moments;
- a typed gap marker when the producer had no valid input for the interval;
- a sequence number, contiguous within its stream (rule 8). A gap marker takes a number like any other publication.

Readers read publications from the store, by time and by number. The platform does not push them to readers in this decision.

## Design rules

1. **The platform stores and serves predictions. It computes none.**
2. **The lane is mandatory.** A `real_world` publication without a `promoted` status is refused when it is written.
3. **No look-ahead.** A reader asking for a time before a publication's `as_of` receives nothing from it.
4. **Gaps are published as gaps.** The platform never fills one (decision 0009, "missing is missing").
5. **Immutable.** A correction is a new publication that refers to the one it corrects. Nothing is edited in place.
6. **Provenance is required.** A publication without a producer identity and input hashes is refused.
7. **Schema changes are additive within a major version**, as in every other platform schema (decision 0017).
8. **Sequence is part of the record.** A stream is one producer name, one instrument, one target (kind and params) and one lane. Each stream numbers its publications 1, 2, 3, with no gaps, gap markers included. The producer assigns each number from a counter it keeps with its state. The runtime does not assign numbers on arrival, because an arrival number would change the record hash and break the determinism check in Plan step 3. The store accepts a write only if its number is the next one for its stream, or if it repeats an accepted publication exactly (same hash, which changes nothing).
9. **The store is the delivery path.** A publication is durable in the store before anyone can read it. Readers pull by time (rule 3) and by number, so a slow reader never affects the producer or another reader.
10. **A missing number is an integrity failure, not an answer.** A producer gap says the model had no valid input for an interval, and it is published as a gap (rule 4). A missing number says a publication did not reach the store. A reader treats a gap as "no answer". It does not use a stream past a missing number until that number is resolved.
11. **No push in this decision.** A live subscription, if one is added later, must not lose a publication silently. It follows the order gateway's rule (a bounded per-client queue, `slow_consumer` on overflow, resume from the store by number), not the market-data relay's rule (a client that falls too far behind is disconnected and told that records were skipped). The relay's rule suits market data, where a skipped record is a fact about the feed. A missing publication here is a defect.
12. **No global order.** Within a stream, order is by number. The platform implies no order across streams. A reader that needs one states its own rule, for example by `as_of`.

## Plan

1. The schema and its validation as a library, with the refusal cases tested: an unpromoted real-world record, missing
   provenance, and a hash mismatch.
2. One publication round-tripped through the store: written, read back at its `as_of`, and checked against its hash.
3. A determinism check: the same inputs produce the same record hash.
4. The record is then delivered through the 0009 inbound port, once that prototype passes its own gate.
5. The numbering rules, tested: a skipped number is refused, an exact repeat is accepted once, and a restarted producer resumes from the store's last number for its stream. Rule 8 needs a store read of the last number per stream, and store.md defines neither that read nor the repeat rule yet.

Steps 1 to 3 and 5 do not depend on the trading runtime and may proceed first. Step 5 can use a fake store until store.md covers those two rules.

## When to reverse

- Decision 0009 is reversed. Publications have no delivery path without it, and this decision is withdrawn with it.
- The store cannot guarantee immutability of a pinned record.
- The store cannot refuse a skipped number in the same write that stores the record, so rule 8 cannot hold.
- No deployment publishes through the schema within the window set by decision 0009.

## Consequences

- Consumers outside a deployment can read a prediction together with its status, and cannot mistake an unpromoted result for
  a promoted one.
- The platform gains a small, model-neutral schema to keep stable. It takes on no model logic.
- Producers must supply provenance and a lane for every publication, which is more work than an untyped file.
- The lane check makes the real-world/risk-neutral distinction a property of the record, not of the consumer's care.
- A slow reader cannot stall a producer or another reader, because readers pull from the store.
- Producers keep a counter per stream that must survive restarts. An untyped file would not need that state.

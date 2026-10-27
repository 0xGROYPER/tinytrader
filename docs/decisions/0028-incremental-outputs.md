# 0028: Incremental outputs

- **Status:** Proposed
- **Date:** 2026-10-27
- **Builds on:** [0009](0009-trading-runtime.md) (the trading runtime),
  [0026](0026-external-process-actors.md) (external-process actors)

## Context

A system that follows equities, futures and options together sees millions
of events a day, and most of what it keeps (rolling statistics, books, bars,
greeks, features, cross-sectional ranks, portfolio aggregates) depends on a
small part of them. Recomputing every output on every event, or on every
snapshot of the whole universe, doesn't scale with the universe; updating
only what an event affects does.

How much that saves depends on the output and its cadence. A measurement on
option chains (contracts 7 to 45 days to expiry, on a five-minute grid)
found a median of about 90% of contracts changed in each step: a snapshot of
a dense chain at that cadence leaves almost nothing to skip, and
recomputing it whole costs the same as updating it. The same chain read
event by event, or a wide universe in which an equity tick moves one name
and its chain, leaves almost everything to skip. So incremental computation
is a property chosen per output, not a verdict for the runtime as a whole.

## Decision

### 1. Outputs are actors with declared inputs

A derived output (a feature, an estimate, an aggregate) is an actor, or part
of one, whose subscriptions name its inputs: instruments, other actors'
outputs, timers. 0009's routing already delivers an event only to the actors
that asked for it, so an event that touches one instrument reaches only the
outputs that depend on it. Nothing else runs.

### 2. Each output says how it updates

- **Incremental:** it updates its state from each delivered event (a running
  sum, an exponentially weighted mean, a book, a greek from a new spot) and
  publishes a change only when its value changes.
- **Recomputed:** it recomputes from its inputs on a timer or a batch
  boundary (a whole-chain fit every few minutes), when its inputs change
  densely enough that an update costs what a recompute does.

Both are ordinary actors; the runtime gives neither a privileged path (0009
rule 2). An output can change mode without its subscribers noticing, since
what they see is its published value.

### 3. Changes flow, unchanged values don't

An output publishes when its value changes, not when it was asked, so a
change propagates only along the dependencies it affects. Cross-sectional
outputs (ranks, portfolio totals) update from their inputs' deltas where the
statistic allows it, and recompute where it doesn't, as section 2 says.

### 4. Measured, not assumed

Whether an output gains from incremental updates is measured on its real
inputs: the share of its inputs that change per step, at the cadence it
runs. A run's records keep each actor's handled events and service time, so
the measurement comes from the run itself. The finding above (dense chains
at a five-minute cadence) is one such measurement, and decides that output,
not the runtime.

### 5. Across languages and machines

An output may be a Rust actor or an external-process actor (0026). The
high-rate incremental work belongs in Rust actors, whose published changes
feed slower consumers, Python ones included, at the rate they can take.

## Consequences

- The runtime scales with what changes, not with the size of the universe.
- An output's update mode is its own choice and can be revisited with
  measurements, without changing the outputs that read it.
- No dependency-graph engine beyond 0009's routing and actors publishing
  changes: the graph is the topology.

# 0026: External-process actors

- **Status:** Proposed
- **Date:** 2026-10-27
- **Builds on:** [0009](0009-trading-runtime.md) (the trading runtime: actors,
  virtual time, the `(ts, class, seq)` order), [0017](0017-client-contracts.md)
  (client contracts), [0022](0022-sessions.md) (sessions, lockstep, client
  libraries), [0023](0023-composable-interfaces.md) (composable interfaces)

## Context

Research often happens in Python notebooks, while the runtime of 0009 is
Rust. Keeping the two apart means writing a strategy twice: once to explore,
once to run. 0022 already lets a strategy in any language be a *session
client*, outside the runtime, reading whole streams over TCP. What's missing
is the step between: an actor **inside** a run, written in another language,
so that the runtime's routing, filters and clock serve it like any other
actor, and the same code runs in a notebook, a backtest and live.

Embedding an interpreter in the runtime (FFI) would tie the runtime to that
interpreter's crashes, threads and packaging, against 0009's rule that
Python bindings are optional. DBN is already a byte format with a native
decoder in Python (`databento-dbn`, the same Rust decoder), so a separate
process costs no translation layer.

## Decision

### 1. An external actor is an actor whose handler is a process

A run declares an actor kind as **external**: a command to start, its
subscriptions, its mailbox policy and its declared service time, as for any
actor (0009). The runtime starts and supervises the process. It keeps
everything 0009 makes the runtime's: the order of deliveries, the virtual
clock, the mailbox and its policy. The process sees only what is delivered
to its actor, and answers.

### 2. The wire

One bidirectional byte stream per external actor: the process's stdin and
stdout by default, or a Unix or TCP socket for a process on another machine.

- **To the process:** DBN, unchanged. A DBN metadata header (the run's
  symbology and schema, or *mixed*, as 0022 section 4 says), then the
  delivered records as they are, with no re-encoding. The runtime's own
  messages travel as DBN extension records (an unused rtype, 0009 rule 1):
  `turn{as_of}` closes a delivery batch and hands the process its turn;
  `timer{id}`, `gap{…}` and venue reports are records in the stream at their
  place in the order.
- **From the process:** newline-delimited JSON in one versioned schema,
  `tinytrader.actor/1`, in the generated contract (0017; 0021 section 5).
  It carries the order commands of the gateway protocol (submit, cancel,
  replace), timers to set, messages to other actors (by actor name, with a
  JSON payload the run's message type declares), and `done{as_of}`, which
  ends its turn.
- Stderr is the process's log, kept with the run.

### 3. Lockstep, as sessions have it

After a batch, the runtime waits for `done{as_of}` before it advances the
actor again. Commands sent in a turn take effect at `as_of` plus the
actor's declared service time, never its wall time, so a backtest is as
deterministic with a Python actor as with a Rust one. In live runs the
protocol is the same and the process's latency is real; the run record says
so. A process that misses a stated deadline for its turn, exits or writes
something unreadable is a failure, never skipped over (0009 rule 5: missing
is missing).

### 4. Filtering before the process

A Python process can't keep up with every record of a tick-level feed, so
the topology, not the external actor, decides how much reaches it:

- Rust actors in the same run filter, aggregate and estimate (a smoothed
  mid, bars, a fitted surface, features), and the external actor subscribes
  to their output instead of the raw feed.
- Their output travels as DBN too: standard records where one fits (OHLCV,
  CBBO), otherwise an extension record the run declares.
- So the split between fast and flexible is a topology choice, made per run,
  and the same Python code can read raw records over a quiet chain or curated
  ones over a busy feed.

### 5. Supervision and bounds

- **Failure:** each external actor declares what its failure does. In a
  backtest the only choice is to stop the run, marked incomplete. Live, the
  default is to stop the run and leave the venue boundary's safety rules to
  act. Never a silent restart.
- **Bounded** (0009 rule 6): the mailbox before the process, the pipe, and
  each answer line (a stated maximum size) have fixed capacities.
- **Isolation:** the process holds no venue credentials and no store or
  relay connection of its own; everything reaches it through the runtime.

### 6. One class, three places

The client libraries of 0022 section 7 gain an actor interface, in Python
first: a class with `on_record`, `on_timer`, `on_report` and the commands
(`submit`, `cancel`, `send`, `set_timer`). The same class runs:

- **in a notebook**, fed from the store's held days by the library's replay
  driver, with the same lockstep semantics and the same order;
- **as an external actor** in a run (`run_stdio()`);
- **as a session client** (0022), when it should read whole streams outside
  a run.

Moving research to a backtest or to live then changes where the class runs,
not the class.

## Steps and budgets

Each its own PR, after 0009's runtime step that executes actors. Production
/ tests lines.

| Step | Prod | Tests |
|---|---|---|
| 1. The external actor kind: start, supervise, the wire both ways, lockstep, failure | 450 | 450 |
| 2. `tinytrader.actor/1` in the generated contract; extension records `turn`, `timer`, `gap` | 120 | 150 |
| 3. The Python actor interface, `run_stdio()` and the notebook replay driver; one example actor over a filtered stream | 350 | 300 |
| 4. Remote placement over a socket | 120 | 120 |
| **Total** | **1,040** | **1,020** |

## Consequences

- Python research code and live code can be the same class; the boundary is
  a process, not an FFI, so neither side's crashes or packaging reach the
  other.
- A run can mix Rust and Python actors, with Rust doing the high-rate work
  and Python the decisions or models, chosen per run.
- Determinism in backtests holds for external actors through lockstep, at
  the cost of one round trip per turn.
- Not doing now: an embedded interpreter, shared-memory transport, columnar
  (Arrow) batches. Each could come later as an optional crate if a run
  measures the pipe as its bottleneck.

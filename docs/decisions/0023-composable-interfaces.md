# 0023: Composable interfaces, a platform rule

- **Status:** Proposed
- **Date:** 2026-10-26
- **Builds on:** [0003](0003-front-ends.md) (one data layer, any number of
  front ends), [0017](0017-client-contracts.md) (client contracts),
  [0021](0021-acquisition-verbs-and-shared-plumbing.md) (acquisition: roles,
  verbs, addresses, `tt`, layered libraries), and
  [0022](0022-sessions.md) (sessions)

## Context

0021 and 0022 apply one ethos, taken from Hyphae, to acquisition and to
sessions: every capability is an endpoint; a command line, a TUI and the
console are front ends over the same endpoints; commands have one input and
one output and exit truthfully, so Bash and a scheduler compose them; and
libraries hold no process concerns. The rest of the platform doesn't follow
it yet. The store, the gateways, the console's writes, the news and calendar
services each have their own shape, and some things (approving a purchase,
until 0021) can only be done in the web page.

This makes the ethos a rule for all of tinytrader, so a new service starts
there and an old one moves there when it is next touched.

## Decision

### 1. Every capability is an endpoint, and every endpoint has a `tt` command

- What a service can do, it does through a documented endpoint (HTTP, or a
  stream protocol for the relay and the gateways), in the generated contract
  (0021 section 5). Nothing a person can do is reachable only from the
  console's page or only from a script.
- `tt` has a command for each: `tt store coverage …`, `tt store files …`,
  `tt fetch …`, `tt quotes`, `tt orders …`, `tt session …`, `tt news …`.
  The console and the TUI are front ends over the same endpoints; none of
  the three holds logic the service doesn't own.
- Writes from `tt` go through the same guards as from the console: the
  console's write gate and login where a person is needed (0021 section 4),
  a service's write token where a service is.

### 2. Commands compose

Every `tt` command, and every process tinytrader ships, keeps one contract:

- **One input, one output.** Inputs: `--connect ADDR`, `--input -`, or the
  command's arguments. Outputs: `--publish ADDR`, `--output -`, `--write
  store`, or its answer on stdout. Addresses have a scheme (`tcp://`,
  `unix://`, `http(s)://`).
- **stdout is data only.** Protocol bytes or the answer (JSON by default,
  `--format table` for people); logs and progress go to stderr.
- **Exit codes are truthful:** 0 done, 1 failed, 2 usage, 130 signalled; a
  partial result is a failure that says what it got.
- **Foreground.** A command runs until done and stops on a signal, cleaning
  up only what it started. Whatever must outlive a terminal runs as a job or
  service in the deployment's scheduler (0020); tinytrader ships no
  supervisor.
- **No secrets in arguments.** Credentials come from the environment or a
  `*_FILE` (0017), never from argv.
- **Help is the reference for flags**; the API pages are the reference for
  behaviour.

### 3. The console is a client like any other

The console reads the same endpoints `tt` and strategies do, by capability
(0017), with no data path of its own. Today it polls services' JSON views
(the relay's bars, quotes and ticks; alpaca-data's snapshots and chains)
through `/api/bars/{service}` and `/api/stream/{service}/{stream}`. Under
this rule:

- **Those views are `fetch`es** (0021 section 2): documented in the
  generated contract, each with a `tt` command over the same endpoint
  (`tt bars relay SPY --days 2`, `tt chain alpaca SPY`).
- **Live widgets `listen`.** The relay serves its stream over a WebSocket as
  well as TCP: the same records, the same join and the same markers, so a
  page is one more subscriber, cut off like any slow one. The page decodes
  the few schemas it shows (bars, BBO, trades) instead of polling.
- **The console server stays a front end** (0003): it proxies, adds the
  write gate and holds no market-data logic.

### 4. Services and processes

The rule shapes interfaces, not process lifetimes:

- **Long-lived things stay services:** the store (the only writer to the
  data disks, 0005), the relay, the order gateways, the console. A command
  line is their front end and their contract, not a reason to make a daemon
  a pipe.
- **Short-lived work is a process per run:** a fetch, a session (0022), a
  sweep, an import. It starts, does one thing, reports and exits.

### 5. Libraries carry the logic

0021's layering is the rule for all code: foundation, plumbing, venues and
clients as libraries; services and front ends on top. A library never
depends on a service or an app, nor a layer on one above it (checked in
CI), and holds no process concerns (argument parsing, environment or secret
lookup, signals, exits, runtime startup). So the same code runs behind HTTP,
a socket, a pipe or a test.

### 6. How it's applied

- **New services and commands** follow it from the start; a review checks
  them against sections 1 to 5.
- **Existing services move when they are next touched**, in the PR that
  touches them, not in a rewrite. Each such PR lists which parts of the
  rule it brings that service to, and its lines added and removed.
- **The order** follows need: acquisition and the approval command line
  (0021), sessions (0022), the console's data views and a WebSocket
  `listen` on the relay, then the store's reads, the gateways' operator
  actions (`tt orders`, `tt flatten`, behind their existing tokens), and the
  rest as they come up.
- **Exceptions** are written down in the service's API page with the
  reason (for example, a stream protocol that can't be a request and
  answer).

## Consequences

- Anything the console can do, a terminal and a script can do, with the
  same guards; anything a script does is a command a person could type.
- A service's review is against one rule rather than its own conventions.
- Moving services over adds work to PRs that touch them; the "lines added
  and removed" line keeps that visible, and moving code into libraries
  should often remove more than it adds.
- Not doing: a rewrite, a plugin or connector framework, or a process
  supervisor.

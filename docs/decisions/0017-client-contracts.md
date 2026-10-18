# 0017: Client contracts, and keeping deployments out of the platform

- **Status:** Accepted (the conventions and the fixes made with it); the proposals below are Proposed
- **Date:** 2026-10-16
- **Builds on:** [0001](0001-platform-and-deployment.md) (platform and deployment),
  [0003](0003-front-ends.md) (one data layer), [0006](0006-market-data-relay.md)
  (the relay), [0009](0009-trading-runtime.md) (the order gateway)

## Context

tinytrader's servers are the boundary between markets and code: relays for
live DBN, the store for historic DBN, order gateways for brokers (and the
simulated broker for paper and backtest). Strategies are clients of them,
written in other repositories and other languages. That only works if the
interfaces are written down, consistent, and free of one deployment's habits.

An audit of every client-facing interface found them mostly consistent
(`{"error": ...}` bodies, `tinytrader.<name>/<n>` schema names, `NAME_FILE`
secrets, observe endpoints everywhere) but undocumented in places (the
console's `/api/*`, the topology file, most services), with a few needless
differences, and with assumptions from the one deployment that grew it:
default symbols, account and service names, ports, Docker networking, and
personal console content.

## Decision

- **The reference lives in [`docs/api/`](../api/README.md)**, one page per
  interface on one template (purpose, transport, versioning, auth, errors,
  health, stability), with the shared conventions in its README: schema
  names, the error body, tokens and `*_FILE`, units, default ports and
  extension points. A service's README points there instead of holding the
  contract.
- **Each interface is marked stable or experimental.** Stable interfaces
  change additively, and a breaking change ships under a new schema name
  beside the old one. Experimental ones may change with their in-tree
  client.
- **Front ends and clients find services by capability**, the names a
  service declares in `/describe`'s `streams`, never by id, and the
  console names no service, account, symbol or broker in its code.
- **Platform defaults are generic.** A choice one operator made (which
  option roots a gateway allows, which directories it refuses, which ids
  the console's clients use) is a flag or variable with a neutral default,
  or no default at all.
- **Personal workflow features are optional modules.** Prefs, reminders and
  profile recordings are off unless configured, and answer `404` then; a
  deployment's curated views come from `CONSOLE_PREFS_DEFAULTS`, not from
  the console's code.

### Fixed with this decision

- The relay join request accepts `name` as well as `client` (the old
  documentation said `name`, which the relay refused).
- The order gateway allows no option roots by default (it was `SPY`):
  `--underlyings` names them. Its state-root check no longer names one
  machine's volume; `--forbid-state-under DIR` adds a deployment's own.
- alpaca-data's default port is 9125 (it shared 9123 with the queue).
- databento-jobs and alpaca-history refuse writes from another site's page,
  as the store and the queue do. The token guard's message no longer says
  "store".
- The console API's errors are JSON everywhere outside the optional modules;
  a missing order gateway is `404` (it was `400` or `403`); its gateway
  client names and order prefix are configurable (`CONSOLE_GATEWAY_CLIENT`,
  `CONSOLE_TRADE_CLIENT`, `CONSOLE_TRADE_PREFIX`, defaults unchanged), and
  `/api/orders` says each gateway's `broker`.
- `/api/topology` and `/api/state` carry `"schema": "tinytrader.fleet/1"`.
- The console finds the ticker directory and account performance by
  capability (no fallback to `econ-calendar` or `accounts`), names
  capabilities rather than services in its messages, offers the option
  roots its relays carry instead of `SPY`/`XSP`, reads the order prefix
  from `/api/orders`, and doesn't call every account "Alpaca"; the terminal
  UI orders relays by the topology's `focus` instead of a fixed list.
- Examples in code comments no longer name `host.docker.internal` or one
  deployment's accounts.

## Proposals

1. **Observe endpoints on the order gateway.** It serves no HTTP today, so
   it has no `/health` or `/metrics`, and the console needs
   `CONSOLE_GATEWAYS`. An optional `--observe ADDR` serving the standard
   three, with `/describe` declaring `{"gateway": {"protocol":
   "tinytrader.gateway/1", "listen", "account", "environment"}}`, lets the
   console discover gateways like relays. It must stay a separate,
   read-only thread: nothing in it may touch the session.
2. **JSON for framework rejections.** A missing or unparsable query or body
   is answered with axum's plain-text `400`/`415`/`422` by alpaca-data,
   databento-jobs, queue, alpaca-history, econ-calendar and the console.
   A shared `Query`/`Json` extractor in `libs/observe` that rejects with
   `{"error": ...}` makes every error body one shape.
3. **Write tokens everywhere writes exist.** The queue, databento-jobs (which
   spends money), alpaca-history and the console's own actions have only
   the same-origin guard, so anyone on the network can use them. Use
   `guard::WriteToken` with `<SERVICE>_WRITE_TOKEN` (or `_FILE`), off by
   default as the store's is; the console takes `CONSOLE_TOKEN` for its
   actions.
4. **`/health` while idle.** It answers `503` for `idle`, a deliberate state,
   so an off-hours service looks down to Docker health checks and load
   balancers. Answer `200` for `idle` in `tinytrader.observe/1` (state is
   already in the body, which is what tinytrader's own readers use), after
   checking deployments' health checks.
5. **Consistent configuration names.** `ALPACA_OPTIONS_FEED` and
   `ALPACA_HISTORY_FEED` (alpaca-data), `JOBS_*` (databento-jobs),
   `HISTORY_*` (alpaca-history), `EARNINGS_*` and `ECON_CONTACT` without a
   `_FILE` form (econ-calendar), alpaca-news's `--keep` without a variable.
   Add the `<SERVICE>_` spellings as aliases; keep the old ones.
6. **econ-calendar's `/events`** is its release calendar, while everywhere
   else `/events` is the observe event log. Serve it also as `/releases`,
   declare that, and give the service a real event log.
7. **Discover the console's write targets.** The jobs service, the queue and
   the store are configured by URL (`CONSOLE_JOBS`, `CONSOLE_QUEUE`,
   `CONSOLE_STORE`) though they declare `jobs`, `queue` and `catalog`.
   Allow `CONSOLE_JOBS=@<service id>` (an explicit opt-in still, since these
   write) and resolve the rest from the topology.
8. **More than one provider of a capability.** "First service with the
   field" can't choose between two chain services. A topology
   `"prefer": {"chain": "<id>"}`, honoured by the one pair of helpers in
   `web/lib/fleet.js` (`serviceFor` and `servicesFor`, which read the
   topology channel), and the remaining direct topology lookups moved onto
   them.
9. **Addresses that aren't Docker DNS.** Discovery assumes
   `http://<compose service>:<port>`; the terminal UI connects to relays at
   `--relay-host` and the port they listen on. A `tinytrader.observe.url`
   label, and an optional `relay.connect` in `/describe` or the topology,
   cover NAT, published ports and other runtimes.
10. **The console's starter content** (watchlist starters, the theme presets,
    the board and relations pairs, the layouts installed on first run, the
    VIX and yields views) reflects one operator's interests. Ship a small
    generic set, and let a deployment supply its own through
    `CONSOLE_PREFS_DEFAULTS`. `CONSOLE_STATE_DIR` should hold prefs,
    profiles and reminders, instead of the first two defaulting to the
    reminders file's directory, and reminders should drop "roadmap" from
    their names and seeding.
11. **Market assumptions** belong in one place: `America/New_York` is
    written inline in about twenty places in the page; regular hours
    (09:30 to 16:00) are the default close in the queue; the TUI labels
    times "ET". One market-calendar module per side, configurable later.
12. **Hosts compiled into data services.** alpaca-data reads option contract
    lists from the live trading host (`api.alpaca.markets`) whatever the
    key; alpaca-history reads its calendar from the paper host. Make both
    follow the key's environment, or a variable.
13. **databento-jobs uses up a quote on a refused confirmation** (`428` or a
    changed price), so the client must price again; keep the quote until
    the submission is attempted.
14. **The queue's documents** are `camelCase` with capitalised states, unlike
    every other service. Leave them (they follow the Kubernetes-style
    templates) unless the queue gets a `/2`.

## Consequences

- A strategy author can write a client from `docs/api/` and the
  [tutorial](../tutorials/strategy-client.md) without reading the servers'
  source, in any language.
- Deployments that relied on the gateway's `SPY` default must pass
  `--underlyings`; a gateway without it logs that every option order is
  refused. A deployment that wants the old volume check passes
  `--forbid-state-under`.
- A deployment that relied on alpaca-data's default port must set
  `ALPACA_DATA_LISTEN`.
- New interfaces start in `docs/api/` with the template, and the generic
  check (`nix flake check`) keeps addresses and home paths out of them.

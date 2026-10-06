# 0003: One data layer, any number of front ends

- **Status:** Accepted
- **Date:** 2026-10-05

## Context

The operator console started as a web page with its data logic inside the
web server: discovery, `/describe` documents, overrides and polling. Some
situations call for a terminal instead, such as working over SSH or on a
host without a browser. A second front end must not mean a second copy of
that logic.

## Decision

- **`libs/fleet` owns the data layer.** It discovers a deployment, polls its
  services and produces the topology, snapshots and event pages. Its JSON is
  the console API (`tinytrader.fleet/1`); `Topology`, `Snapshot` and
  `EventsPage` are the same data, typed.
- **Front ends stay thin.** The web console serves what a `Poller` collects;
  `apps/tui` renders it with Ratatui. Neither re-implements discovery,
  polling or status rules: status colours come from `ServiceState::status`.
- **Two ways to read, one interface.** `Source::Direct` embeds a poller;
  `Source::Console` reads a running console's API. A front end works with
  either.

## Consequences

- A change to how services are discovered or summarised lands once and
  reaches every front end.
- The console API is a contract: change it additively; the typed model
  tolerates unknown and missing fields.
- Direct mode must reach the services at their discovered addresses, so it
  suits the deployment's own network or host; console mode works anywhere
  the console is reachable.

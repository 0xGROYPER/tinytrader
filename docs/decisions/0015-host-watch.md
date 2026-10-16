# 0015: A host watches itself, and says so to the phone

- **Status:** Accepted
- **Date:** 2026-10-15
- **Builds on:** [0005](0005-store-service-and-supervision.md) (supervision),
  [0009](0009-trading-runtime.md) (the order gateway)

## Context

The console pushes a notice when a service goes down (`CONSOLE_NOTIFY`), but
it runs in a container: when the container engine stops, so does the only
thing that could say so. A deployment once lost its container VM for fifteen
hours to a full disk, and nothing told the operator. The console also counted
only exited and dead containers as down: a restart loop and a service
reporting `failed` were only "needs attention", and a container that
disappeared was skipped.

## Decision

- **`watch`** (`packages.<system>.watch`, `nix/watch.nix`) is a small shell
  program run natively on each host, beside the engine rather than inside it.
  Every minute it checks what it is configured to: the Docker engine (a socket
  ping), the Compose services expected on the host (present, running, restart
  count not rising), the console's `/api/state` (health `failed`, or
  `degraded` for three minutes), the store's `/health`, native services
  (launchd or systemd) and disks. The deployment says what is expected, from
  its own placement; nothing is hardcoded.
- **Alerting is quiet by default.** A problem counts when seen two runs in a
  row; high-priority ones repeat every 30 minutes, warnings every 6 hours,
  and a "resolved" notice follows. Notices go to an ntfy-style URL; after each
  run it pings a heartbeat URL (healthchecks.io), so a host that stops
  altogether, network and all, is noticed by the heartbeat service. A notice
  that can't be delivered fails that run's heartbeat instead.
- **It never connects to an order gateway.** Gateways speak an authenticated
  protocol and may trade real money: they are judged by their containers'
  state and restarts only. URLs on the gateways' ports are refused.
- **It runs as `tinytrader.services.watch`**, the generic service definition:
  a launchd agent on macOS (or, with `launchd = "daemon"`, a boot-time
  daemon; it finds native services run as either), a systemd unit on Linux,
  with its secrets as credentials (`WATCH_NOTIFY_FILE`, `WATCH_HEARTBEAT_FILE`).
- **The console's rule for down** (libs/fleet `ServiceState::status`, and the
  page's `statusOf`) now includes a restarting container and a service
  reporting `failed`; its notices also say when a container it had seen is
  gone. `CONSOLE_NOTIFY_FILE` is read like other secrets.

## Consequences

- A stopped engine, a filling disk or an unplugged drive reaches the phone
  within about two minutes, and a dead host within the heartbeat's grace.
- The console and `watch` may both report the same container going down. The
  console's notice is fast (seconds) and dies with the engine; `watch`'s is
  slower (minutes) and doesn't.
- `watch` is configured per host by the deployment; a host with no list of
  expected services checks only what it is given.

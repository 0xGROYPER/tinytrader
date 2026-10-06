# 0011: Bundles and work classes

- **Status:** Accepted
- **Date:** 2026-10-06
- **Amends:** [0007](0007-storage-pool-and-automations.md) (automations)

## Context

Several services keep the same hours: five relays and the accounts service
each take an `--hours` option, and the jobs service has its own market-hours
pause. A change to the session means editing each one.

The pause is also the wrong rule. The jobs service stays off the network from
09:25 to 16:20 on trading days whether or not anything needs protecting, so
it sits idle on a day the relays aren't running, yet it does nothing about
the work that actually hurt the relays on 2026-10-06: builds and backtests
from other sessions on the same 8 GB machine, which made Databento's gateway
skip records.

## Decision

### Bundles

A **bundle** is a named group of work with shared hours, defined by the
deployment, not in code:

- a name, such as "US session";
- hours on the market calendar, such as 07:55–17:05 ET on trading days
  (early closes shift them, as relays do now);
- its members: services or automations;
- a class (below).

Bundles live in the deployment's inventory: the topology names each
bundle's members and class, and the hours are written once in the
deployment's Compose file and shared by every member (a member may keep
narrower hours, as the options relays keep the session inside the wider
day). The services keep their own `--hours` options; the deployment sets
them from that one place. The Schedule widget shows one line per bundle,
opening to its members.

### Work classes

Every service and automation has one class:

| Class | Examples | Rule |
|---|---|---|
| capture | relays, recorders, the accounts session | Never yields. Protected. |
| interactive | the console, store reads, pricing a job | Runs; kept light. |
| background | downloads, filing, copies, hash checks, backtests, builds | Runs while no capture work is live, and throttled even then. |

Background work follows three rules:

- **It yields to what is live, not to the clock.** It checks whether any
  capture service is running (its health is anything but idle). A capture
  service that doesn't answer isn't running, so isn't capturing: on a
  holiday, or with the relays stopped, background work runs. It checks every
  15 seconds while yielding and before each download file, so a file in
  progress finishes; work it pauses must be resumable, as downloads are.
- **The schedule predicts.** It doesn't start a long task shortly before a
  capture bundle's hours begin.
- **Pressure slows it.** The relays' count of skipped upstream records
  rising, or memory pressure (swap nearly full), slows or pauses background
  work even outside any bundle's hours.

When background work can't tell whether capture is live (a capture service
answers, but not readably), it assumes it is.

### Limits by resource

- **CPU and disk I/O:** native background processes run under macOS's
  background policy (`taskpolicy -b`), which throttles CPU, disk and network
  together. Containers get a low CPU share.
- **Network:** the jobs service caps its download rate while interactive work
  is active.
- **Disk space:** the store's thresholds are already per class (non-capture
  writers pause at the critical level, capture stops at the floor). Budgets per
  class extend them later.
- **Work outside tinytrader:** `tt may-run background` answers with its exit
  status, so builds, backtests and other sessions' jobs can ask the same
  question before starting.

## Plan

1. Bundles in the inventory, shown in the Schedule widget.
2. The jobs service becomes background: it yields to live capture instead of
   pausing on the clock.
3. The pressure signals, and `tt may-run`.
4. Space and bandwidth budgets.

Steps 1 and 2 follow the drive work of 0007 (volume approval and per-file
placement), which comes first because a new drive is arriving.

## Consequences

- Hours are set once per bundle.
- Downloads use idle days and stop competing with live data.
- Builds and research jobs gain a rule they can check instead of one people
  remember.
- Each background service needs a way to learn what is live; until there are
  two hosts it reads the capture services' health directly, as the console
  does. Its list of them repeats the bundle's members, beside the bundle in
  the same deployment.

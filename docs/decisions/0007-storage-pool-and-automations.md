# 0007: Drives as one pool, copies by kind of data, and automations

- **Status:** Accepted
- **Date:** 2026-10-06
- **Amends:** [0002](0002-data-store.md) (where files go) and
  [0005](0005-store-service-and-supervision.md) (what the store runs)

## Context

A deployment starts with whatever disks its owner has: often one spare
external drive, later a second, eventually a NAS. Today the store manages a
list of drives but treats each separately: a provider batch must fit on one
drive, free space is reported per drive, a drive is added by editing the
deployment's configuration, and every file has exactly one copy. On
2026-10-05 a 435 GB batch couldn't be placed although the two drives together
had room, and both drives dropped off USB during heavy reads, which one copy
of irreplaceable data can't survive.

Separately, scheduled work is scattered. The relays and the accounts service
each parse their own active hours, Databento downloads were run by ad hoc
scripts in `/tmp`, and nothing re-checks stored files against their hashes.
None of it is visible or controllable in one place.

RAID is not the answer for the first case. Joining drives without redundancy
loses everything when any one drive fails, Apple RAID needs drives erased
first, and ZFS on macOS needs a kernel extension that breaks on updates. A
NAS does pooling and redundancy properly, but a new deployment shouldn't need
one to start.

## Decision

### The store's drives are one pool

- **Placement per file.** Each closed day file is placed whole on whichever
  drive has the most room below the critical threshold, file by file. One
  batch may span drives. Its receipts go to `provenance/` on every drive that
  holds part of it, and its migration record lists where each file went.
- **One capacity figure.** The catalog and console report the pool's total and
  free space first, with each drive beneath it. A file still lives whole on
  one drive, so losing a drive loses only its files, and the catalog says
  exactly which.
- **Adding a drive** is plugging it in and approving it once in the console.
  Approval writes a small marker, `.tinytrader-volume` (the volume's ID and
  name), at the drive's root. The store then uses every mounted volume with a
  marker. It never writes to an unmarked volume, and never to the system
  disk. Configuration may still name volumes explicitly (servers, NAS
  mounts), and may mark one read-only.
- **A missing drive** is reported, not worked around. Files on it are shown
  as unavailable, and nothing is re-placed until it returns or the operator
  retires it.

### Copies by kind of data

Each dataset has a protection class, recorded in its `dataset.json` with a
default per provider:

| Class | Examples | Copies |
|---|---|---|
| refetchable | Alpaca history, any free provider history | 1 |
| purchased | Databento batches | 2 when a second drive exists |
| recorded | the deployment's own live captures | 2 when a second drive exists |

A second copy always goes on a different drive. It is made by copying the
sealed file, verifying its SHA-256 at the destination, and then recording it
in the same manifest. Copies are part of the store's placement, not a backup
tool. Until a deployment has a second drive, the catalog shows each
two-copy file as "1 of 2 copies".

A NAS joins the pool as another volume, a mounted share or a remote store.
The store copies onto it until the operator moves the store itself there
(decision 0005).

### Automations

An **automation** is named, scheduled work with a recorded history. One
model covers all of it:

- **What:** a task the deployment knows how to run, never arbitrary commands:
  a provider top-off (such as Databento definitions every trading day), filing
  a finished download, making second copies, re-checking stored files against
  their hashes, or a service's active hours.
- **When:** active hours (as relays have now), a time of day on trading days,
  an interval, or after another automation finishes. Times are in New York
  time unless stated, and trading days come from the market calendar.
- **Limits:** every automation that can cost money states its ceiling. One
  that would exceed it, or whose cost is unknown, fails closed and asks
  (decision 0002's rule for purchases). One that writes names what it writes.
- **History:** each run records its start, end, outcome and what it changed,
  kept by the service that runs it and served at `/automations`.

The services that already do scheduled work run their automations themselves
and describe them (`/describe` gains an `automations` list). The console
gathers them into one Automations widget (workspaces replaced pages,
[0010](0010-console-widgets.md)), grown from the Schedule widget, with each
automation's next run, last run and outcome, and run-now, pause and resume
controls.
Creating an automation picks a task and a schedule. The store runs
storage tasks (filing, copies, re-checking), and the jobs service runs
provider tasks. There is no separate scheduler service until two hosts need
one coordinated.

Relays and the accounts service keep their `--hours` options, which become
their active-hours automation and appear on the page with the rest.

## Consequences

- The store needs a placement change (per file instead of per batch), volume
  markers, a pool view in the catalog, a copier with verification, and a
  periodic re-check of hashes. Each is small, and each ships separately.
- `dataset.json` gains a protection class.
- The console gains an Automations widget and a pool view on Storage. Its
  controls for automations are the second place the console writes,
  after Jobs, so it needs a login before the console is reachable from
  other machines.
- A deployment with one drive works from the start. Adding a drive gives it
  capacity and second copies without migration, and adding a NAS is the same
  step again.

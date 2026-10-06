# 0005: The store as a service, and what supervises what

- **Status:** Accepted
- **Date:** 2026-10-05
- **Amends:** [0002](0002-data-store.md) (how the store is reached) and
  [0004](0004-nix-not-nixos.md) (supervision on macOS)

## Context

A deployment's data may live on drives plugged into a Mac, on a NAS, or on a
server's own disks, and it moves between these as the deployment grows.
Services that write or read data by filesystem path have to change every time
the data moves, and each one has to be trusted with durability on its own.

On macOS, Docker runs containers in a Linux virtual machine and reaches the
Mac's drives through a shared-folder layer. That layer cannot report free
space truthfully, and it gives no assurance that a flushed write has reached
the drive. On Linux, containers use the host's own kernel and filesystems, so
neither problem exists there.

## Decision

### The store is a service

One **store service** runs beside each set of disks and is the only process
that touches them. It owns writing (segments, sealing, manifests, hashes),
reading and replay, the catalog, and tiering, as decision 0002 describes.

Every other service reaches data through it over the network: producers send
records in, consumers ask for ranges or live tails back. No service except the
store takes a data path. Where the data lives is then a single setting, the
store's address: drives on a Mac today, a NAS later, with nothing else
changing.

The read-only catalog service of decision 0002 becomes part of the store
service.

### What supervises what

| Machine | Store service | Everything else |
|---|---|---|
| macOS | Native binary under **launchd** | Docker Compose, restarted by Compose |
| Linux | systemd unit or a container | Docker Compose or systemd units |

On macOS, launchd's role is kept as small as possible, because every native
job is a portability cost: it runs the store service (a bare-metal interface
to the drives) and starts Docker at login. Everything else runs in containers,
so a Mac and a Linux NAS run the same images and the same Compose files.

A deployment without a separate NAS keeps this arrangement permanently; one
with a NAS runs the store service there and the Mac runs only containers.

### Services schedule themselves

A service that should only run at certain times (a market data relay outside
trading hours, for example) declares its active hours, and reports
"off schedule" rather than a fault outside them. Schedules come from data the
deployment already has, such as the market calendar, rather than per-machine
scheduler jobs.

## Consequences

- tinytrader generates launchd jobs only for the store service and Docker's
  start, alongside the systemd units it already generates.
- Market data relays, recorders and readers are containers that stream into
  and out of the store service. Rewriting the relays in tinytrader is the first
  step.
- Until the store service exists, services that need durable local files on a
  Mac run natively and say so; this is a temporary exception, not a pattern.

## Status, 2026-10-06

The store service exists (`services/store`) and replaces the catalog service.
Its first version ingests provider batches (Databento) into the standard
layout of decision 0002, with provenance and a migration ledger, and serves
the catalog. Reading data back over the network and live recording come
later.

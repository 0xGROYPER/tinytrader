# 0004: Nix, not NixOS

- **Status:** Accepted
- **Date:** 2026-10-05


> **Built so far (2026-10-07):** the flake builds and checks every crate
> (`nix flake check`), and the NixOS module and generated systemd units
> exist. Since 2026-10-07 container images are built by Nix too
> (`nix/images.nix`); on a Mac, or with Docker alone, `nix/build-images.sh`
> builds them inside Docker's Linux VM, so there is one build. The
> nix-darwin module (`darwinModules.services`) runs the same service
> definitions as launchd agents on macOS.

## Context

tinytrader's first host configurations target NixOS, and service discovery
began with Docker labels. Left unchecked, either could become a hidden
requirement: a deployment would need NixOS to be configured, or Docker to be
seen.

## Decision

**tinytrader's contracts depend on nothing.** `tinytrader.observe/1`, the topology
format and the store layout are plain HTTP, JSON and files. Any process on
any operating system that speaks them is part of a deployment.

**Building depends on Nix, the package manager,** which runs on every Linux
distribution and on macOS, with Docker images for anyone who won't install
it. Contributors use `nix develop`.

**Running a host has three supported forms**, from least to most
commitment:

| Form | Runs services with | Discovered by |
|---|---|---|
| Docker Compose, any OS | Compose restart policies | `tinytrader.observe` labels |
| Nix on any Linux | systemd units generated from `tinytrader.services` (`systemdUnits`) | `observe` addresses in the topology file |
| NixOS | the `tinytrader.services` NixOS module | `observe` addresses (generated) |

Services can also be run by hand or by any other supervisor and listed by
address.

## Portability

Services are portable by default and build natively on Linux and macOS.
Platform-specific capability is the exception, kept separate:

1. **Isolate it behind a small interface** in a library, with the
   platform-specific implementation under `#[cfg(target_os = "linux")]`.
2. **Provide an honest fallback where one exists** (for example, receive
   timestamps taken in user space where kernel timestamps are unavailable),
   and **report which implementation is in use** in `/describe` and metrics,
   so data quality is visible rather than assumed.
3. **Where no honest fallback exists, the whole service is Linux-only:**
   Linux-only dependencies go under `[target.'cfg(target_os = "linux")']`,
   other targets fail to compile with a clear message, and its Nix package
   sets `meta.platforms = lib.platforms.linux`. On macOS it runs in Docker.

## Consequences

- A Mac can run tinytrader natively, in Docker, or both; the choice is the
  operator's, not forced by the code.
- NixOS remains the most complete way to run a host, without being the only
  one.
- Discovery by address means a deployment needs neither Docker nor NixOS to
  appear in the console or the terminal view.

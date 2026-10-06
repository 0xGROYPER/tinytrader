# 0001: tinytrader is a platform; deployments live elsewhere

- **Status:** Accepted
- **Date:** 2026-10-05

## Context

tinytrader began as the configuration for one operator's setup. Built that way,
every host name, product and display preference becomes part of the code,
and sharing it later means a rewrite. The same tension exists between nixpkgs
and a NixOS user's own configuration, and it is solved the same way.

## Decision

Split the work into two kinds of repository:

| tinytrader (this repository) | A deployment (each operator's own) |
|---|---|
| Libraries, services, the console | Inventory: hosts, roles, data roots |
| NixOS and nix-darwin modules | Topology, panels and display choices |
| Topology and observation formats | Secrets |
| Generic display capabilities | Strategies and research |
| Examples and documentation | Decisions about that operation |

A deployment takes tinytrader as a flake input and imports its modules and
packages. tinytrader never references a deployment.

## Consequences

- Anyone can clone tinytrader and run the quickstart, then start their own
  deployment repository from an example.
- Display and behaviour choices must be expressible as data. When they are
  not, the topology or module options grow a documented field instead of the
  console growing a special case.
- Personal data in tinytrader is a defect. `CONTRIBUTING.md` states the rule
  and `checks.<system>.generic` catches the commonest leaks.
- A change that needs both repositories lands in tinytrader first, then the
  deployment updates its flake input.

# 0001: Glacier is a platform; deployments live elsewhere

- **Status:** Accepted
- **Date:** 2026-10-05

## Context

Glacier began as the configuration for one operator's setup. Built that way,
every host name, product and display preference becomes part of the code,
and sharing it later means a rewrite. The same tension exists between nixpkgs
and a NixOS user's own configuration, and it is solved the same way.

## Decision

Split the work into two kinds of repository:

| Glacier (this repository) | A deployment (each operator's own) |
|---|---|
| Libraries, services, the console | Inventory: hosts, roles, data roots |
| NixOS and nix-darwin modules | Topology, panels and display choices |
| Topology and observation formats | Secrets |
| Generic display capabilities | Strategies and research |
| Examples and documentation | Decisions about that operation |

A deployment takes Glacier as a flake input and imports its modules and
packages. Glacier never references a deployment.

## Consequences

- Anyone can clone Glacier and run the quickstart, then start their own
  deployment repository from an example.
- Display and behaviour choices must be expressible as data. When they are
  not, the topology or module options grow a documented field instead of the
  console growing a special case.
- Personal data in Glacier is a defect. `CONTRIBUTING.md` states the rule
  and `checks.<system>.generic` catches the commonest leaks.
- A change that needs both repositories lands in Glacier first, then the
  deployment updates its flake input.

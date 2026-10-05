# Contributing to Glacier

Glacier is a **platform**. Anyone should be able to clone it and run their
own deployment without first removing someone else's choices. Please keep it
that way.

## Where your work belongs

| Belongs in Glacier | Belongs in your own deployment repository |
|---|---|
| Libraries, services, the console | Your inventory: hosts, roles, data roots |
| NixOS and nix-darwin modules | Your topology, panels and display choices |
| Formats (topology, `glacier.observe/1`) and their documentation | Secrets, credentials, account identifiers |
| Generic display capabilities and formats | Strategies, research, models |
| Examples that run on their own | Decisions about your operation |

Your deployment repository takes Glacier as a flake input and imports its
modules and packages. Glacier never references a deployment. See
[decision 0001](docs/decisions/0001-platform-and-deployment.md).

If you're unsure, ask: *would this make sense in someone else's
deployment?* If not, it goes in yours.

## Never commit to Glacier

- Real hostnames, tailnet names, IP addresses, or filesystem paths from a
  particular machine.
- Usernames, email addresses, account or API identifiers, secrets.
- Venues, products, strategies or schedules chosen by an operator, outside
  `examples/`.
- Links to anyone's private repositories or infrastructure.

Example values are fine inside `examples/`, because they are clearly samples
(the quickstart's public Coinbase products, for instance).

## Mechanisms here, choices there

Glacier provides capabilities; deployments supply data. If you're about to
write code that checks for a particular service, metric or product name,
especially in the console, add an optional, documented field to the topology
format or a module option instead.

## Contracts

- **`glacier.observe/1`** (`libs/observe`): the `/health` document and metric
  names. Change additively; never rename or remove. Labels stay low
  cardinality (service, venue, product), never per-order ids.
- **Topology format** (read by the console): every deployment depends on it.
  Add optional fields; don't break existing files.

## Guardrails

- `nix flake check` runs `checks.<system>.generic`, which fails on the most
  common leaks: absolute home paths, tailnet domains and private addresses.
  It is a backstop, not a substitute for the rules above.
- It also runs the Rust tests and clippy, and evaluates the example NixOS host.

## Before you send a change

```sh
cargo fmt --all
cargo clippy --all-targets -- --deny warnings
nix flake check
```

Services must meet the service contract: configuration from environment or
flags, logs to stdout/stderr, `/health` and `/metrics` via `libs/observe`, and
clean shutdown on SIGTERM. Record decisions that shape the platform in
`docs/decisions/`.

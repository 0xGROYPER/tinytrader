# Glacier Labs

A platform for running small trading and research operations: services,
deployment modules and an operator console. Services run under NixOS, under
systemd on any Linux with Nix, or as Docker containers on any operating
system.

Glacier is the platform, not anyone's deployment. Your hosts, data, secrets,
strategies and research live in your own repository, which uses Glacier as a
flake input. See [decision 0001](docs/decisions/0001-platform-and-deployment.md).

> **Status:** early. The deployment and observation layer comes first,
> exercised by a toy service.

## Try it

[`examples/quickstart`](examples/quickstart/) runs a toy collector streaming
live Coinbase prices and the Glacier console, with only Docker:

```sh
cd examples/quickstart && docker compose up --build -d
# then open http://localhost:8080
```

Have a Databento or Alpaca account? [`examples/databento`](examples/databento/)
and [`examples/alpaca`](examples/alpaca/) stream live market data into the
console the same way.

## What's here

| Path | Holds |
|---|---|
| `libs/observe` | What every service serves: `/health`, `/metrics`, `/describe`, `/events`, `/recent` (`glacier.observe/1`) |
| `libs/fleet` | Discovers and polls a deployment; the data layer behind every front end |
| `services/Dockerfile` | Container image build for any service |
| `apps/tui` | Terminal view of a deployment (Ratatui) |
| `services/console` | The operator console: deployment graph, services, market charts and news, data, storage and docs, in one sidebar |
| `services/ticker` | Toy service used by the quickstart |
| `services/databento-live` | Example service streaming a Databento live subscription |
| `services/alpaca-live` | Example service streaming Alpaca's free real-time IEX data |
| `services/dbn-tcp` | Example service reading any live DBN stream served over TCP |
| `services/catalog` | Read-only catalog of data roots: datasets, coverage, disk space |
| `services/alpaca-news` | Example service serving market news from Alpaca (supplied by Benzinga) |
| `services/alpaca-account` | Example service serving an Alpaca account's balances and positions, read only |
| `services/relay` | Market data relay: holds one provider connection and serves it to local clients ([decision 0006](docs/decisions/0006-market-data-relay.md)) |
| `infra/modules` | NixOS modules, such as `glacier.services.<name>` |
| `nix/` | Flake modules: Rust builds and checks |
| `examples/` | Complete deployments you can run or copy |
| `docs/` | Platform decisions and contracts |

## Running Glacier

Glacier's contracts are plain HTTP, JSON and files, so services run on any
operating system ([decision 0004](docs/decisions/0004-nix-not-nixos.md)):

| On | Run services with | They're found by |
|---|---|---|
| Any OS with Docker | Docker Compose (see `examples/`) | `glacier.observe` container labels |
| Any Linux with Nix | systemd unit files generated from `glacier.services` | `observe` addresses in the topology file |
| NixOS | the `glacier.services` NixOS module | the topology file the module generates |

With `glacier.services`, giving a service `observe.port` is enough: it
listens there, and the module writes `/etc/glacier/topology.json` listing it,
on NixOS directly and in the `systemdUnits` output for other distributions.
A service started any other way joins a deployment by being listed by address:

```json
{ "services": [{ "id": "ticker", "observe": "http://127.0.0.1:9105" }] }
```

## Terminal view

```sh
cargo run -p glacier-tui -- --console http://localhost:8080
```

Tab switches between **Services** (status, details, live events) and
**Hosts**, an htop-style view of one machine: CPU, memory, swap, load and
disks from its node_exporter, with each Glacier service's CPU and memory
against its limits.

`--console` reads a running console from anywhere it is reachable, over SSH
included. `--topology FILE` polls the services the file lists by address
directly, with no console needed.

## Using Glacier in your own deployment

```nix
{
  inputs.glacier.url = "github:<owner>/glacier";
  outputs = { glacier, ... }: {
    nixosConfigurations.myhost = glacier.inputs.nixpkgs.lib.nixosSystem {
      system = "x86_64-linux";
      modules = [
        glacier.nixosModules.services
        {
          glacier.services.ticker = {
            enable = true;
            package = glacier.packages.x86_64-linux.ticker;
          };
        }
      ];
    };
  };
}
```

## Working here

```sh
nix develop          # shell with the repo's tools
nix flake check      # tests, lints, module evaluation, genericity check
cargo test           # Rust workspace only
```

Read [CONTRIBUTING.md](CONTRIBUTING.md) before contributing.

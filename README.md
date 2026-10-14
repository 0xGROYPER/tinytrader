# tinytrader

A platform for running trading and research operations: services,
deployment modules and an operator console. Services run under NixOS, under
systemd on any Linux with Nix, or as Docker containers on any operating
system.

tinytrader is the platform, not anyone's deployment. Your hosts, data, secrets,
strategies and research live in your own repository, which uses tinytrader as a
flake input. See [decision 0001](docs/decisions/0001-platform-and-deployment.md).

> **Status:** early, in daily use by one deployment: live market data relays,
> a data store, provider batch jobs, accounts, news, an economic calendar, a
> console and a terminal view. Trading (decision 0009) is next.

## Try it

[`examples/quickstart`](examples/quickstart/) runs a toy collector streaming
live Coinbase prices and the tinytrader console, with only Docker:

```sh
cd examples/quickstart
../../nix/build-images.sh ticker console   # builds the images with Nix, inside Docker
docker compose up -d
# then open http://localhost:8080
```

Have a Databento or Alpaca account? [`examples/databento`](examples/databento/)
and [`examples/alpaca`](examples/alpaca/) stream live market data into the
console the same way.

## What's here

| Path | Holds |
|---|---|
| `libs/observe` | What every service serves: `/health`, `/metrics`, `/describe`, `/events`, `/recent` (`tinytrader.observe/1`), plus shared helpers (credentials, the write guard) |
| `libs/fleet` | Discovers and polls a deployment; the data layer behind every front end |
| `libs/schedule` | Active hours on the market calendar, in New York time |
| `libs/dbn` | tinytrader's DBN extension records (trade conditions) |
| `libs/tinytrader` | The crates.io umbrella crate |
| `nix/images.nix`, `nix/build-images.sh` | Container images of the services, built by Nix (inside Docker when Nix isn't installed) |
| `apps/tui` | Terminal view of a deployment (Ratatui): overview, services, market chart, a live options book, data and hosts |
| `services/console` | The operator console: workspaces of widgets (Dockview), docs and the roadmap |
| `services/ticker` | Toy service used by the quickstart |
| `services/relay` | Market data relay: holds one provider connection (Databento, or Alpaca IEX) and serves it to local clients, with minute bars ([decision 0006](docs/decisions/0006-market-data-relay.md)) |
| `services/store` | The data store: the one process that touches the disks; files provider downloads across approved drives, and catalogs datasets, coverage and disk space |
| `services/databento-jobs` | Databento batch jobs: prices and submits requests (charges need an exact confirmation, every submission is written down first), and downloads finished jobs into the store |
| `services/alpaca-account` | Alpaca accounts' balances and positions, read only, and the market calendar |
| `services/alpaca-data` | Market data for any symbol on request: stock snapshots, bars and option chains with IV and greeks (Alpaca) |
| `services/alpaca-news` | Market news from Alpaca (supplied by Benzinga) |
| `services/econ-calendar` | Scheduled US economic releases and FOMC decisions, from the agencies' own schedules |
| `infra/modules` | NixOS modules, such as `tinytrader.services.<name>` |
| `nix/` | Flake modules: Rust builds and checks |
| `examples/` | Complete deployments you can run or copy |
| `docs/` | Platform decisions and contracts |

## Running tinytrader

tinytrader's contracts are plain HTTP, JSON and files, so services run on any
operating system ([decision 0004](docs/decisions/0004-nix-not-nixos.md)):

| On | Run services with | They're found by |
|---|---|---|
| Any OS with Docker | Docker Compose (see `examples/`) | `tinytrader.observe` container labels |
| Any Linux with Nix | systemd unit files generated from `tinytrader.services` | `observe` addresses in the topology file |
| NixOS | the `tinytrader.services` NixOS module | the topology file the module generates |

With `tinytrader.services`, giving a service `observe.port` is enough: it
listens there, and the module writes `/etc/tinytrader/topology.json` listing it,
on NixOS directly and in the `systemdUnits` output for other distributions.
A service started any other way joins a deployment by being listed by address:

```json
{ "services": [{ "id": "ticker", "observe": "http://127.0.0.1:9105" }] }
```

## Terminal view

```sh
cargo run -p tinytrader-tui -- --console http://localhost:8080
```

Tab switches between **Services** (status, details, live events) and
**Hosts**, an htop-style view of one machine: CPU, memory, swap, load and
disks from its node_exporter, with each tinytrader service's CPU and memory
against its limits.

`--console` reads a running console from anywhere it is reachable, over SSH
included. `--topology FILE` polls the services the file lists by address
directly, with no console needed.

## Using tinytrader in your own deployment

```nix
{
  inputs.tinytrader.url = "github:<owner>/tinytrader";
  outputs = { tinytrader, ... }: {
    nixosConfigurations.myhost = tinytrader.inputs.nixpkgs.lib.nixosSystem {
      system = "x86_64-linux";
      modules = [
        tinytrader.nixosModules.services
        {
          tinytrader.services.ticker = {
            enable = true;
            package = tinytrader.packages.x86_64-linux.ticker;
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

# Glacier Labs

A platform for running small trading and research operations: services,
deployment modules and an operator console. NixOS hosts run services under
systemd from Nix; any other machine runs the same services as Docker
containers.

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
| `services/console` | The operator console: deployment graph, live service state, panels |
| `services/ticker` | Toy service used by the quickstart |
| `services/databento-live` | Example service streaming a Databento live subscription |
| `services/alpaca-live` | Example service streaming Alpaca's free real-time IEX data |
| `services/dbn-tcp` | Example service reading any live DBN stream served over TCP |
| `infra/modules` | NixOS modules, such as `glacier.services.<name>` |
| `nix/` | Flake modules: Rust builds and checks |
| `examples/` | Complete deployments you can run or copy |
| `docs/` | Platform decisions and contracts |

## Terminal view

```sh
cargo run -p glacier-tui -- --console http://localhost:8080
```

`--console` reads a running console from anywhere it is reachable, over SSH
included. Inside a deployment's network, `--docker URL --project NAME` polls
the services directly instead.

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

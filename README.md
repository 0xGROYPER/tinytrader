# Glacier Labs

A platform for running a small trading and research operation: services,
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

## What's here

| Path | Holds |
|---|---|
| `libs/observe` | Shared `/health` and `/metrics` endpoints (`glacier.observe/1`) |
| `services/Dockerfile` | Container image build for any service |
| `services/console` | The operator console: deployment graph, live service state, panels |
| `services/ticker` | Toy service used by the quickstart |
| `infra/modules` | NixOS modules, such as `glacier.services.<name>` |
| `nix/` | Flake modules: Rust builds and checks |
| `examples/` | Complete deployments you can run or copy |
| `docs/` | Platform decisions and contracts |

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

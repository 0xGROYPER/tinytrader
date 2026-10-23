# TODO 
- single machine deployment example with docker compose, multi machine with docker swarm
- tinytrader-ops build with nix, package to OCI 

# tinytrader

A platform for running trading and research operations: services and an
operator console. The services are an ordinary Rust (Cargo) workspace and
plain HTTP, JSON and files; they build with Cargo and run anywhere. Nix is
one supported way to package them, for reproducible builds, container images
and NixOS and nix-darwin service modules, not a requirement (see
[Building and testing](#building-and-testing)).

tinytrader is the platform, not anyone's deployment. Your hosts, data, secrets,
strategies and research live in your own repository, which uses tinytrader as a
flake input. See [decision 0001](docs/decisions/0001-platform-and-deployment.md).

> **Status:** early, in daily use by one deployment: live market data relays,
> a data store, provider batch jobs and history downloads, accounts, news, an
> economic calendar, an order gateway, a job queue, a host monitor, a console
> and a terminal view. The trading runtime ([decision 0009](docs/decisions/0009-trading-runtime.md))
> is proposed.

The documentation starts at [docs/](docs/README.md): tutorials, the
console's widgets, the relay and store APIs, recovery, and the decisions.

## Quick start with Docker

[`examples/quickstart`](examples/quickstart/) runs a toy collector streaming
live Coinbase prices and the tinytrader console. It needs only Docker with
Compose: the images are published prebuilt, so there is nothing to install or
build.

```sh
cd examples/quickstart && docker compose up
```

Then open <http://localhost:8080>. `Ctrl-C` stops it; `docker compose down -v`
removes it and its recorded data.

Have a Databento or Alpaca account? [`examples/databento`](examples/databento/)
and [`examples/alpaca`](examples/alpaca/) stream live market data into the
console the same way. To build the images from source instead, with Docker,
Cargo or Nix, see [Building and testing](#building-and-testing).

## What's here

| Path | Holds |
|---|---|
| `libs/observe` | What every service serves: `/health`, `/metrics`, `/describe`, `/events`, `/recent` (`tinytrader.observe/1`), plus shared helpers (credentials, the write guard) |
| `libs/fleet` | Discovers and polls a deployment; the data layer behind every front end |
| `libs/schedule` | Active hours on the market calendar, in New York time |
| `libs/dbn` | tinytrader's DBN extension records (trade conditions) |
| `libs/tinytrader` | The crates.io umbrella crate |
| `Dockerfile`, `docker-bake.hcl` | Container images of the services, built with Cargo; published to `ghcr.io` by CI |
| `nix/images.nix`, `nix/build-images.sh` | The same images built by Nix (inside Docker when Nix isn't installed) |
| `apps/tui` | Terminal view of a deployment (Ratatui): overview, services, market chart, a live options book, data and hosts |
| `services/console` | The operator console: workspaces of widgets (Dockview), docs and the roadmap |
| `services/ticker` | Toy service used by the quickstart |
| `services/relay` | Market data relay: holds one provider connection (Databento, or Alpaca IEX) and serves it to local clients, with minute bars ([decision 0006](docs/decisions/0006-market-data-relay.md)) |
| `services/store` | The data store: the one process that touches the disks; files provider downloads across approved drives, and catalogs datasets, coverage and disk space |
| `services/databento-jobs` | Databento batch jobs: prices and submits requests (charges need an exact confirmation, every submission is written down first), and downloads finished jobs into the store |
| `services/alpaca-account` | Alpaca accounts' balances and positions, read only, and the market calendar |
| `services/alpaca-data` | Market data for any symbol on request: stock snapshots, bars and option chains with IV and greeks (Alpaca) |
| `services/alpaca-news` | Market news from Alpaca (supplied by Benzinga) |
| `services/alpaca-history` | Alpaca's historical trades and quotes, a day at a time, into the store ([decision 0014](docs/decisions/0014-history-feeds.md)) |
| `services/econ-calendar` | Scheduled US economic releases and FOMC decisions, from the agencies' own schedules |
| `services/order-gateway` | The order gateway (`tinytrader.gateway/1`): the only holder of a broker account's keys; validation, caps, a kill switch and a hash-chained journal ([its README](services/order-gateway/README.md)) |
| `services/queue` | The job queue: work that runs later or when the machine can spare it, from templates the deployment defines ([decision 0012](docs/decisions/0012-job-queue.md)) |
| `nix/watch.nix` | `watch`: a host's own monitor (Docker, containers, health, native services, disks), pushed to an ntfy-style URL, with a heartbeat ping after each run ([decision 0015](docs/decisions/0015-host-watch.md)) |
| `infra/modules` | `tinytrader.services.<name>`: NixOS modules (systemd units) and the same definitions as nix-darwin launchd agents or boot-time daemons |
| `nix/` | Flake modules: Rust builds, images, checks |
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
| macOS with nix-darwin | the same `tinytrader.services` definitions as launchd agents or boot-time daemons (`darwinModules.services`) | `observe` addresses in the topology file |

With `tinytrader.services`, giving a service `observe.port` is enough: it
listens there, and the module writes `/etc/tinytrader/topology.json` listing it,
on NixOS directly and in the `systemdUnits` output for other distributions.
On macOS a service runs as the host's primary user, as a launchd agent by
default: it starts when that user logs in. Give it `launchd = "daemon"` to run
it as a launchd daemon instead (still as that user, with `HOME` and `USER`
set), which starts at boot with no one logged in, so it comes back after a
reboot or power cut. With FileVault on, boot waits for the disk to be
unlocked at the login window either way. Switching a service to a daemon,
activation stops the agent it was. `watch` finds a service either way.

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

## Building and testing

### With Cargo (no Nix)

The services build and test with Rust alone. `rustup` installs the pinned
toolchain from `rust-toolchain.toml` on first use:

```sh
cargo build --release -p console     # one service; binaries in target/release/
cargo build --release                # every service
cargo test --workspace
cargo fmt --all && cargo clippy --workspace --all-targets -- --deny warnings
```

A service runs from its binary, taking its settings from the environment
(documented at the top of its `main.rs`), and the console serves its page
from `services/console/web/`. CI's `cargo` job builds and tests this way, with
no Nix installed, so the Cargo path stays first-class.

### With Docker

The `Dockerfile` builds any service's image with Cargo in the official Rust
image, onto a distroless base, as user 65532. `docker-bake.hcl` builds every
service's image in one go, compiling the workspace once:

```sh
docker build --build-arg SERVICE=console -t tinytrader/console:local .
docker buildx bake --load                   # every service, as tinytrader/<service>:local
docker buildx bake --load ticker console    # some
```

Each example's `compose.build.yaml` builds its images this way
(`docker compose -f compose.yaml -f compose.build.yaml up --build`).

#### Publishing images

The `images` workflow builds every service's image on each pull request (for
linux/arm64 only, without pushing), and on each push to `main` and each
`vX.Y.Z` tag builds them for linux/amd64 and linux/arm64 and publishes them as
`ghcr.io/<owner>/tinytrader-<service>`, tagged `main` and `latest` (the same
image), `sha-<short commit>`, and `X.Y.Z` and `X.Y` for releases. It pushes
with the workflow's own `GITHUB_TOKEN`, so no secrets are needed.

GitHub makes a package private when a workflow first pushes it. Once per
package, after its first publish: open the package (the repository's
**Packages** list, or your profile's **Packages** tab), then **Package
settings**, **Change visibility**, **Public**. Until then, `docker compose
up` fails to pull it with `denied` or `unauthorized`.

### With Nix (optional)

Nix adds reproducible builds (every dependency pinned in `flake.lock`),
container images from the same pinned packages, and the `tinytrader.services` modules for
NixOS, other Linux distributions and nix-darwin:

```sh
nix develop                          # shell with the repo's tools
nix build .#store                    # a service as a Nix package
nix/build-images.sh console relay    # container images, loaded into Docker as tinytrader/<name>:nix
nix flake check                      # everything below
```

`nix/build-images.sh` runs Nix inside a container when Nix isn't installed,
so building the images needs only Docker.

`nix flake check` runs:

| Check | What |
|---|---|
| `rust-tests`, `rust-clippy` | The workspace's tests and lints |
| `console-types` | Type-checks the console's page |
| `watch` | The host monitor's tests |
| `generic` | Fails on deployment-specific details: home paths, tailnet names, private addresses, key material |

and evaluates the NixOS and nix-darwin modules and the example host.

Read [CONTRIBUTING.md](CONTRIBUTING.md) before contributing: tinytrader is a
platform, and your hosts, data and secrets belong in your own repository.

# tinytrader

A platform for running trading and research operations, as Rust services:
market data relays, a data store, provider batch jobs and history downloads,
accounts, news, an economic calendar, an order gateway, a job queue, an
operator console and a terminal view. The services are one Cargo workspace.
They speak plain HTTP, JSON, TCP and files, take their settings from the
environment, and run anywhere Rust does.

tinytrader is the platform, not anyone's deployment. Your hosts, data,
secrets, strategies and research live in your own repository
([decision 0001](docs/decisions/0001-platform-and-deployment.md)). Running the
whole stack on one machine or a network is
[tinytrader-deploy](https://github.com/dawson-shay/tinytrader-deploy)'s job
(see [Deploying it](#deploying-it)).

> **Status:** early, in daily use by one deployment. The trading runtime
> ([decision 0009](docs/decisions/0009-trading-runtime.md)) is proposed.

The documentation starts at [docs/](docs/README.md): tutorials, the
console's widgets, every interface in [docs/api/](docs/api/README.md),
recovery, and the decisions.

## Quick start

You need Rust: `rustup` installs the toolchain pinned in
`rust-toolchain.toml` on first use. Nothing below needs an account or a key.
Every service binds to loopback by default.

**1. A live feed.** `ticker` is a toy collector: it subscribes to Coinbase's
public ticker feed and appends every message to a JSON Lines file per product
and day.

```sh
cargo run -p ticker              # BTC-USD and ETH-USD into ./data, endpoints on 127.0.0.1:9105
```

**2. Look at it from the command line.** Every service serves the same
[observe endpoints](docs/api/observe.md): what it is, whether it's healthy,
its metrics, recent events and series.

```sh
curl -s http://127.0.0.1:9105/health      # tinytrader.observe/1: state, detail, uptime
curl -s http://127.0.0.1:9105/describe    # its role, sources and capabilities
curl -s http://127.0.0.1:9105/recent      # sampled prices per product
curl -s http://127.0.0.1:9105/metrics     # Prometheus text
ls data/BTC-USD/                          # what it wrote
```

**3. The terminal view and the console.** Both read a deployment from a
[topology file](docs/api/fleet.md#the-topology-file) that lists services by
address. In a second terminal:

```sh
echo '{ "services": [{ "id": "ticker", "observe": "http://127.0.0.1:9105" }] }' > topology.json
cargo run -p console             # serves http://127.0.0.1:8080 from ./topology.json
```

```sh
curl -s http://127.0.0.1:8080/api/state                         # every service's latest health and metrics
cargo run -p tinytrader-tui -- --console http://127.0.0.1:8080  # the terminal view
```

Then open <http://127.0.0.1:8080> for the console: the same state, as a
deployment graph and workspaces of widgets. The terminal view also takes
`--topology topology.json` to poll the services itself, with no console.
`Ctrl-C` stops each.

**4. The data store**, on an empty folder (`:anydir` lets it take a plain
folder rather than a mounted drive):

```sh
mkdir -p demo-volume
STORE_STATE=$PWD/store-state.json cargo run -p store -- --volume demo=$PWD/demo-volume:anydir
curl -s http://127.0.0.1:9120/catalog     # each volume: datasets, days held, free space
curl -s 'http://127.0.0.1:9120/coverage?provider=alpaca&dataset=ALPACA.IEX&symbol=SPY&schema=mbp-1&start=2026-10-01&end=2026-10-02'
```

`STORE_STATE` keeps its state file beside you instead of in the default
per-user location.

**5. The economic calendar**, from the agencies' own published schedules:

```sh
ECON_CACHE=econ-cache cargo run -p econ-calendar
curl -s http://127.0.0.1:9122/events      # scheduled releases and FOMC decisions
```

Have a Databento or Alpaca account? The relay, `databento-jobs`, the Alpaca
services and the order gateway take your keys from the environment; see
[Services](#services).

## The `tt` command line

`tt` is tinytrader from the command line. Today it drives the
[job queue](docs/api/queue.md):

```sh
cargo run -p queue --bin tt -- queue templates     # what the queue can run
cargo run -p queue --bin tt -- queue list          # jobs: waiting, running, done
cargo run -p queue --bin tt -- queue add TEMPLATE NAME=VALUE --when after-close
```

It finds the queue at `TT_QUEUE` (default `http://127.0.0.1:9123`). To try it,
run a queue with a template file, such as the example in
[docs/api/queue.md](docs/api/queue.md):

```sh
QUEUE_TEMPLATES=templates.yaml QUEUE_STATE=queue-state cargo run -p queue --bin queue
```

`cargo install --path services/queue --bin tt` puts `tt` on your `PATH`.

## Coming

Planned and not on `main` yet; none of these commands works today:

- **A live Coinbase stream through the relay**, the public feed with no
  account: the relay has no Coinbase source yet (it has Databento and Alpaca
  IEX).
- **`tt` for acquisition** ([decision 0021](docs/decisions/0021-acquisition-verbs-and-shared-plumbing.md)):
  Databento's `tt fetch`, `tt quotes`, `tt approve`, `tt decline` and `tt jobs`,
  then `tt fetch alpaca` (historical data from Alpaca's free IEX feed into the
  store), `tt listen` (a live stream) and `tt store coverage`.

Until then, the [store](docs/api/store.md) and
[databento-jobs](docs/api/databento-jobs.md) HTTP endpoints do the same
work with `curl`.

## Services

Each service runs with `cargo run -p <name>`, takes its settings from the
environment (documented at the top of its `main.rs` and in its reference), and
listens on its own `*_LISTEN` variable, else `TINYTRADER_LISTEN`, else its
default port on `127.0.0.1`. Every one serves `/health`, `/describe` and
`/metrics`; the endpoints below are the ones a person calls first.

| Service | Run | Key settings | Endpoints to try | Reference |
|---|---|---|---|---|
| ticker | `cargo run -p ticker` | `TICKER_PRODUCTS` (default `BTC-USD,ETH-USD`), `TICKER_DATA_DIR` (default `./data`) | `:9105/recent` | [observe](docs/api/observe.md) |
| console | `cargo run -p console` | `CONSOLE_TOPOLOGY` (default `topology.json`), `CONSOLE_DOCKER` (a read-only Docker API), `CONSOLE_STORE`, `CONSOLE_JOBS` | `:8080/api/topology`, `:8080/api/state` | [console](docs/api/console.md), [fleet](docs/api/fleet.md) |
| tinytrader-tui | `cargo run -p tinytrader-tui -- --console URL` | `--console URL` or `--topology FILE` | (a terminal view) | [fleet](docs/api/fleet.md) |
| store | `cargo run -p store -- --volume NAME=PATH` | `--volume` (repeatable), `STORE_STATE`, `STORE_WRITE_TOKEN` (or `_FILE`) | `:9120/catalog`, `:9120/coverage?...`, `:9120/files` | [store](docs/api/store.md) |
| econ-calendar | `cargo run -p econ-calendar` | `ECON_CACHE`; optional `ECON_CONTACT` (BLS), `SEC_CONTACT`, earnings keys | `:9122/events`, `:9122/earnings` | [econ-calendar](docs/api/econ-calendar.md) |
| queue | `cargo run -p queue --bin queue` | `QUEUE_TEMPLATES`, `QUEUE_STATE` (both required), `QUEUE_DOCKER` | `:9123/jobs`, or `tt queue` | [queue](docs/api/queue.md) |
| relay | `RELAY_SYMBOLS=SPY.OPT cargo run -p relay` | `DATABENTO_API_KEY`, or `RELAY_SOURCE=alpaca-iex` with `APCA_API_KEY_ID` and `APCA_API_SECRET_KEY`; `RELAY_DATASET`, `RELAY_SCHEMA`, `RELAY_HOURS` | `:9110/describe`, `:9110/quotes`, `:9110/bars?symbol=...`; the stream on `:7600` | [relay](docs/api/relay.md) |
| databento-jobs | `cargo run -p databento-jobs` | `DATABENTO_API_KEY`, `JOBS_STORE`, `JOBS_LEDGER`, `JOBS_APPROVE_TOKEN` | `:9121/jobs`, `POST :9121/price` | [databento-jobs](docs/api/databento-jobs.md) |
| alpaca-history | `cargo run -p alpaca-history` | `ALPACA_HISTORY_KEY_ID`, `ALPACA_HISTORY_SECRET_KEY`, `HISTORY_STORE`, `HISTORY_STATE` | `POST :9124/runs` | [alpaca-history](docs/api/alpaca-history.md) |
| alpaca-data | `cargo run -p alpaca-data` | `APCA_API_KEY_ID`, `APCA_API_SECRET_KEY`, `ALPACA_DATA_FEED` (default `iex`) | `:9125/snapshots?symbols=SPY`, `:9125/bars?symbol=SPY` | [alpaca-data](docs/api/alpaca-data.md) |
| alpaca-account | `cargo run -p alpaca-account` | `APCA_API_KEY_ID`, `APCA_API_SECRET_KEY`, or `ALPACA_ACCOUNTS` | `:9107/accounts`, `:9107/calendar` | [alpaca-account](docs/api/alpaca-account.md) |
| alpaca-news | `cargo run -p alpaca-news` | `APCA_API_KEY_ID`, `APCA_API_SECRET_KEY`, `ALPACA_NEWS_SYMBOLS` | `:9106/news` | [alpaca-news](docs/api/alpaca-news.md) |
| order-gateway | `cargo run -p order-gateway -- --account NAME --broker sim --listen 127.0.0.1:7601 --state DIR --client ...` | flags, not the environment: `--broker` (`sim`, `alpaca-paper`), `--credentials env`, caps | TCP NDJSON (`tinytrader.gateway/1`), no HTTP | [order-gateway](docs/api/order-gateway.md), [its README](services/order-gateway/README.md) |

Ports are `127.0.0.1`'s; every secret `NAME` can instead be given as
`NAME_FILE`, a file holding it. The shared conventions (errors, auth, units)
are in [docs/api/](docs/api/README.md).

## What's here

| Path | Holds |
|---|---|
| `services/` | The services above ([services/README.md](services/README.md)) |
| `apps/tui` | The terminal view (Ratatui): overview, services, market chart, a live options book, data and hosts |
| `libs/observe` | What every service serves: `/health`, `/metrics`, `/describe`, `/events`, `/recent` (`tinytrader.observe/1`), plus shared helpers (credentials, the write guard) |
| `libs/fleet` | Discovers and polls a deployment; the data layer behind every front end |
| `libs/schedule` | Active hours on the market calendar, in New York time |
| `libs/dbn` | tinytrader's DBN extension records (trade conditions) |
| `libs/alpaca` | Shared Alpaca client code |
| `libs/tinytrader` | The crates.io umbrella crate |
| `Dockerfile`, `docker-bake.hcl` | Container images of the services, built with Cargo; published to `ghcr.io` by CI |
| `examples/` | Compose files for running the services with a Databento or Alpaca account |
| `docs/` | Tutorials, interfaces, decisions |

## Building and testing

The services build and test with Cargo alone:

```sh
cargo build --release -p console     # one service; binaries in target/release/
cargo build --release                # every service
cargo test --workspace
cargo fmt --all && cargo clippy --workspace --all-targets -- --deny warnings
```

A service runs from its binary the same way as with `cargo run`. The console
has its page built in (`CONSOLE_PAGE` serves it from disk instead, for work on
`services/console/web/`). CI's `cargo` job builds and tests this way.

### Container images

The `Dockerfile` builds any service's image with Cargo in the official Rust
image, onto a distroless base, as user 65532. `docker-bake.hcl` builds every
service's image in one go, compiling the workspace once:

```sh
docker build --build-arg SERVICE=console -t tinytrader/console:local .
docker buildx bake --load                   # every service, as tinytrader/<service>:local
docker buildx bake --load ticker console    # some
```

#### Published images

The `images` workflow builds every service's image on each pull request (for
linux/arm64 only, without pushing), and on each push to `main` and each
`vX.Y.Z` tag builds them for linux/amd64 and linux/arm64 and publishes them as
`ghcr.io/<owner>/tinytrader-<service>`, tagged `main` and `latest` (the same
image), `sha-<short commit>`, and `X.Y.Z` and `X.Y` for releases. It pushes
with the workflow's own `GITHUB_TOKEN`, so no secrets are needed.

GitHub makes a package private when a workflow first pushes it. Once per
package, after its first publish: open the package, then **Package
settings**, **Change visibility**, **Public**. Until then, pulling it fails
with `denied` or `unauthorized`.

## Deploying it

[tinytrader-deploy](https://github.com/dawson-shay/tinytrader-deploy) runs the
whole stack, with out-of-the-box examples:

- **One machine:** Docker Compose, with the published images. Its quick start
  runs the ticker and the console in one `docker compose up`.
- **A network:** machines on a tailnet, scheduled with Nomad.

A service started any other way joins a deployment by being listed by
address in the [topology file](docs/api/fleet.md#the-topology-file), as in
the quick start above.

Read [CONTRIBUTING.md](CONTRIBUTING.md) before contributing: tinytrader is a
platform, and your hosts, data and secrets belong in your own repository.

# Quickstart

Run a small tinytrader deployment on your own machine: a toy collector that
streams live Coinbase prices to disk, and the tinytrader console showing the
deployment graph and live state of every service.

You need Docker with Compose. No Nix, accounts or API keys.

```sh
cd examples/quickstart
docker compose up -d             # pulls the images; nothing to build
open http://localhost:8080       # or visit it in any browser
```

The first start downloads the images (about 15 MB each); later starts take
seconds. To build the images yourself instead, see [Images](#images).

## What runs

| Service | Does |
|---|---|
| `ticker` | Subscribes to Coinbase's public ticker feed for BTC-USD and ETH-USD and appends every message to `/data/<product>/<YYYY-MM-DD>.jsonl`. Serves `/health`, `/metrics` and `/recent` on the Compose network. |
| `console` | Serves the console at `http://localhost:8080`, reachable from this machine only. Polls every service and Docker every two seconds. |
| `docker-proxy` | Read-only Docker API. The console reads container state, uptime, restarts, CPU and memory through it, and cannot start, stop or change anything. |

The graph is drawn from [`topology.json`](topology.json). Edit it and the
console picks up the change within 30 seconds.

## Images

`docker compose up` pulls the published images,
`ghcr.io/dawson-shay/tinytrader-<service>:main`, built from `main` for
linux/amd64 and linux/arm64. Two variables choose others:

| Variable | Default | Meaning |
|---|---|---|
| `TINYTRADER_IMAGE_PREFIX` | `ghcr.io/dawson-shay/tinytrader-` | Image name before the service name |
| `TINYTRADER_TAG` | `main` | Image tag: `main`, `sha-<short commit>`, or a release, `X.Y.Z` |

**Build from this checkout** with the repository's `Dockerfile` (needs only
Docker; the first build compiles the workspace and takes a few minutes):

```sh
docker compose -f compose.yaml -f compose.build.yaml up -d --build
```

**Nix images** (optional), built with Nix inside Docker and tagged
`tinytrader/<service>:nix`:

```sh
../../nix/build-images.sh ticker console
TINYTRADER_IMAGE_PREFIX=tinytrader/ TINYTRADER_TAG=nix docker compose up -d
```

`CONSOLE_PORT` moves the console from port 8080, for example when another
console already uses it.

## Things to try

- **Watch a failure.** `docker compose stop ticker` turns it red in the graph
  within a few seconds; `docker compose start ticker` brings it back.
- **Read the data.** The images have no shell, so look through a throwaway
  container:
  ```sh
  docker run --rm -v tinytrader-quickstart_ticker-data:/d alpine sh -c 'ls -lR /d; tail -n 2 /d/BTC-USD/*.jsonl'
  ```
- **Query the console's API.** Everything the page shows comes from here:
  ```sh
  curl -s http://localhost:8080/api/topology
  curl -s http://localhost:8080/api/state
  ```
- **Change products.** Set `TICKER_PRODUCTS` in [`compose.yaml`](compose.yaml),
  for example `BTC-USD,ETH-USD,SOL-USD`, then `docker compose up -d`.

## Stop

```sh
docker compose down          # stop, keep the recorded data
docker compose down -v       # stop and delete the recorded data
```

## Notes

- `ticker` is a toy for exercising the platform. It is not a recorder: files
  are plain appends without commits, recovery or gap accounting beyond its
  metrics.
- The console binds to `127.0.0.1` only. Exposing it further, for example
  over Tailscale, belongs to a real deployment's configuration.
- Access to the Docker socket is equivalent to root on this machine, which is
  why the console only reaches it through the read-only proxy.

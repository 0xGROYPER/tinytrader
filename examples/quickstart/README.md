# Quickstart

Run a small Glacier deployment on your own machine: a toy collector that
streams live Coinbase prices to disk, and the Glacier console showing the
deployment graph and live state of every service.

You need Docker with Compose. No Nix, accounts or API keys.

```sh
cd examples/quickstart
docker compose up --build -d
open http://localhost:8080        # or visit it in any browser
```

The first build compiles the services and takes a few minutes. Later starts
take seconds.

## What runs

| Service | Does |
|---|---|
| `ticker` | Subscribes to Coinbase's public ticker feed for BTC-USD and ETH-USD and appends every message to `/data/<product>/<YYYY-MM-DD>.jsonl`. Serves `/health`, `/metrics` and `/recent` on the Compose network. |
| `console` | Serves the console at `http://localhost:8080`, reachable from this machine only. Polls every service and Docker every two seconds. |
| `docker-proxy` | Read-only Docker API. The console reads container state, uptime, restarts, CPU and memory through it, and cannot start, stop or change anything. |

The graph is drawn from [`topology.json`](topology.json). Edit it and the
console picks up the change within 30 seconds.

## Things to try

- **Watch a failure.** `docker compose stop ticker` turns it red in the graph
  within a few seconds; `docker compose start ticker` brings it back.
- **Read the data.** The images have no shell, so look through a throwaway
  container:
  ```sh
  docker run --rm -v glacier-quickstart_ticker-data:/d alpine sh -c 'ls -lR /d; tail -n 2 /d/BTC-USD/*.jsonl'
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

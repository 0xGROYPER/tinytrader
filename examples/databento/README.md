# Databento example

A Databento live subscription streamed into the Glacier console. Select
`databento-live` in the deployment graph to watch records arrive.

You need Docker with Compose and a Databento API key whose plan covers the
subscription. **Live data can be billed by usage:** check what the dataset and
schema below cost on your plan before starting.

```sh
cd examples/databento
export DATABENTO_API_KEY=...          # from your Databento account
docker compose up --build -d
open http://localhost:8080
```

The key reaches the service as a Compose secret taken from your shell's
environment. It is never written to disk or into the image.

## Choosing the subscription

| Variable | Default | Meaning |
|---|---|---|
| `DATABENTO_DATASET` | `OPRA.PILLAR` | Dataset to subscribe to |
| `DATABENTO_SYMBOLS` | `SPY.OPT` | Comma-separated symbols |
| `DATABENTO_SCHEMA` | `trades` | Record schema |
| `DATABENTO_STYPE_IN` | `parent` | How symbols are interpreted |

For example, `DATABENTO_DATASET=GLBX.MDP3 DATABENTO_SYMBOLS=ES.FUT docker compose up -d`.
Prefer low-volume schemas such as `trades` while trying things out.

## What runs

| Service | Does |
|---|---|
| `databento-live` | Holds one live session, reconnects with backoff, and serves `/health`, `/metrics` and `/events` on the Compose network. It does not record data. |
| `console` | The Glacier console at `http://localhost:8080`, reachable from this machine only. |
| `docker-proxy` | Read-only Docker API for container state. |

Outside market hours an options or futures feed may be quiet: the service
stays ready and shows heartbeats in its metrics, but few or no records.

```sh
docker compose down
```

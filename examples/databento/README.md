# Databento example

A Databento live subscription held by a tinytrader relay, which serves it to
programs on this machine at `127.0.0.1:7600` (`tinytrader.relay/1`: one line
of JSON, then DBN), with the tinytrader console beside it. Select `relay` in
the System graph to watch its events and rates.

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
Prefer low-volume schemas such as `trades` while trying things out (`DATABENTO_SCHEMA=trades`; the default is `cmbp-1`).

## What runs

| Service | Does |
|---|---|
| `relay` | Holds one live session, reconnects with backoff, and serves it to clients on port 7600, with `/health`, `/metrics`, `/events` and minute bars on the Compose network. It does not record data. |
| `console` | The tinytrader console at `http://localhost:8080`, reachable from this machine only. |
| `docker-proxy` | Read-only Docker API for container state. |

Outside market hours an options or futures feed may be quiet: the service
stays ready and shows heartbeats in its metrics, but few or no records.

```sh
docker compose down
```

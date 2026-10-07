# Alpaca example

Alpaca's free real-time IEX trades and quotes streamed into the tinytrader
console, with market news (supplied by Benzinga through Alpaca) and the
balances and positions of your account. The home page shows the account,
charts and news; select `relay` in the System graph to watch its events. Your
own programs can read the feed from the relay at `127.0.0.1:7610`
(`tinytrader.relay/1`: one line of JSON, then DBN).

You need Docker with Compose and an Alpaca account (a free paper-trading
account is enough). Create API keys in Alpaca's dashboard.

```sh
cd examples/alpaca
export APCA_API_KEY_ID=... APCA_API_SECRET_KEY=...
docker compose up --build -d
open http://localhost:8080
```

The keys reach the service as Compose secrets taken from your shell's
environment; they are never written to disk or into the image.

| Variable | Default | Meaning |
|---|---|---|
| `ALPACA_SYMBOLS` | `SPY,QQQ,AAPL` | Comma-separated symbols |
| `ALPACA_CHANNELS` | `trades,quotes` | Which streams to subscribe to |

IEX trades and quotes flow during US market hours, with less activity before
and after them.

```sh
docker compose down
```

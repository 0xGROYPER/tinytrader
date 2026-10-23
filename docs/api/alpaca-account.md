# alpaca-account

| | |
|---|---|
| **Purpose** | Read Alpaca brokerage accounts (paper or live) and serve their balances, positions, open orders, performance and round-trip trades. Read only: it never places orders (that's the [order gateway](order-gateway.md)). |
| **Transport** | HTTP/1.1, JSON, on `ALPACA_ACCOUNT_LISTEN` (default `127.0.0.1:9107`). |
| **Versioning** | Unversioned paths; additive changes only. |
| **Auth** | None (all reads). Its Alpaca keys never leave it. Bind to loopback or a private network. |
| **Errors** | `{"error": "..."}`: `404` for an unknown or missing `account`, `400` for a bad `date`, `502` when Alpaca fails. |
| **Health** | [Observe](observe.md) endpoints, plus `/events` (positions opened, changed, closed) and `/recent` (equity per account). `idle` outside `--hours`. |
| **Stability** | `/accounts` and `/calendar` are **stable**; `/performance` and `/trades` are **experimental**. |

## Configuration

| Variable | Default | |
|---|---|---|
| `ALPACA_ACCOUNTS` | unset | Several accounts: `NAME=LABEL;NAME=LABEL`. Keys come from `<secrets>/<NAME>_key_id` and `<NAME>_secret_key`. |
| `ALPACA_ACCOUNT_SECRETS` | `/run/secrets` | The directory holding those key files. |
| `APCA_API_KEY_ID`, `APCA_API_SECRET_KEY` (or `_FILE`) | | One account, named `account`, labelled `ALPACA_ACCOUNT_LABEL` (default `Alpaca`), when `ALPACA_ACCOUNTS` is unset. |
| `ALPACA_ACCOUNT_HOURS` | always | `HH:MM-HH:MM` New York, on trading days. |
| `ALPACA_ACCOUNT_RELAYS` | none | Price positions from relays during the session: `stocks=HOST:PORT;ROOT=HOST:PORT` (stream addresses, [`tinytrader.relay/1`](relay.md)). |
| `ALPACA_ACCOUNT_REFRESH` | 600 | Seconds between full re-reads (at least 30). |

A key starting `PK` is a paper key; others are live. Account names are the deployment's own; nothing in tinytrader expects particular names.

## `GET /accounts`

```json
{"accounts": [{"name": "paper", "label": "Paper", "environment": "paper", "updated_at": 1791200000.5,
  "account": {"status": "ACTIVE", "currency": "USD", "equity": 100000.0, "cash": 50000.0, "buying_power": 200000.0},
  "positions": [{"symbol": "SPY", "asset_class": "us_equity", "qty": 10.0, "market_value": 6690.0, "priced_by": "relay"}],
  "orders": [{"client_order_id": "strat-1", "status": "new", "order_class": "mleg", "qty": 1.0, "limit_price": -0.54,
    "created_at": "2026-10-12T16:00:00Z", "legs": [{"symbol": "SPY261016C00775000", "side": "sell", "ratio_qty": 1.0}]}]}]}
```

- Before the first read an entry has only `name`, `label` and `environment`.
- `account`: `status`, `currency`, `equity`, `last_equity`, `cash`, `buying_power`, `long_market_value`, `short_market_value`, `initial_margin`, `maintenance_margin`, `daytrade_count`, `pattern_day_trader`, `trading_blocked`, `account_blocked`. Alpaca's numeric strings become numbers (dollars).
- `positions[]`: `symbol`, `asset_class`, `side`, `qty`, `avg_entry_price`, `current_price`, `lastday_price`, `market_value`, `cost_basis`, `unrealized_pl`, `unrealized_plpc` (a fraction), `unrealized_intraday_pl`, `unrealized_intraday_plpc`, `change_today` (a fraction); during a session also `priced_by` (`relay` or `alpaca`).
- `orders`: open orders, oldest first, or `null` if they couldn't be read. Alpaca's order id and the account number are never served.
- `updated_at` is Unix seconds; order times are RFC 3339.

## `GET /calendar`

The market calendar: `{"updated_at", "clock": {"is_open", "timestamp", "next_open", "next_close"}, "days": [{"date": "YYYY-MM-DD", "open": "HH:MM", "close": "HH:MM"}]}`, New York times, from a week ago onward. `null` before the first read. Other services take it as their `*_CALENDAR` URL (databento-jobs, relays, a deployment's batch jobs).

## `GET /performance?account=NAME`

A year of daily equity and P&L, and fill counts by day:

```json
{"account": "paper", "environment": "paper",
 "days": [{"date": "2026-10-14", "equity": 886.17, "pnl": 289.05, "pnl_pct": 0.4841, "transfers": 500.0}],
 "trades": {"2026-10-14": {"trades": 3, "closed": 2, "wins": 1}}}
```

`pnl` excludes deposits and withdrawals; `pnl_pct` is a fraction; `transfers` appears only when non-zero. Cached for a minute.

## `GET /trades?account=NAME&date=YYYY-MM-DD`

One day's round trips (flat to flat, per stock or per option root and expiry):

```json
{"account": "paper", "date": "2026-10-15", "fills": 7,
 "trades": [{"key": "SPY 2026-10-15", "start": "2026-10-15T14:01:00Z", "end": "2026-10-15T15:30:00Z",
   "fills": 6, "contracts": ["SPY261015C00769000"], "open": false, "pnl": 74.0}]}
```

`pnl` is in dollars before fees, `null` while `open`.

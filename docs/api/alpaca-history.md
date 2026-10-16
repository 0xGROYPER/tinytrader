# alpaca-history

| | |
|---|---|
| **Purpose** | Fetch past days of Alpaca stock trades and quotes, encode them as DBN (`ALPACA.SIP` `cmbp-1`, `ALPACA.IEX` `mbp-1`, with `tinytrader.trade-conditions/1` sidecars) and file them in the [store](store.md) ([decision 0014](../decisions/0014-history-feeds.md)). |
| **Transport** | HTTP/1.1, JSON, on `HISTORY_LISTEN` (default `127.0.0.1:9124`). |
| **Versioning** | Unversioned paths. Files it writes carry `tinytrader.alpaca-history.receipts/1` and `tinytrader.alpaca-history.conditions-binding/1`. |
| **Auth** | Writes are refused from a web page of another origin; no token of its own yet. It writes to the store with `HISTORY_STORE_TOKEN` (or `_FILE`). |
| **Errors** | `{"error": "..."}`: `400` invalid run, `404` unknown run. |
| **Health** | [Observe](observe.md) endpoints and `/events`. `idle` with nothing queued or while waiting. |
| **Stability** | **Experimental.** |

## Configuration

| Variable | |
|---|---|
| `ALPACA_HISTORY_KEY_ID`, `ALPACA_HISTORY_SECRET_KEY` (or `_FILE`) | Required. |
| `HISTORY_STORE` | The store (default `http://127.0.0.1:9120`). |
| `HISTORY_STATE` | Where runs and the spool are kept (default `/data`). |
| `HISTORY_TOPOFF` | Symbols to top off every evening (none by default). |
| `HISTORY_YIELD_TO`, `HISTORY_CAPTURE_HOURS` | Hold off while capture is live. |
| `HISTORY_MAX_RPM` | Requests a minute (default 190). |

## Endpoints

| Endpoint | |
|---|---|
| `GET /runs` | `{"runs": [{id, feed, symbols, start, through, state, from, planned, committed, held, failed, bytes, days: {"SYM YYYY-MM-DD": {state, trades, quotes, pages, bytes}}}]}`, newest first. `state` is `queued`, `running`, `done`, `stopped` or `failed`. |
| `POST /runs` | `{feed: "sip" \| "iex", symbols: [1..50], start, through, order?: "oldest-first" \| "newest-first"}` → the run. |
| `POST /runs/{id}/stop` | `{"stopped": id}`, after the current day. |

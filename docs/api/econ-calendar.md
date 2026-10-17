# econ-calendar

| | |
|---|---|
| **Purpose** | Scheduled US economic releases (BLS, BEA, the Fed, weekly claims, oil inventories), earnings report dates from several sources, past earnings with surprises, and the SEC's ticker list. |
| **Transport** | HTTP/1.1, JSON, on `ECON_LISTEN` (default `127.0.0.1:9122`). |
| **Versioning** | Unversioned paths; additive changes only. |
| **Auth** | None (all reads). |
| **Errors** | `{"error": "..."}`: `503` before the first read (or without `SEC_CONTACT` for `/tickers`), `400` bad symbol, `502` an upstream failed. |
| **Health** | [Observe](observe.md) endpoints. It has no observe event log: its `/events` is the release calendar (declared as `econ`). |
| **Stability** | `/events` and `/earnings` are **stable**; `/history` and `/tickers` are **experimental**. |

## Configuration

| Variable | |
|---|---|
| `ECON_CONTACT` | A contact address for BLS's User-Agent; without it BLS is skipped. |
| `SEC_CONTACT` (or `_FILE`) | The same for SEC EDGAR (`/history`, `/tickers`). |
| `ALPHAVANTAGE_API_KEY`, `FINNHUB_API_KEY` (or `_FILE`) | Optional earnings sources. |
| `EARNINGS_WATCH` | Symbols always rated `high`. |
| `EARNINGS_DAYS` | Days ahead to read (default 42). |
| `ECON_REFRESH_HOURS` | Hours between reads (default 24). `ECON_CACHE`: a cache directory. |

## Endpoints

| Endpoint | |
|---|---|
| `GET /events?from=&to=&min=` | `{"events": [{at, date, time, title, source, importance, category}], "sources": {name: {events, error, read_at}}, from, to, min}`. `at` is Unix seconds; `date` and `time` are New York. `min` is `low`, `medium` (default) or `high`. |
| `GET /earnings?from=&to=&min=` | `{"reports": [{symbol, name, date, when, eps_estimate, eps_actual, market_cap, dates, disputed, importance}], "sources": {...}, ...}`. `when` is `before-open`, `after-close` or `unknown`; `dates` gives each source's date, `disputed` when they differ. |
| `GET /history?symbol=` | `{"symbol", "reports": [{date, when, fiscal, eps, estimate, surprise_pct}], "sources", "estimates"}`: up to 12 past reports. |
| `GET /tickers` | `{"fetched_at", "names": {SYMBOL: name}}` from EDGAR. |

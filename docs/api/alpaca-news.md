# alpaca-news

| | |
|---|---|
| **Purpose** | Market news as it's published: Alpaca's news stream (Benzinga), caught up over REST at start and after each reconnect, merged with optional RSS feeds. |
| **Transport** | HTTP/1.1, JSON, on `ALPACA_NEWS_LISTEN` (default `127.0.0.1:9106`). |
| **Versioning** | Unversioned; additive changes only. |
| **Auth** | None (all reads). |
| **Errors** | None of its own; upstream failures show in `/health`'s `last_error`. |
| **Health** | [Observe](observe.md) endpoints, plus `/events` (one per new story, `raw` the story as JSON). `starting` until the stream subscribes. |
| **Stability** | **Experimental**: items are stored as their source sent them. |

## Configuration

| Variable | Default | |
|---|---|---|
| `APCA_API_KEY_ID`, `APCA_API_SECRET_KEY` (or `_FILE`) | required | |
| `ALPACA_NEWS_SYMBOLS` | all news | Comma-separated symbols. |
| `ALPACA_NEWS_RSS` | none | Comma-separated RSS feed URLs, each read every 5 minutes. |
| `--keep` | 300 | Stories kept. |

## `GET /news`

```json
{"items": [{"id": 24918784, "headline": "...", "summary": "...", "author": "...", "source": "benzinga",
  "url": "https://...", "symbols": ["SPY"], "created_at": "2026-10-12T15:20:30Z", "updated_at": "2026-10-12T15:20:30Z"}],
 "updated_at": 1791200000.2}
```

Newest first. Items are kept exactly as their source sent them, so stream and REST items differ slightly in extra fields; every item has `id`, `headline`, `summary`, `source`, `url`, `symbols`, `created_at` and `updated_at` (RFC 3339). RSS items have string ids `rss:<guid>`; Alpaca's are integers. `updated_at` at the top is Unix seconds, `null` before the first read.

For a search over past news by symbol, use alpaca-data's [`/news`](alpaca-data.md#endpoints).

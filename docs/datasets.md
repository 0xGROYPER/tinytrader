# Dataset names

Datasets are named `VENUE.FEED`, following Databento. Where Databento already
names a dataset, use its name unchanged (for example `OPRA.PILLAR`,
`XNAS.ITCH`). For venues Databento doesn't cover, the feed part is the
venue's own name for the feed.

Add a row here before writing data under a new name, so every deployment
uses the same one.

| Dataset | Venue | Feed |
|---|---|---|
| `COINBASE.EXCHANGE` | Coinbase | Coinbase Exchange market data (websocket) |
| `KRAKEN.SPOT` | Kraken | Kraken spot market data |
| `KRAKEN.FUTURES` | Kraken | Kraken derivatives market data |
| `ALPACA.IEX` | Alpaca | IEX market data as served by Alpaca |
| `ALPACA.SIP` | Alpaca | Consolidated (SIP) US equities data as served by Alpaca |

Schema names are lower case, using Databento's names where the meaning
matches (`trades`, `mbp-1`, `mbo`, `definition`). tinytrader's own schemas are
listed with their extensions once they are defined.

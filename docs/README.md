# tinytrader documentation

tinytrader is a small platform for trading on financial markets: market data
relays, a data store, a console and a terminal view, built on Databento's DBN
format.

- **Tutorials** walk through using it, from the quickstart on.
- **[Widgets](widgets/README.md)** describes each of the console's widgets.
- **[Profile mode](profile-mode.md)** measures everything on a console page while it's used, and records it for reading later.
- **[The implied distribution](implied-distribution.md)** is how option chains become probabilities: the bands on Candles, the Spread builder's chance of profit, the screener's skew.
- **[Market data relay API](relay-api.md)**: the stream protocol and every HTTP endpoint, including today's replay of one option contract (`/session`).
- **[Store API](store-api.md)**: the data store's HTTP endpoints, including reading files back by logical name.
- **Decisions** record why it is built the way it is.
- [Dataset names](datasets.md) lists the data it knows.
- [Recovering a deployment](recovery.md) inventories how state survives a lost disk, and who decides each part.

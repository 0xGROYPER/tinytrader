# tinytrader documentation

tinytrader is a small platform for trading on financial markets: market data
relays, a data store, a console and a terminal view, built on Databento's DBN
format.

- **[Tutorials](tutorials/01-quickstart.md)** walk through using it, from the quickstart on.
- **[Widgets](widgets/README.md)** describes each of the console's widgets.
- **[Profile mode](profile-mode.md)** measures everything on a console page while it's used, and records it for reading later.
- **[The implied distribution](implied-distribution.md)** is how option chains become probabilities: the bands on Candles, the Spread builder's chance of profit, the screener's skew.
- **[Interfaces](api/README.md)**: every client-facing contract, with shared conventions: the [relay](api/relay.md) (`tinytrader.relay/1`), the [order gateway](api/order-gateway.md) (`tinytrader.gateway/1`), the [store](api/store.md), the [observe endpoints](api/observe.md), the [topology file and console API](api/fleet.md), and each service.
- **[Write a strategy as a client](tutorials/strategy-client.md)**: market data from a relay, orders through a gateway, paper and backtests with the sim broker.
- **[Decisions](decisions/)** record why it is built the way it is.
- [Dataset names](datasets.md) lists the data it knows.
- [Recovering a deployment](recovery.md) inventories how state survives a lost disk, and who decides each part.

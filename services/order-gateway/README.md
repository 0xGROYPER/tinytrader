# order-gateway

One broker account behind the `tinytrader.gateway/1` order protocol: the only
holder of the account's keys, with validation, caps, a kill switch and a
hash-chained journal. Brokers: Alpaca (paper, and live with `--allow-live`)
and a simulated broker that replays DBN quotes.

- The protocol, its options and the simulator: [docs/api/order-gateway.md](../../docs/api/order-gateway.md).
- Writing a client: [docs/tutorials/strategy-client.md](../../docs/tutorials/strategy-client.md).
- A minimal Rust client: [`examples/strategy_client.rs`](examples/strategy_client.rs), and [`src/client.rs`](src/client.rs).

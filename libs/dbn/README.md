# tinytrader-dbn

Extensions to [Databento's DBN format](https://databento.com/docs/standards-and-conventions/databento-binary-encoding)
for records the official [`dbn`](https://crates.io/crates/dbn) crate doesn't define yet. It re-exports
`dbn`, so one dependency covers both.

- `TradeConditions` (`tinytrader.trade-conditions/1`): a trade's sale condition codes, kept exactly as
  the venue sent them, as a 48-byte record (rtype `0xD4`) that follows its trade. Standard DBN readers
  skip it by length.

Each extension is meant to be retired once Databento ships an equivalent.

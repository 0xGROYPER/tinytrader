# tinytrader-dbn

Extensions to [Databento's DBN format](https://databento.com/docs/standards-and-conventions/databento-binary-encoding)
for records the official [`dbn`](https://crates.io/crates/dbn) crate doesn't define yet. It re-exports
`dbn`, so one dependency covers both.

- `TradeConditions` (`tinytrader.trade-conditions/1`): a trade's sale condition codes, kept exactly as
  the venue sent them, as a 48-byte record (rtype `0xD4`) that follows its trade. Standard DBN readers
  skip it by length.

- `Merge` (decision 0009, "Ordering"): a lazy k-way merge of DBN sources on one key, `(ts, class, seq)`.
  `ts` is a record's index time: `ts_recv`, or `ts_event` plus the source's declared latency offset where
  the record is flagged `F_BAD_TS_RECV` (filed IEX). Ties keep the sources' order, then each source's own,
  so a merge is the same every time. An extension record (or one with no time) stays right after the
  record before it in its source; a flagged record whose `ts_event` steps back is clamped and counted; any
  other step back is an error, and errors are sticky.

Each extension is meant to be retired once Databento ships an equivalent.

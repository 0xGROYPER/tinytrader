# tinytrader-client (Python)

A strategy as a client of a tinytrader session ([decision 0022](../../docs/decisions/0022-sessions.md), [the protocol](../../docs/api/session.md)). One class, the same code in backtest, paper and live:

```python
from tinytrader_client import Strategy, run, single, combo

class MyStrategy(Strategy):
    name, revision = "my-strategy", "abc123"
    def on_start(self, ctx): ...                    # ctx.every(60, "09:31", "15:59"), ctx.set_timer(at, id)
    def on_record(self, ctx, rec): ...              # every delivered DBN record (databento_dbn objects)
    def on_report(self, ctx, venue, event): ...     # gateway events, a step's before its timers
    def on_timer(self, ctx, timer_id, ts): ...
    def on_end(self, ctx, end): ...

run(MyStrategy())   # TT_SESSION, or run(s, "127.0.0.1:PORT")
```

**`ctx`** gives you:
- `now`, the session's time in ns;
- `book.quote(symbol)`, `book.chain(root, expiry)` and `book.expiries(root)`: the last quote per instrument, with its time;
- orders: `submit(intent)`, which returns the client order id, and `cancel(cid)`;
- queries: `positions()`, `account()` and `open_orders()`;
- `record(series, value)`, collected in what `run` returns.

Name the venue when a session has several.

**Intents:**
- `single(symbol, side, effect, qty, limit)`;
- `combo([(symbol, ratio, side, effect), …], qty, net_limit)`, where a credit is negative.

The gateway has no `replace`, so cancel and resubmit, waiting for the `canceled` event in between. It refuses market orders that open a position, so open with marketable limits.

Tests: `cargo build -p session --bins --examples`, then `python -m unittest discover -s tests` here. They run a toy strategy against a real session over a synthetic fixture.

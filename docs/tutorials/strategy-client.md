# Write a strategy as a client

tinytrader doesn't run your strategy. It runs the servers at the edge: a **relay** streams live market data as DBN, the **store** serves historic DBN, and an **order gateway** holds a broker account behind a small order protocol, with a **simulated broker** for paper trading and backtests. Your strategy is a program, in its own repository and any language, that connects to them.

```text
            tinytrader.relay/1 (DBN)                tinytrader.gateway/1 (NDJSON)
 relay  ───────────────────────────►  your strategy  ─────────────────────────────►  order gateway ──► broker
 store  ── GET /files/... (DBN) ──►                                                   (or the sim broker)
```

This walks through both connections. The references are [relay](../api/relay.md), [order gateway](../api/order-gateway.md) and [store](../api/store.md); the shared conventions are in the [API overview](../api/README.md). A complete Rust client is [`services/order-gateway/examples/strategy_client.rs`](../../services/order-gateway/examples/strategy_client.rs).

## 1. Find the servers

Your deployment decides the addresses. The console's [`/api/topology`](../api/fleet.md#apitopology) lists every relay with what it carries (`relay: {protocol, listen, symbols}`), so a client can look relays up by the symbols it needs instead of hard-coding a host. Gateways are one process per account, each on its own loopback port, named in your deployment's configuration.

## 2. Market data from a relay

Connect over TCP and send one line of JSON; the relay answers with a DBN v3 stream.

```json
{"protocol": "tinytrader.relay/1", "client": "my-strategy", "account": "paper", "start": "2026-10-12T13:30:00Z"}
```

- `start` (RFC 3339) replays from that time and then continues live, where the source keeps history (the Alpaca stock relays do; Databento's OPRA relays start live). Leave it out to start live.
- `client` and `account` are only labels, shown on the console beside your connection.
- The stream is: the metadata header, reference records (instrument definitions), the catch-up, a `SystemMsg` with code `ReplayCompleted`, then live records. Quotes before `ReplayCompleted` are history: warm up indicators on them, but don't trade on them.
- Watch for `SystemMsg` text `upstream-down`, `upstream-up gap_from=… gap_to=…` and `upstream-gap`: the provider dropped, and you missed records in that window.
- Read promptly. A client that falls behind is warned (`SlowReaderWarning`) and then disconnected; reconnect with `start` set to the last time you saw.

Map records to symbols with the definitions: `instrument_id` to `raw_symbol` (spaces removed for OCC option symbols).

### In Rust

With Databento's [`dbn`](https://crates.io/crates/dbn) crate, the relay is an ordinary DBN reader:

```rust
use std::io::{BufReader, Write};
use std::net::TcpStream;
use dbn::decode::DecodeRecordRef;
use dbn::{RecordRefEnum, SystemCode};

let mut s = TcpStream::connect("127.0.0.1:7610")?;
s.write_all(b"{\"protocol\":\"tinytrader.relay/1\",\"client\":\"my-strategy\",\"start\":\"2026-10-12T13:30:00Z\"}\n")?;
let mut decoder = dbn::decode::dbn::Decoder::new(BufReader::new(s))?;
let mut live = false;
while let Some(rec) = decoder.decode_record_ref()? {
    match rec.as_enum()? {
        RecordRefEnum::InstrumentDef(d) => { /* remember d.hd.instrument_id -> d.raw_symbol() */ }
        RecordRefEnum::Mbp1(q) => { /* a stock quote or trade: q.levels[0], q.price */ }
        RecordRefEnum::Cmbp1(q) => { /* an option's consolidated quote */ }
        RecordRefEnum::System(m) if matches!(m.code(), Ok(SystemCode::ReplayCompleted)) => live = true,
        RecordRefEnum::Error(e) => return Err(e.err()?.into()), // refused: the reason, then close
        _ => {}
    }
}
```

Prices are DBN fixed point (`1e-9` units; `dbn::UNDEF_PRICE` for an empty side) and times are nanoseconds since the epoch. The example's `market_data` function is the same loop.

### In Python

The [`databento-dbn`](https://pypi.org/project/databento-dbn/) package decodes the same bytes. A sketch:

```python
import json, socket
import databento_dbn as dbn

s = socket.create_connection(("127.0.0.1", 7610))
s.sendall((json.dumps({"protocol": "tinytrader.relay/1", "client": "my-strategy",
                       "start": "2026-10-12T13:30:00Z"}) + "\n").encode())
decoder = dbn.DBNDecoder()
while chunk := s.recv(1 << 16):
    decoder.write(chunk)
    for rec in decoder.decode():
        if isinstance(rec, dbn.Metadata):
            continue
        if isinstance(rec, dbn.SystemMsg) and rec.is_heartbeat():
            continue
        if isinstance(rec, dbn.ErrorMsg):
            raise RuntimeError(rec.err)
        handle(rec)  # InstrumentDefMsg, MBP1Msg, CMBP1Msg, SystemMsg (ReplayCompleted, upstream-*)
```

## 3. History from the store

For backtests and research, read finished DBN files from the [store](../api/store.md): `GET /files?pattern=*cmbp-1*` lists them by logical name, and `GET /files/{volume}/{path}` streams one (`.dbn.zst`, zstd-compressed DBN). Sealed files never change, so cache them.

## 4. Orders through the gateway

The gateway speaks newline-delimited JSON over TCP on loopback. Every message carries `"schema": "tinytrader.gateway/1"`.

### Get a client identity

The gateway's operator gives your strategy a client: a name, a role (`trade` to place orders), an order-id prefix, and a token (64 or more hex characters, in a mode-600 file or a secret):

```text
order-gateway --account paper --broker alpaca-paper --credentials env --listen 127.0.0.1:7601 \
  --state /var/lib/order-gateway/paper --client my-strategy,trade,strat-,/etc/tinytrader/my-strategy.token \
  --equities AAPL,MSFT --underlyings SPY --max-open 4 --max-notional 2500
```

The token is the only credential your strategy holds: the broker's keys stay in the gateway.

### Connect

```json
→ {"schema":"tinytrader.gateway/1","type":"hello","client":"my-strategy","token":"…","resume_after_seq":null}
← {"schema":"tinytrader.gateway/1","type":"welcome","account":"paper","broker":"alpaca","environment":"paper","state":"ready","last_seq":1301,"resume":"none","capabilities":{…},"pins":{…},"limits":{…}}
```

`state` must be `ready` before a submit can go. `limits` and the profile say what the gateway will accept.

### Ask, then act

```json
→ {"schema":"tinytrader.gateway/1","type":"positions","request_id":"r1"}
← {"schema":"tinytrader.gateway/1","type":"response","request_id":"r1","op":"positions","outcome":"ok","positions":[…],…}
→ {"schema":"tinytrader.gateway/1","type":"submit","request_id":"r2","intent":{"client_order_id":"strat-20261016-0001",
   "shape":"single","instrument":{"class":"equity","symbol":"AAPL"},"side":"buy","effect":"open","qty":1,
   "type":"limit","limit_price":"212.50","time_in_force":"day"}}
← {"schema":"tinytrader.gateway/1","type":"response","request_id":"r2","op":"submit","outcome":"accepted",…}
```

Then fills arrive as `event` lines (`kind: "order"`, `event: "fill"`), interleaved with responses.

### The rules that matter

- **`client_order_id` is yours and is never reused.** It starts with your prefix. Sending the same id with the same intent is safe (`duplicate`, nothing sent); a different intent under it is refused.
- **`accepted` is not a fill.** Fills come as events.
- **Never resend after `unknown`.** It means the order may have reached the broker. The gateway looks it up and tells you (`resolution` event); meanwhile ask `order` by id. This is what makes a crash or timeout safe.
- **`not_sent` means nothing reached the broker**: a cap (`detail.cap`), the kill switch, a validation error. Fix the cause; don't loop.
- **Caps are the gateway's, not your risk model.** `--max-open`, `--max-notional`, submits per minute and day, and the allowlist (`--underlyings`, `--equities`) are mechanical limits; closes are always allowed through the opening caps. Your strategy still owns sizing and P&L.
- **Reconnect with `resume_after_seq`** set to the last event `seq` you saw, and the gateway replays what you missed.
- Prices are decimal strings with at most two decimals; timestamps are integer nanoseconds.

A simple daily strategy can avoid following events at all: connect once per run, ask `account`, `positions` and `open_orders`, submit (for example stock limit orders on the close, `time_in_force: "cls"`), and check each new order with `order` before the next run. A strategy that trades intraday keeps one connection open and follows the events.

### In Rust

The gateway crate includes a blocking client, `order_gateway::client::Client` (connect sends `hello`; you send requests and read lines). The example puts it together:

```text
GATEWAY=127.0.0.1:7601 CLIENT=my-strategy TOKEN_FILE=my-strategy.token SUBMIT=AAPL:1.25 \
  cargo run -p order-gateway --example strategy_client
```

### In Python

The protocol needs nothing but a socket:

```python
import json, socket

class Gateway:
    def __init__(self, host, port, client, token):
        self.sock = socket.create_connection((host, port))
        self.lines = self.sock.makefile("r", encoding="utf-8")
        self.send({"type": "hello", "client": client, "token": token, "resume_after_seq": None})
        self.welcome = self.read()
        if self.welcome["type"] != "welcome":
            raise RuntimeError(self.welcome)

    def send(self, msg):
        self.sock.sendall((json.dumps({"schema": "tinytrader.gateway/1", **msg}, separators=(",", ":")) + "\n").encode())

    def read(self):
        return json.loads(self.lines.readline())

    def request(self, kind, request_id, **fields):
        self.send({"type": kind, "request_id": request_id, **fields})
        while True:  # events and heartbeats arrive in between; keep them if you follow events
            m = self.read()
            if m["type"] == "error":
                raise RuntimeError(m["code"])
            if m.get("request_id") == request_id:
                return m
```

## 5. Paper and backtests with the sim broker

### In a session, with the Python client

A **session** ([docs/api/session.md](../api/session.md)) serves held days to a strategy, runs its simulated venues and keeps everyone on one clock. The Python client (`clients/python`, `pip install ./clients/python`) wraps the session, the data stream and the gateways in one class:

```python
from tinytrader_client import Strategy, run, single

class Toy(Strategy):
    name, revision = "toy", "v1"

    def on_start(self, ctx):
        ctx.every(60, start="10:00", end="15:55")          # a timer each minute, New York time

    def on_timer(self, ctx, timer_id, ts):
        q = ctx.book.quote("SPY   261027C00580000")       # the last quote, with its time
        if q and q.usable and q.ask <= 1.25 and not ctx.open_orders():
            ctx.submit(single("SPY261027C00580000", "buy", "open", 1, 1.25))

    def on_report(self, ctx, venue, event):
        if event.get("event") == "fill":
            ctx.record("fill", event["fill"]["price"])

run(Toy())   # the session's control address from TT_SESSION
```

```sh
tinytrader-session run session.toml --dir runs/a &      # prints READY … control=ADDR …
TT_SESSION=ADDR python toy.py
```

The strategy sees only records already delivered. An order takes effect after the step it is sent in. The step's fills reach `on_report` before its timers run. Two runs of the same configuration and strategy give the same audit hash. The class doesn't change between backtest, paper and live: only the session behind `TT_SESSION` does.

### Directly against a sim gateway

The same protocol runs against a simulated broker, so the strategy doesn't change: only `welcome.broker` (`"sim"`) and `pins` differ.

- **Paper against live data:** `--broker sim --sim-quotes relay:HOST:PORT` fills your orders against a relay's live quotes, at the touch.
- **Backtest, reproducibly:** `--broker sim --sim-quotes day.dbn` (a decompressed store file) with the market clock in your hands. Send `{"type":"advance","request_id":"…","to_ts":<ns>}` to apply quotes up to that time; fills come back as events, stamped in market time. The same quotes, the same `pins` and the same requests give a byte-identical transcript.
- **Scripted tests:** `--sim-script FILE` decides each submit's fate (accept, reject, partial fill, timeout after accept, 429, …) with no market data, to test how your strategy handles every outcome, `unknown` included.

A common shape: one strategy binary, configured with a relay address and a gateway address. In research it reads store files and talks to a sim gateway in client-clocked replay; on paper, a live relay and a sim or paper gateway; live, the same with a live gateway with small caps.

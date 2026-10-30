# Sessions (`tinytrader.session/1`)

| | |
|---|---|
| **Purpose** | One run of a strategy against held days: the sources merged on one clock, served over the relay protocol, simulated venues as ordinary gateways, everything advanced in lockstep ([decision 0022](../decisions/0022-sessions.md)). |
| **Transport** | Control: TCP NDJSON. Data: `tinytrader.relay/1` (a JSON join line, then DBN v3). Orders: `tinytrader.gateway/1`, one gateway per venue. All on loopback unless `--listen` says otherwise. |
| **Versioning** | Every control message is in `tinytrader.session/1`; `hello` names it. |
| **Auth** | The control and data addresses are the run's own and go unauthenticated, as the relay's do; each venue's gateway token is in `welcome`. |
| **Errors** | A run that can't go on ends with `end{status: "incomplete", reason}`, and the process exits 1. |
| **Stability** | **Experimental** until 0022's later steps (paper and live sessions) land. |

A session is a process per run, not a service. Backtest mode is built; `paper` and `live` are later steps.

## Running one

```sh
tinytrader-session run session.toml --dir runs/2026-10-27-a
# READY session=5f1c… mode=backtest control=127.0.0.1:53001 data=127.0.0.1:53002 orders=options=127.0.0.1:53003
```

The strategy reads the control address (from the `READY` line, or `TT_SESSION` set from it) and finds everything else in `welcome`. The run folder must not be under `/tmp`, as for any gateway state. It ends up holding:
- `run.json`, the run record;
- `audit.log`;
- `config.toml`;
- each venue's journal under `venues/<name>/`.

## Configuration

```toml
days = ["2026-10-26", "2026-10-27"]   # in order
strategy_latency_ms = 0               # the strategy's declared latency
ready_timeout_s = 60                  # per step, and for the first connections

[[sources]]                           # in rank order: ties in time go to the earlier
name = "opra"
holdings = "store:http://127.0.0.1:9120"   # or "job:DIR" (a databento-jobs cache job), "folder:DIR" (the store's layout)
provider = "databento"
dataset = "OPRA.PILLAR"
symbol = "SPY.OPT"
schema = "cbbo-1m"
latency_offset_ns = 0                 # added to ts_event where F_BAD_TS_RECV is set

[[venues]]                            # one simulated account, one gateway
name = "options"
quotes = ["opra"]                     # the sources it fills against
underlyings = ["SPY"]
equities = []
max_notional = 2500                   # dollars per order
max_open = 4
allow_short = false
[venues.sim]                          # the simulator's profile: order-gateway.md §10's script keys
latency_ms = 5
fee_per_contract = "0.65"
```

Sources are read through the store producer (`libs/producer`). A day not held is a `store-gap` message in the stream, never a skip. Each file read is pinned by SHA-256 in the run record.

## The control connection

| Direction | Message |
|---|---|
| strategy → session | `{"type":"hello","protocol":"tinytrader.session/1","strategy":NAME,"revision":REV}` |
| session → strategy | `{"type":"welcome","session","mode":"backtest","data":ADDR,"orders":{VENUE:{"addr","client","token","prefix"}},"days","strategy_latency_ms"}` |
| strategy → session | `{"type":"ready","as_of":0,"sent":{}}`, once joined to the data stream and every venue |
| session → strategy | `{"type":"step","step":N,"as_of":TS,"records":K,"timers":[ID],"reports":{VENUE:SEQ}}` |
| strategy → session | `{"type":"ready","as_of":TS,"sent":{VENUE:COUNT},"timers":[{"at":TS,"id":ID}]}` |
| session → strategy | `{"type":"end","status","reason","audit","steps","records"}` |

**A step** is the next time anything happens: the records of one index time (decision 0009's order: `ts_recv`, or `ts_event` plus the source's offset where flagged), timers due then, or both. For each step the session:
1. pushes the records to the venues that quote their source;
2. advances every venue to `as_of`, once it has handled the order messages the strategy last said it sent (venue reports, class 2);
3. writes the records to the data stream;
4. sends `step`, whose `timers` are the timers due at `as_of` (class 3).

**The strategy:**
1. reads `records` records from the data stream;
2. reads each venue's gateway events up to the `reports` sequence number, so it has every fill of the step before acting on it;
3. acts;
4. answers `ready`.

**`sent`** counts the submits and cancels the strategy has sent to each venue since it first connected, valid or not, resent ones included; queries don't count ([order-gateway.md §10](order-gateway.md#10-simulated-broker)). Orders take effect at `as_of` plus the strategy's and the venue's latencies.

**Timers** are set in `ready`, each for a time after the step's.

**After the last record**, the venues are advanced to the end of the last day, so day orders expire, and `end` follows.

A strategy that doesn't answer within `ready_timeout_s` ends the run `incomplete`. So does one that answers for another time, or whose `sent` goes back.

## The data stream

Join as for a relay: `{"protocol":"tinytrader.relay/1","client":NAME}`. The session sends a DBN v3 header, then each step's records unchanged, as the store holds them:
- each day's symbol mappings at the day's start;
- the stored records;
- `store-gap` and `store-partial` notices.

With one source, the header's schema is that source's. With several, it's unset (mixed), and each record's rtype says what it is.

## The run record and the audit hash

`run.json` (`tinytrader.session-run/1`) holds:
- the status and reason;
- the audit hash;
- the steps and records;
- the strategy's name and revision;
- the days;
- the lockstep batch and strategy latency;
- the merge's rank order;
- the configuration's SHA-256;
- per source: its holdings, records, clamps and each day's files with their SHA-256s;
- per venue: the simulator model, its profile, the sources it quotes, and the order messages sent.

The **audit hash** is a SHA-256 over `audit.log`, which records:
- the strategy's `hello`;
- every step (its time, each source's span of positions, the timers fired, each venue's last event);
- every `ready`;
- the final advance;
- each venue's journaled order events (submits, cancels, broker order events), without their sequence numbers and hash-chain links, which also count connection events.

The same configuration, the same held days and the same strategy give the same hash.

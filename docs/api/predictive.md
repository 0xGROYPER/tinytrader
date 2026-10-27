# Predictive publications (`tinytrader.predictive/1`)

| | |
|---|---|
| **Purpose** | A deployment publishes a predictive distribution with its measure, its status and its provenance, so that other clients can read it without knowing the model. |
| **Transport** | Written through the trading runtime's inbound port ([decision 0009](../decisions/0009-trading-runtime.md)). Stored and read through the store service ([store](store.md)), as immutable records pinned by hash. |
| **Versioning** | Every record carries `"schema": "tinytrader.predictive/1"`. A reader refuses a major version it does not know. |
| **Auth** | As the store: writes carry the store's token; reads follow the store's rules. |
| **Errors** | Write refusals return a typed code (see Refusals). Reads return `{"error": "..."}` with a 4xx or 5xx status. |
| **Health** | The store's and the observe endpoints ([observe](observe.md)). Refusal counts by code, and the last `seq` for each producer name and lane. |
| **Stability** | **Experimental** until the runtime prototype in [decision 0018](../decisions/0018-predictive-publications.md) passes. |

Shared conventions are in the [API overview](README.md).

## Record

| Field | Type | Meaning |
|---|---|---|
| `schema` | string | `tinytrader.predictive/1` |
| `publication` | hash | SHA-256 of the canonical record, excluding this field |
| `seq` | integer | `1` or more. Contiguous within its stream: the same producer name, instrument, target and lane. A gap publication takes a number too. See Ordering and delivery. |
| `producer` | object | `{ "name": ..., "version": ... }`, the deployment's own name for the model |
| `as_of` | integer | UTC nanoseconds: the last input the distribution uses. Readers act only at or after this time |
| `valid_until` | integer, optional | UTC nanoseconds |
| `instrument` | string | a name from the [dataset registry](../datasets.md) |
| `target` | object | `{ "kind": ..., "params": ... }`, in the producer's declared vocabulary |
| `units` | string | the units of the quantiles, stated, never implied |
| `lane` | `risk_neutral` or `real_world` | the measure the distribution is under |
| `status` | object | `{ "state": "unpromoted" }`, or `{ "state": "promoted", "evidence": ... }` |
| `quantiles` | array | `[{ "level": ..., "value": ... }]`, sorted by level, with the levels stated |
| `moments` | object, optional | `{ "mean": ..., "variance": ... }` |
| `gap` | object or null | `{ "kind": ..., "from": ..., "to": ... }` when the producer had no valid input in the interval |
| `provenance` | object | `{ "code_sha256": ..., "input_sha256": [...], "parent": hash or null }` |

## Lanes and status

- `risk_neutral` distributions are pricing inputs. They are not probabilities of outcomes.
- `real_world` distributions are expectations of outcomes. A `real_world` record is accepted only with
  `"state": "promoted"` and an `evidence` reference. Otherwise it is refused at write.
- A consumer must read `lane` and `status` before using a value as a probability.

## Gaps

A gap is a record, not an absence. A producer that had no valid input for an interval writes a publication with `gap`
set, and no quantiles. The platform never fills a gap, and a consumer must treat a gap as "no answer", not as zero. A gap publication takes the next `seq` of its stream. A `seq` that never appears is not a gap: it is a delivery failure (see Ordering and delivery).

## Ordering and delivery

- **Writes.** The store accepts a publication only if its `seq` is the next one for its stream, or if it repeats an accepted publication exactly (same `publication` hash, which changes nothing). Any other write is refused with `seq_violation`. A stream is identified by the producer's name, so a new version continues the numbering.
- **Restarts.** A producer keeps its last `seq` per stream with its state, and reads the last number from the store before it writes again. The store API does not define that read yet.
- **Reads.** Readers read from the store by `as_of`, which gives no look-ahead, and by `seq` within a stream. A reader that needs every publication checks that the numbers are contiguous. A missing number is an integrity failure to report, and the reader does not use the stream past it until it is resolved.
- **Ordering.** Within a stream, order is by `seq`. Across streams the platform implies no order. A reader that merges streams states its own rule, for example by `as_of`.
- **Live push.** None in this version. A later push follows the order gateway's policy: a bounded queue, `slow_consumer` on overflow, and resume by number from the store. It does not follow the market-data relay's policy, because a missing publication is a defect, not a fact about a feed.

## Refusals

| Code | Cause |
|---|---|
| `unpromoted_real_world` | a `real_world` record without a promoted status |
| `missing_provenance` | no producer, no input hashes, or no code hash |
| `hash_mismatch` | the `publication` hash does not match the canonical record |
| `as_of_violation` | `as_of` is later than the record's own write time, or missing |
| `unknown_units` | `units` absent or not in the registry |
| `bad_quantiles` | levels not increasing, or values not non-decreasing |
| `seq_violation` | `seq` is not the next number for its stream, and the write is not an exact repeat of an accepted publication |

## Example

```json
{
  "schema": "tinytrader.predictive/1",
  "publication": "0000000000000000000000000000000000000000000000000000000000000000",
  "seq": 1,
  "producer": { "name": "example-model", "version": "0.1.0" },
  "as_of": 1791998400000000000,
  "instrument": "EXAMPLE.OPT",
  "target": { "kind": "example_target", "params": { "horizon_minutes": 30 } },
  "units": "basis_points",
  "lane": "risk_neutral",
  "status": { "state": "unpromoted" },
  "quantiles": [
    { "level": 0.05, "value": -41.2 },
    { "level": 0.50, "value": 0.3 },
    { "level": 0.95, "value": 39.8 }
  ],
  "gap": null,
  "provenance": {
    "code_sha256": "1111111111111111111111111111111111111111111111111111111111111111",
    "input_sha256": ["2222222222222222222222222222222222222222222222222222222222222222"],
    "parent": null
  }
}
```

The names in the example are placeholders. Real instrument and target names come from the producer's declared vocabulary
and the dataset registry.

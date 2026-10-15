//! Option chains from Alpaca's option snapshots: each contract parsed from
//! its OCC symbol, and a summary per expiry (at-the-money IV, the straddle's
//! price as the implied move, and the implied distribution from the whole
//! chain: see `smile`).

use std::collections::BTreeMap;

use serde_json::{Value, json};

/// One option contract and its latest quote, IV and greeks.
#[derive(Clone, Debug, PartialEq)]
pub struct Contract {
    pub symbol: String,
    /// YYYY-MM-DD.
    pub expiry: String,
    pub call: bool,
    pub strike: f64,
    pub bid: Option<f64>,
    pub ask: Option<f64>,
    pub last: Option<f64>,
    pub iv: Option<f64>,
    pub delta: Option<f64>,
    pub gamma: Option<f64>,
    pub theta: Option<f64>,
    pub vega: Option<f64>,
}

impl Contract {
    /// Midpoint of a two-sided quote.
    #[must_use]
    pub fn mid(&self) -> Option<f64> {
        match (self.bid, self.ask) {
            (Some(b), Some(a)) if b > 0.0 && a >= b => Some((a + b) / 2.0),
            _ => None,
        }
    }

    #[must_use]
    pub fn to_json(&self) -> Value {
        json!({
            "symbol": self.symbol, "expiry": self.expiry, "type": if self.call { "call" } else { "put" },
            "strike": self.strike, "bid": self.bid, "ask": self.ask, "mid": self.mid(), "last": self.last,
            "iv": self.iv, "delta": self.delta, "gamma": self.gamma, "theta": self.theta, "vega": self.vega,
        })
    }
}

/// `AAPL261016C00250000` as (root, expiry, call, strike).
#[must_use]
pub fn occ(symbol: &str) -> Option<(String, String, bool, f64)> {
    let tail = symbol.len().checked_sub(15)?;
    let (root, rest) = symbol.split_at(tail);
    let (date, rest) = rest.split_at(6);
    let (kind, strike) = rest.split_at(1);
    let call = match kind {
        "C" => true,
        "P" => false,
        _ => return None,
    };
    if !date.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let strike = strike.parse::<u64>().ok()? as f64 / 1000.0;
    Some((
        root.to_owned(),
        format!("20{}-{}-{}", &date[0..2], &date[2..4], &date[4..6]),
        call,
        strike,
    ))
}

/// The contracts in one page of `/v1beta1/options/snapshots/{underlying}`.
#[must_use]
pub fn contracts(page: &Value) -> Vec<Contract> {
    let num = |v: &Value| v.as_f64().filter(|x| x.is_finite());
    page["snapshots"]
        .as_object()
        .into_iter()
        .flatten()
        .filter_map(|(sym, s)| {
            let (_, expiry, call, strike) = occ(sym)?;
            // Alpaca's `impliedVolatility` and `greeks` are ignored:
            // `pricing::price` sets ours from the quotes.
            Some(Contract {
                symbol: sym.clone(),
                expiry,
                call,
                strike,
                bid: num(&s["latestQuote"]["bp"]),
                ask: num(&s["latestQuote"]["ap"]),
                last: num(&s["latestTrade"]["p"]),
                iv: None,
                delta: None,
                gamma: None,
                theta: None,
                vega: None,
            })
        })
        .collect()
}

/// The underlying's price implied by put-call parity, for an underlying
/// with no quote of its own (an index such as XSP): on the nearest expiry
/// with three strikes quoted both ways, strike + call − put at the three
/// strikes where call and put are closest, their median. Over a day or two
/// the forward and the price differ by well under a basis point.
#[must_use]
pub fn parity_spot(contracts: &[Contract]) -> Option<f64> {
    // Expiry → strike (as bits) → (call mid, put mid).
    type Mids = BTreeMap<u64, (Option<f64>, Option<f64>)>;
    let mut by: BTreeMap<&str, Mids> = BTreeMap::new();
    for c in contracts {
        let at = by
            .entry(c.expiry.as_str())
            .or_default()
            .entry(c.strike.to_bits())
            .or_default();
        if c.call {
            at.0 = c.mid();
        } else {
            at.1 = c.mid();
        }
    }
    by.values().find_map(|strikes| {
        let mut pairs: Vec<(f64, f64)> = strikes
            .iter()
            .filter_map(|(k, q)| match *q {
                (Some(c), Some(p)) => Some(((c - p).abs(), f64::from_bits(*k) + c - p)),
                _ => None,
            })
            .collect();
        if pairs.len() < 3 {
            return None;
        }
        pairs.sort_by(|a, b| a.0.total_cmp(&b.0));
        let mut f: Vec<f64> = pairs[..3].iter().map(|x| x.1).collect();
        f.sort_by(f64::total_cmp);
        Some(f[1])
    })
}

/// Per expiry, from the strike nearest `spot`: the at-the-money IV (the
/// out-of-the-money side's there, against the forward: the call at or above
/// it, the put below; the other side when that one has none; contracts
/// priced by `pricing::price` first), and the straddle's mid as an implied move (in dollars and as a
/// fraction of `spot`), and the implied distribution with `years` (the time
/// to an expiry's close, in years) when it is given.
#[must_use]
pub fn expiries(
    contracts: &[Contract],
    spot: Option<f64>,
    years: &dyn Fn(&str) -> Option<f64>,
) -> Vec<Value> {
    let mut by: BTreeMap<&str, Vec<&Contract>> = BTreeMap::new();
    for c in contracts {
        by.entry(c.expiry.as_str()).or_default().push(c);
    }
    by.into_iter()
        .map(|(expiry, list)| {
            let atm = spot.and_then(|s| {
                list.iter()
                    .map(|c| c.strike)
                    .min_by(|a, b| (a - s).abs().total_cmp(&(b - s).abs()))
            });
            let at = |call: bool| {
                atm.and_then(|k| list.iter().find(|c| c.call == call && c.strike == k))
            };
            let (c, p) = (at(true), at(false));
            let fwd = years(expiry).and_then(|t| crate::pricing::forward(&list, t, spot));
            let call_side = match (atm, fwd) {
                (Some(k), Some(f)) => k >= f,
                _ => true,
            };
            let (otm, itm) = if call_side { (c, p) } else { (p, c) };
            let atm_iv = otm.and_then(|x| x.iv).or_else(|| itm.and_then(|x| x.iv));
            let straddle = match (c.and_then(|x| x.mid()), p.and_then(|x| x.mid())) {
                (Some(a), Some(b)) => Some(a + b),
                _ => None,
            };
            let distribution = match (spot, years(expiry)) {
                (Some(s), Some(t)) => crate::smile::distribution(&list, s, t, straddle),
                _ => Value::Null,
            };
            json!({
                "expiry": expiry, "contracts": list.len(), "atm_strike": atm, "distribution": distribution,
                "atm_iv": atm_iv,
                "straddle": straddle,
                "implied_move": straddle.zip(spot).map(|(x, s)| x / s),
            })
        })
        .collect()
}

/// An underlying's listed contracts as the chain's index: its expiries from
/// `today` on, each with its OCC root and the call and put strikes, from
/// pages of Alpaca's `/v2/options/contracts`. What exists, not what it costs:
/// it changes once a day, so it's kept a day.
#[must_use]
pub fn index(underlying: &str, today: &str, pages: &[Value]) -> Value {
    // Expiry → (root, call strikes, put strikes).
    type Listed = (String, Vec<f64>, Vec<f64>);
    let mut by: BTreeMap<String, Listed> = BTreeMap::new();
    for c in pages
        .iter()
        .flat_map(|v| v["option_contracts"].as_array().into_iter().flatten())
    {
        let (Some(expiry), Some(strike)) = (
            c["expiration_date"].as_str().filter(|e| *e >= today),
            c["strike_price"]
                .as_str()
                .and_then(|x| x.parse::<f64>().ok())
                .or_else(|| c["strike_price"].as_f64()),
        ) else {
            continue;
        };
        let e = by.entry(expiry.to_owned()).or_insert_with(|| {
            (
                c["root_symbol"].as_str().unwrap_or(underlying).to_owned(),
                Vec::new(),
                Vec::new(),
            )
        });
        if c["type"] == "call" {
            e.1.push(strike)
        } else {
            e.2.push(strike)
        }
    }
    let tidy = |mut v: Vec<f64>| {
        v.sort_by(f64::total_cmp);
        v.dedup();
        v
    };
    json!({
        "underlying": underlying,
        "date": today,
        "expiries": by.into_iter().map(|(expiry, (root, calls, puts))| json!({
            "expiry": expiry, "root": root, "calls": tidy(calls), "puts": tidy(puts),
        })).collect::<Vec<_>>(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_occ_symbols() {
        assert_eq!(
            occ("AAPL261016C00250000"),
            Some(("AAPL".into(), "2026-10-16".into(), true, 250.0))
        );
        assert_eq!(
            occ("SPXW261016P05742500"),
            Some(("SPXW".into(), "2026-10-16".into(), false, 5742.5))
        );
        assert_eq!(occ("AAPL"), None);
        assert_eq!(occ("AAPL261016X00250000"), None);
    }

    #[test]
    fn prices_the_underlying_by_parity() {
        let q = |sym: &str, bid: f64, ask: f64| {
            (
                sym.to_owned(),
                json!({ "latestQuote": { "bp": bid, "ap": ask } }),
            )
        };
        let page = json!({ "snapshots": serde_json::Map::from_iter([
            // The nearest expiry has only one pair, so the next one is used.
            q("X261015C00100000", 1.0, 1.2),
            q("X261015P00100000", 1.0, 1.2),
            q("X261016C00099000", 2.5, 2.7),
            q("X261016P00099000", 1.3, 1.5),
            q("X261016C00100000", 1.9, 2.1),
            q("X261016P00100000", 1.6, 1.8),
            q("X261016C00101000", 1.4, 1.6),
            q("X261016P00101000", 2.1, 2.3),
            q("X261016C00110000", 0.1, 0.2),
            q("X261016P00110000", 9.2, 9.4),
            // One-sided: ignored.
            q("X261016C00102000", 0.0, 1.0),
        ]) });
        // 99 + 1.2 = 100.2, 100 + 0.3, 101 − 0.7 = 100.3: the median, 100.3.
        let s = parity_spot(&contracts(&page)).unwrap();
        assert!((s - 100.3).abs() < 1e-9, "{s}");
        assert_eq!(parity_spot(&[]), None);
    }

    #[test]
    fn summarises_expiries_at_the_money() {
        let page = json!({ "snapshots": {
            "X261016C00100000": { "latestQuote": { "bp": 2.0, "ap": 2.2 }, "impliedVolatility": 0.5, "greeks": { "delta": 0.52 } },
            "X261016P00100000": { "latestQuote": { "bp": 1.8, "ap": 2.0 }, "impliedVolatility": 0.54 },
            "X261016C00110000": { "latestQuote": { "bp": 0.3, "ap": 0.4 } },
            "X261023C00100000": { "latestQuote": { "bp": 3.0, "ap": 3.2 }, "impliedVolatility": 0.4 },
        }});
        let mut cs = contracts(&page);
        assert_eq!(cs.len(), 4);
        // Alpaca's IVs and greeks are dropped on reading.
        assert!(cs.iter().all(|c| c.iv.is_none() && c.delta.is_none()));
        let t = 30.0 / 365.0;
        crate::pricing::price(&mut cs, Some(101.0), &|_| Some(t));
        let e = expiries(&cs, Some(101.0), &|_| Some(t));
        assert_eq!(e.len(), 2);
        assert_eq!(e[0]["atm_strike"], 100.0);
        // The strike is below the forward (101: one pair, so the price), so the put's.
        let put = crate::pricing::greeks(1.9, 101.0, 100.0, t, false, 101.0).unwrap();
        assert!((e[0]["atm_iv"].as_f64().unwrap() - put.iv).abs() < 1e-9);
        assert!((put.iv - 0.5).abs() > 0.01);
        assert!((e[0]["straddle"].as_f64().unwrap() - 4.0).abs() < 1e-9);
        assert!((e[0]["implied_move"].as_f64().unwrap() - 4.0 / 101.0).abs() < 1e-9);
        // Only a call at the money: an IV, but no straddle.
        assert!(e[1]["straddle"].is_null() && e[1]["atm_iv"].as_f64().is_some());
    }

    #[test]
    fn indexes_listed_contracts() {
        let page = json!({ "option_contracts": [
            { "symbol": "SPXW261016P05742500", "root_symbol": "SPXW", "expiration_date": "2026-10-16", "type": "put", "strike_price": "5742.5" },
            { "symbol": "SPXW261016C05750000", "root_symbol": "SPXW", "expiration_date": "2026-10-16", "type": "call", "strike_price": "5750" },
            { "symbol": "SPXW261016P05700000", "root_symbol": "SPXW", "expiration_date": "2026-10-16", "type": "put", "strike_price": "5700" },
            { "symbol": "SPXW261014P05700000", "root_symbol": "SPXW", "expiration_date": "2026-10-14", "type": "put", "strike_price": "5700" },
        ]});
        let v = index("SPX", "2026-10-15", &[page]);
        assert_eq!(
            v["expiries"],
            json!([{ "expiry": "2026-10-16", "root": "SPXW", "calls": [5750.0], "puts": [5700.0, 5742.5] }])
        );
    }
}

//! Option chains from Alpaca's option snapshots: each contract parsed from
//! its OCC symbol, and a summary per expiry (at-the-money IV and the
//! straddle's price as the implied move).

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
            let g = &s["greeks"];
            Some(Contract {
                symbol: sym.clone(),
                expiry,
                call,
                strike,
                bid: num(&s["latestQuote"]["bp"]),
                ask: num(&s["latestQuote"]["ap"]),
                last: num(&s["latestTrade"]["p"]),
                iv: num(&s["impliedVolatility"]),
                delta: num(&g["delta"]),
                gamma: num(&g["gamma"]),
                theta: num(&g["theta"]),
                vega: num(&g["vega"]),
            })
        })
        .collect()
}

/// Per expiry, from the strike nearest `spot`: the call and put IVs and their
/// average, and the straddle's mid as an implied move (in dollars and as a
/// fraction of `spot`).
#[must_use]
pub fn expiries(contracts: &[Contract], spot: Option<f64>) -> Vec<Value> {
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
            let ivs: Vec<f64> = [c, p].into_iter().flatten().filter_map(|x| x.iv).collect();
            let straddle = match (c.and_then(|x| x.mid()), p.and_then(|x| x.mid())) {
                (Some(a), Some(b)) => Some(a + b),
                _ => None,
            };
            json!({
                "expiry": expiry, "contracts": list.len(), "atm_strike": atm,
                "atm_iv": (!ivs.is_empty()).then(|| ivs.iter().sum::<f64>() / ivs.len() as f64),
                "straddle": straddle,
                "implied_move": straddle.zip(spot).map(|(x, s)| x / s),
            })
        })
        .collect()
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
    fn summarises_expiries_at_the_money() {
        let page = json!({ "snapshots": {
            "X261016C00100000": { "latestQuote": { "bp": 2.0, "ap": 2.2 }, "impliedVolatility": 0.5, "greeks": { "delta": 0.52 } },
            "X261016P00100000": { "latestQuote": { "bp": 1.8, "ap": 2.0 }, "impliedVolatility": 0.54 },
            "X261016C00110000": { "latestQuote": { "bp": 0.3, "ap": 0.4 } },
            "X261023C00100000": { "latestQuote": { "bp": 3.0, "ap": 3.2 }, "impliedVolatility": 0.4 },
        }});
        let cs = contracts(&page);
        assert_eq!(cs.len(), 4);
        let e = expiries(&cs, Some(101.0));
        assert_eq!(e.len(), 2);
        assert_eq!(e[0]["atm_strike"], 100.0);
        assert!((e[0]["atm_iv"].as_f64().unwrap() - 0.52).abs() < 1e-9);
        assert!((e[0]["straddle"].as_f64().unwrap() - 4.0).abs() < 1e-9);
        assert!((e[0]["implied_move"].as_f64().unwrap() - 4.0 / 101.0).abs() < 1e-9);
        // Only a call at the money: an IV, but no straddle.
        assert!(e[1]["straddle"].is_null() && e[1]["atm_iv"].as_f64().is_some());
    }
}

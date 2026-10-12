//! Real option quotes from the OPRA relays (Databento's consolidated best bid
//! and offer, `cmbp-1`) for the underlyings they carry, in place of Alpaca's
//! indicative feed. `ALPACA_DATA_QUOTES` names them:
//! `SPY=http://relay-opra-spy:9110;XSP=http://relay-opra-xsp:9110`.
//!
//! A chain for one of those underlyings asks its relay's `/quotes` for the
//! expiries in range and takes each contract's bid and ask from it before
//! pricing, while the relay's feed is live (a record within `LIVE`). A
//! contract the relay has no quote for, or any contract when the feed isn't
//! live (outside its hours, or down), keeps Alpaca's indicative quote.

use std::collections::{HashMap, HashSet};

use serde_json::Value;

use crate::chain::Contract;

/// The feed counts as live if its last record is at most this old.
pub const LIVE: f64 = 120.0;

/// Relay base URLs by underlying, from `SPY=http://...;XSP=http://...`.
#[must_use]
pub fn parse(text: &str) -> HashMap<String, String> {
    text.split(';')
        .filter_map(|e| {
            let (u, url) = e.trim().split_once('=')?;
            let (u, url) = (u.trim().to_uppercase(), url.trim().trim_end_matches('/'));
            (!u.is_empty() && !url.is_empty()).then(|| (u, url.to_owned()))
        })
        .collect()
}

/// The relay's quotes for an underlying's expiries `from` to `to`.
pub async fn fetch(
    http: &reqwest::Client,
    base: &str,
    underlying: &str,
    from: &str,
    to: &str,
) -> Result<Value, String> {
    let r = http
        .get(format!("{base}/quotes"))
        .query(&[("underlying", underlying), ("from", from), ("to", to)])
        .send()
        .await
        .map_err(|e| format!("OPRA relay for {underlying}: {e}"))?;
    if !r.status().is_success() {
        return Err(format!(
            "OPRA relay for {underlying} answered {}",
            r.status()
        ));
    }
    r.json()
        .await
        .map_err(|e| format!("OPRA relay for {underlying}: unreadable: {e}"))
}

/// Replace contracts' bid and ask with the relay's, when its feed is live at
/// `now` (Unix seconds); returns the symbols that took an OPRA quote. A side
/// the relay shows empty is a zero bid, or no ask.
pub fn overlay(contracts: &mut [Contract], answer: &Value, now: f64) -> HashSet<String> {
    let mut used = HashSet::new();
    let live = answer["last_record"]
        .as_f64()
        .is_some_and(|t| now - t <= LIVE);
    if !live {
        return used;
    }
    let quotes = &answer["quotes"];
    for c in contracts.iter_mut() {
        let q = &quotes[c.symbol.as_str()];
        if !q.is_object() {
            continue;
        }
        let (bid, ask) = (q["bid"].as_f64(), q["ask"].as_f64());
        if bid.is_none() && ask.is_none() {
            continue;
        }
        c.bid = Some(bid.unwrap_or(0.0));
        c.ask = ask;
        used.insert(c.symbol.clone());
    }
    used
}

/// `opra`, `indicative` or `mixed`, from how many contracts took OPRA quotes.
#[must_use]
pub fn label(opra: usize, total: usize) -> &'static str {
    match opra {
        0 => "indicative",
        n if n == total => "opra",
        _ => "mixed",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn contract(symbol: &str, bid: f64, ask: f64) -> Contract {
        let (_, expiry, call, strike) = crate::chain::occ(symbol).unwrap();
        Contract {
            symbol: symbol.into(),
            expiry,
            call,
            strike,
            bid: Some(bid),
            ask: Some(ask),
            last: None,
            iv: None,
            delta: None,
            gamma: None,
            theta: None,
            vega: None,
        }
    }

    #[test]
    fn parses_relays() {
        let r = parse("spy=http://relay-opra-spy:9110/; XSP=http://x:9110;bad;=y");
        assert_eq!(r.len(), 2);
        assert_eq!(r["SPY"], "http://relay-opra-spy:9110");
    }

    #[test]
    fn takes_live_quotes_and_keeps_the_rest() {
        let mut cs = vec![
            contract("SPY261016C00775000", 1.10, 1.40),
            contract("SPY261016P00700000", 0.20, 0.35),
            contract("SPY261016P00600000", 0.00, 0.05),
        ];
        let answer = json!({ "last_record": 1000.0, "quotes": {
            "SPY261016C00775000": { "bid": 1.21, "ask": 1.24, "ts": 400.0 },
            "SPY261016P00600000": { "bid": null, "ask": 0.01, "ts": 990.0 },
        } });
        let used = overlay(&mut cs, &answer, 1050.0);
        assert_eq!(used.len(), 2);
        assert_eq!((cs[0].bid, cs[0].ask), (Some(1.21), Some(1.24)));
        // No relay quote: Alpaca's stays.
        assert_eq!((cs[1].bid, cs[1].ask), (Some(0.20), Some(0.35)));
        // An empty bid is a zero bid.
        assert_eq!((cs[2].bid, cs[2].ask), (Some(0.0), Some(0.01)));
        assert_eq!(label(used.len(), cs.len()), "mixed");
    }

    #[test]
    fn a_feed_that_isnt_live_changes_nothing() {
        let mut cs = vec![contract("SPY261016C00775000", 1.10, 1.40)];
        let answer = json!({ "last_record": 1000.0, "quotes": { "SPY261016C00775000": { "bid": 1.21, "ask": 1.24 } } });
        assert!(overlay(&mut cs, &answer, 1000.0 + LIVE + 1.0).is_empty());
        assert!(overlay(&mut cs, &json!({ "last_record": null, "quotes": {} }), 1.0).is_empty());
        assert_eq!(cs[0].bid, Some(1.10));
        assert_eq!(label(0, 1), "indicative");
        assert_eq!(label(1, 1), "opra");
    }
}

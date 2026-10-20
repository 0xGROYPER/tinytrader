//! Buying only what's missing (decision 0018, section 5): for each day of a
//! request on an option parent (`SPY.OPT`), the contracts wanted (that day's
//! definitions, filtered by a rule) less those the store holds and those in
//! jobs on Databento's list, bought as one contract-list request per day.

use std::collections::{BTreeMap, BTreeSet};

use dbn::decode::{DbnMetadata as _, DecodeRecordRef as _};
use serde::Deserialize;
use serde_json::{Value, json};

/// Databento's limit on the symbols in one request.
pub const MAX_SYMBOLS: usize = 2_000;

/// Which of a day's contracts are wanted. Every field is optional; none at
/// all is the whole chain.
#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
pub struct Rule {
    /// The day's first N expiries, counting that day's own (1: same-day
    /// expiry; 2: same day and the next).
    pub expiries: Option<usize>,
    /// Expiring at most this many calendar days after the day.
    pub dte_max: Option<i64>,
    /// Strikes within this range (dollars), inclusive.
    pub strike_min: Option<f64>,
    pub strike_max: Option<f64>,
}

/// One contract from a definition day: its symbol, expiry date and strike.
#[derive(Clone, Debug, PartialEq)]
pub struct Contract {
    pub symbol: String,
    pub expiry: time::Date,
    pub strike: f64,
}

/// The contracts a definition day file (zstd DBN) lists.
///
/// # Errors
/// If it isn't a readable definition file.
pub fn contracts(bytes: &[u8]) -> Result<Vec<Contract>, String> {
    let mut d = dbn::decode::dbn::Decoder::with_zstd(bytes)
        .map_err(|e| format!("not a readable DBN file: {e}"))?;
    let _ = d.metadata();
    let mut seen = BTreeMap::new();
    while let Some(r) = d.decode_record_ref().map_err(|e| e.to_string())? {
        let Some(def) = r.get::<dbn::InstrumentDefMsg>() else {
            continue;
        };
        let Ok(symbol) = def.raw_symbol() else {
            continue;
        };
        let Ok(at) = time::OffsetDateTime::from_unix_timestamp_nanos(i128::from(def.expiration))
        else {
            continue;
        };
        if def.expiration == dbn::UNDEF_TIMESTAMP || def.strike_price == dbn::UNDEF_PRICE {
            continue;
        }
        #[expect(
            clippy::cast_precision_loss,
            reason = "strikes are far below 2^52 nano-dollars"
        )]
        let strike = def.strike_price as f64 / 1e9;
        seen.insert(
            symbol.to_owned(),
            Contract {
                symbol: symbol.to_owned(),
                expiry: at.date(),
                strike,
            },
        );
    }
    Ok(seen.into_values().collect())
}

/// The contracts `rule` wants on `day`, from that day's definitions.
#[must_use]
pub fn wanted(defs: &[Contract], day: time::Date, rule: &Rule) -> BTreeSet<String> {
    let live: Vec<&Contract> = defs.iter().filter(|c| c.expiry >= day).collect();
    let expiries: BTreeSet<time::Date> = live.iter().map(|c| c.expiry).collect();
    let cutoff = rule
        .expiries
        .and_then(|n| expiries.iter().nth(n.saturating_sub(1)).copied());
    live.into_iter()
        .filter(|c| rule.expiries.is_none() || cutoff.is_none_or(|x| c.expiry <= x))
        .filter(|_| rule.expiries != Some(0))
        .filter(|c| {
            rule.dte_max
                .is_none_or(|m| (c.expiry - day).whole_days() <= m)
        })
        .filter(|c| rule.strike_min.is_none_or(|m| c.strike >= m))
        .filter(|c| rule.strike_max.is_none_or(|m| c.strike <= m))
        .map(|c| c.symbol.clone())
        .collect()
}

/// What a day needs, given what's wanted, the store's coverage of that day
/// (from `GET /coverage`: none, complete, or partial with contracts), and
/// the contract lists of jobs on Databento's list for that day (`None` in the
/// list: a job for the whole parent).
#[must_use]
pub fn to_buy(
    wanted: &BTreeSet<String>,
    held: Option<&Value>,
    pending: &[Option<BTreeSet<String>>],
) -> (BTreeSet<String>, usize, usize) {
    let all = wanted.len();
    if held.is_some_and(|h| h["kind"] == "complete") || pending.iter().any(Option::is_none) {
        return (BTreeSet::new(), all, 0);
    }
    let held: BTreeSet<&str> = held
        .map(|h| {
            h["coverage"]["symbols"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .collect()
        })
        .unwrap_or_default();
    let mut rest: BTreeSet<String> = wanted
        .iter()
        .filter(|s| !held.contains(s.as_str()))
        .cloned()
        .collect();
    let after_held = rest.len();
    for p in pending.iter().flatten() {
        rest.retain(|s| !p.contains(s));
    }
    let skipped_held = all - after_held;
    let skipped_pending = after_held - rest.len();
    (rest, skipped_held, skipped_pending)
}

/// The option parent (`SPY.OPT`) whose contracts these all are, if they are
/// all OCC symbols of one root.
#[must_use]
pub fn parent_of(symbols: &[String]) -> Option<String> {
    let root = |s: &str| (s.len() == 21).then(|| s[..6].trim_end().to_owned());
    let first = root(symbols.first()?)?;
    symbols
        .iter()
        .all(|s| root(s).as_deref() == Some(first.as_str()))
        .then(|| format!("{first}.OPT"))
}

/// A day's line in a quote.
#[derive(Clone, Debug)]
pub struct Day {
    pub day: time::Date,
    pub symbols: Vec<String>,
    pub cost_usd: f64,
    pub bytes: u64,
    pub skipped_held: usize,
    pub skipped_pending: usize,
}

impl Day {
    #[must_use]
    pub fn to_json(&self) -> Value {
        json!({
            "day": self.day.to_string(), "contracts": self.symbols.len(), "cost_usd": self.cost_usd,
            "billable_bytes": self.bytes, "skipped_held": self.skipped_held, "skipped_pending": self.skipped_pending,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(s: &str) -> time::Date {
        time::Date::parse(s, time::macros::format_description!("[year]-[month]-[day]")).unwrap()
    }

    fn c(symbol: &str, expiry: &str, strike: f64) -> Contract {
        Contract {
            symbol: symbol.into(),
            expiry: d(expiry),
            strike,
        }
    }

    fn defs() -> Vec<Contract> {
        vec![
            c("SPY   261002C00728000", "2026-10-02", 728.0),
            c("SPY   261002C00760000", "2026-10-02", 760.0),
            c("SPY   261005C00728000", "2026-10-05", 728.0),
            c("SPY   261016C00728000", "2026-10-16", 728.0),
            c("SPY   261001C00728000", "2026-10-01", 728.0),
        ]
    }

    #[test]
    fn a_rule_picks_expiries_and_strikes() {
        let day = d("2026-10-02");
        let all = wanted(&defs(), day, &Rule::default());
        assert_eq!(all.len(), 4, "expired contracts are never wanted");
        let two = wanted(
            &defs(),
            day,
            &Rule {
                expiries: Some(2),
                ..Rule::default()
            },
        );
        assert_eq!(two.len(), 3, "same day and the next expiry (Monday)");
        let band = wanted(
            &defs(),
            day,
            &Rule {
                expiries: Some(1),
                strike_min: Some(700.0),
                strike_max: Some(740.0),
                ..Rule::default()
            },
        );
        assert_eq!(band, BTreeSet::from(["SPY   261002C00728000".to_owned()]));
        let week = wanted(
            &defs(),
            day,
            &Rule {
                dte_max: Some(7),
                ..Rule::default()
            },
        );
        assert_eq!(week.len(), 3);
    }

    #[test]
    fn subtracts_held_and_pending_contracts() {
        let w: BTreeSet<String> = ["A", "B", "C", "D"].map(String::from).into();
        let partial = json!({ "kind": "partial", "coverage": { "symbols": ["A", "B"] } });
        let (rest, h, p) = to_buy(&w, Some(&partial), &[Some(["C".to_owned()].into())]);
        assert_eq!((rest, h, p), (["D".to_owned()].into(), 2, 1));
        let complete = json!({ "kind": "complete" });
        assert!(to_buy(&w, Some(&complete), &[]).0.is_empty());
        assert!(
            to_buy(&w, None, &[None]).0.is_empty(),
            "a whole-parent job pending covers the day"
        );
        assert_eq!(to_buy(&w, None, &[]).0.len(), 4);
    }

    #[test]
    fn finds_the_parent_of_a_contract_list() {
        let s = |v: &[&str]| v.iter().map(|x| (*x).to_owned()).collect::<Vec<_>>();
        assert_eq!(
            parent_of(&s(&["SPY   261002C00728000", "SPY   261002P00700000"])).as_deref(),
            Some("SPY.OPT")
        );
        assert_eq!(
            parent_of(&s(&["SPY   261002C00728000", "QQQ   261002P00500000"])),
            None
        );
        assert_eq!(parent_of(&s(&["SPY.OPT"])), None);
    }
}

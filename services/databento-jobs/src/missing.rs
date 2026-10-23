//! Buying only what's missing (decision 0018, section 5): for each day of a
//! request on an option parent (`SPY.OPT`), the contracts wanted (that day's
//! definitions, filtered by a rule) less what's had already (`had`), bought
//! as contract-list requests per day.

use std::collections::{BTreeMap, BTreeSet};

use dbn::decode::{DbnMetadata as _, DecodeRecordRef as _};
use serde::{Deserialize, Serialize};
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

/// Wanted hours of each day, in a named time zone's wall clock. Only
/// `America/New_York` (daylight saving from the second Sunday in March to the
/// first Sunday in November).
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct Hours {
    /// `HH:MM`.
    pub start: String,
    /// `HH:MM`, exclusive.
    pub end: String,
    #[serde(default = "new_york")]
    pub tz: String,
}

fn new_york() -> String {
    "America/New_York".into()
}

fn hm(s: &str) -> Option<(i64, i64)> {
    let (h, m) = s.trim().split_once(':')?;
    let (h, m) = (h.parse().ok()?, m.parse().ok()?);
    ((0..=24).contains(&h) && (0..60).contains(&m) && (h, m) <= (24, 0)).then_some((h, m))
}

impl Hours {
    /// # Errors
    /// If the zone isn't New York's, or the times aren't `HH:MM`, start
    /// before end.
    pub fn check(&self) -> Result<(), String> {
        if self.tz != "America/New_York" {
            return Err(format!(
                "hours in {} aren't supported; use America/New_York",
                self.tz
            ));
        }
        match (hm(&self.start), hm(&self.end)) {
            (Some(a), Some(b)) if a < b => Ok(()),
            _ => Err(format!(
                "hours {}..{} aren't HH:MM with the start before the end",
                self.start, self.end
            )),
        }
    }

    /// The hours on `day`, in UTC.
    #[must_use]
    pub fn window(&self, day: time::Date) -> crate::had::Window {
        let at = |s: &str| {
            let (h, m) = hm(s).unwrap_or((0, 0));
            tinytrader_schedule::ny_to_unix(
                i64::from(day.year()),
                i64::from(u8::from(day.month())),
                i64::from(day.day()),
                h,
                m,
            ) * 1_000_000_000
        };
        (at(&self.start), at(&self.end))
    }
}

/// A request in a quote: the whole symbol over a run of days (or one day's
/// hours), or one day's missing contracts over the hours they still need.
#[derive(Clone, Debug)]
pub struct Part {
    pub day: time::Date,
    /// The last day, inclusive.
    pub last: time::Date,
    pub window: crate::had::Window,
    /// The whole symbol (a parent, or a raw symbol), not a contract list.
    pub whole: bool,
    pub symbols: Vec<String>,
    pub stype: dbn::SType,
    pub cost_usd: f64,
    pub bytes: u64,
    pub skipped_held: usize,
    pub skipped_pending: usize,
}

impl Part {
    /// The same request (its price aside).
    #[must_use]
    pub fn same(&self, o: &Self) -> bool {
        (self.day, self.last, self.window, self.whole, self.stype)
            == (o.day, o.last, o.window, o.whole, o.stype)
            && self.symbols == o.symbols
    }

    #[must_use]
    pub fn to_json(&self) -> Value {
        use crate::had::{day_window, stamp};
        let mut v = json!({
            "kind": if self.whole { "whole" } else { "contracts" },
            "first_day": self.day.to_string(), "last_day": self.last.to_string(),
            "window": [stamp(self.window.0), stamp(self.window.1)],
            "hours": self.window != (day_window(self.day).0, day_window(self.last).1),
            "cost_usd": self.cost_usd, "billable_bytes": self.bytes,
        });
        if self.whole {
            v["symbol"] = json!(self.symbols.first());
        } else {
            v["contracts"] = json!(self.symbols.len());
            v["skipped_held"] = json!(self.skipped_held);
            v["skipped_pending"] = json!(self.skipped_pending);
        }
        v
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
    fn hours_follow_new_york_daylight_saving() {
        let rth = Hours {
            start: "09:30".into(),
            end: "16:01".into(),
            tz: new_york(),
        };
        assert!(rth.check().is_ok());
        let at = |day: &str| {
            let (a, b) = rth.window(d(day));
            let t = |ns: i64| crate::had::stamp(ns);
            (t(a), t(b))
        };
        let utc = |a: &str, b: &str| (a.to_owned(), b.to_owned());
        // Daylight saving starts on the second Sunday of March (2026-03-08).
        assert_eq!(
            at("2026-03-06"),
            utc(
                "2026-03-06T14:30:00.000000000Z",
                "2026-03-06T21:01:00.000000000Z"
            )
        );
        assert_eq!(
            at("2026-03-08"),
            utc(
                "2026-03-08T13:30:00.000000000Z",
                "2026-03-08T20:01:00.000000000Z"
            )
        );
        assert_eq!(
            at("2026-03-09"),
            utc(
                "2026-03-09T13:30:00.000000000Z",
                "2026-03-09T20:01:00.000000000Z"
            )
        );
        // And ends on the first Sunday of November (2026-11-01).
        assert_eq!(
            at("2026-10-30"),
            utc(
                "2026-10-30T13:30:00.000000000Z",
                "2026-10-30T20:01:00.000000000Z"
            )
        );
        assert_eq!(
            at("2026-11-01"),
            utc(
                "2026-11-01T14:30:00.000000000Z",
                "2026-11-01T21:01:00.000000000Z"
            )
        );
        assert_eq!(
            at("2026-11-02"),
            utc(
                "2026-11-02T14:30:00.000000000Z",
                "2026-11-02T21:01:00.000000000Z"
            )
        );
        // 2022: March 13 and November 6.
        assert_eq!(at("2022-03-11").0, "2022-03-11T14:30:00.000000000Z");
        assert_eq!(at("2022-03-14").0, "2022-03-14T13:30:00.000000000Z");
        assert_eq!(at("2022-11-04").0, "2022-11-04T13:30:00.000000000Z");
        assert_eq!(at("2022-11-07").0, "2022-11-07T14:30:00.000000000Z");
        for (s, e, tz) in [
            ("16:00", "09:30", "America/New_York"),
            ("9h", "10:00", "America/New_York"),
            ("09:30", "16:00", "Europe/London"),
        ] {
            let h = Hours {
                start: s.into(),
                end: e.into(),
                tz: tz.into(),
            };
            assert!(h.check().is_err(), "{s} {e} {tz}");
        }
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

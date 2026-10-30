//! A venue's own rules, from its profile (`venue_rules`), as a broker applies them:
//!
//! - **No new same-day openings after a time** (`no_same_day_opening_after`, such as
//!   `"15:30"`, Alpaca's): an order opening an option that expires today is
//!   refused after it (`sim_same_day_opening`).
//! - **Same-day short options liquidated at a time** (`liquidate_same_day_shorts_at`,
//!   such as `"15:45"`, Alpaca's for accounts that can't cover exercise or
//!   assignment): every short option position in a physically settled root
//!   expiring that day is bought back at its ask, as a broker order the account's
//!   clients see as a `foreign` order with its fill; working orders on those
//!   contracts are canceled first. A short without an ask stays, to settle at the
//!   close.
//! - **Buying power** (`buying_power = "defined_risk"`): an opening order is refused
//!   (`sim_insufficient_buying_power`), never resized, when cash, less the order's
//!   premium, less the worst loss at expiry of each expiry's option positions with
//!   the order filled, would be negative. A position whose loss has no bound (more
//!   short calls than long) is refused outright. Shares are bought with cash. It
//!   approximates a broker's options buying power for defined-risk spreads, and
//!   does not model margin on naked positions.
use super::{CENT, SimBroker};
use crate::intent::Intent;
use crate::safety::{et_day, et_ns, hhmm};
use std::collections::BTreeMap;

/// Option legs `(call, strike, signed quantity)` by root and expiry.
type Groups = BTreeMap<(String, i64), Vec<(bool, i64, i64)>>;

/// The rules of a profile.
#[derive(Clone, Debug, Default)]
pub(crate) struct VenueRules {
    /// Minutes after midnight, ET.
    pub(crate) no_same_day_opening_after: Option<i64>,
    pub(crate) liquidate_at: Option<i64>,
    pub(crate) defined_risk: bool,
}

impl VenueRules {
    pub(crate) fn parse_time(s: &str) -> Option<i64> {
        hhmm(s)
    }
}

/// The worst payoff at expiry (nanodollars, ≤ 0 is a loss) of option positions in
/// one expiry, or `None` when it has no bound.
fn worst(legs: &[(bool, i64, i64)]) -> Option<i128> {
    // Calls whose net quantity is short lose without bound as the price rises.
    let calls: i64 = legs.iter().filter(|l| l.0).map(|l| l.2).sum();
    if calls < 0 {
        return None;
    }
    let payoff = |s: i64| -> i128 {
        legs.iter()
            .map(|(call, k, q)| {
                let v = if *call {
                    (s - k).max(0)
                } else {
                    (k - s).max(0)
                };
                i128::from(*q) * 100 * i128::from(v)
            })
            .sum()
    };
    // Piecewise linear between strikes: the worst is at zero or a strike.
    let mut points: Vec<i64> = legs.iter().map(|l| l.1).collect();
    points.push(0);
    points.iter().map(|s| payoff(*s)).min().or(Some(0))
}

impl SimBroker {
    /// Why the venue's rules refuse `intent` now, if they do: a code and a message.
    pub(super) fn refuse(&self, intent: &Intent) -> Option<(&'static str, String)> {
        let r = &self.s.venue;
        let today = et_day(self.now);
        if let Some(after) = r.no_same_day_opening_after {
            let late = self.now >= et_ns(today, after);
            // Its last trading day: the session before expiry for an AM-settled root.
            let same_day = intent
                .legs
                .iter()
                .any(|l| l.open && self.last_trading_day(&l.symbol) == Some(today));
            if late && same_day {
                return Some((
                    "sim_same_day_opening",
                    "options expiring today can't be opened this late".into(),
                ));
            }
        }
        if r.defined_risk && intent.legs.iter().any(|l| l.open) {
            return self.short_of_buying_power(intent);
        }
        None
    }

    /// The defined-risk buying power check of an opening order.
    fn short_of_buying_power(&self, intent: &Intent) -> Option<(&'static str, String)> {
        let qty = i64::from(intent.qty);
        // The order's premium: a debit is paid, a credit received.
        let signed = if intent.combo || intent.legs[0].buy {
            intent.limit_cents
        } else {
            -intent.limit_cents
        };
        let multiplier = if intent.legs.iter().all(|l| l.equity) {
            1
        } else {
            100
        };
        let premium = i128::from(signed) * i128::from(CENT) * i128::from(qty) * multiplier;
        // Option positions with the order filled, by root and expiry.
        let extra: Vec<(String, i64)> = intent
            .legs
            .iter()
            .map(|l| {
                let sign = if l.buy { 1 } else { -1 };
                (l.symbol.clone(), sign * qty * i64::from(l.ratio))
            })
            .collect();
        let required = match self.required(&extra) {
            Ok(r) => r,
            Err(root) => {
                return Some((
                    "sim_insufficient_buying_power",
                    format!("{root}: a position with no bound on its loss"),
                ));
            }
        };
        let left = self.cash - premium - required;
        (left < 0).then(|| {
            (
                "sim_insufficient_buying_power",
                format!("needs {} more", super::fmt_nanos(-left)),
            )
        })
    }

    /// The worst loss at expiry of the option positions, with `extra` added, summed
    /// over expiries; the root of a position with no bound, as an error.
    fn required(&self, extra: &[(String, i64)]) -> Result<i128, String> {
        let mut groups: Groups = BTreeMap::new();
        let all = self
            .pos
            .iter()
            .map(|(s, p)| (s.as_str(), p.0))
            .chain(extra.iter().map(|(s, q)| (s.as_str(), *q)));
        for (symbol, q) in all {
            if let (Some((root, call, strike)), Some((_, exp))) =
                (super::expiry::contract(symbol), crate::intent::occ(symbol))
            {
                groups
                    .entry((root.to_owned(), exp))
                    .or_default()
                    .push((call, strike, q));
            }
        }
        let mut required = 0_i128;
        for ((root, _), legs) in &groups {
            required += (-worst(legs).ok_or_else(|| root.clone())?).max(0);
        }
        Ok(required)
    }

    /// Cash less the worst expiry loss of the option positions held.
    pub(super) fn buying_power(&self) -> i128 {
        self.required(&[]).map_or(0, |r| (self.cash - r).max(0))
    }

    /// The rules, for `pins`.
    pub(super) fn rules_pin(&self) -> String {
        let r = &self.s.venue;
        let t = |m: Option<i64>| m.map(|m| format!("{:02}:{:02}", m / 60, m % 60));
        crate::protocol::Obj::new()
            .opt_s(
                "no_same_day_opening_after",
                t(r.no_same_day_opening_after).as_deref(),
            )
            .opt_s("liquidate_same_day_shorts_at", t(r.liquidate_at).as_deref())
            .s(
                "buying_power",
                if r.defined_risk {
                    "defined_risk"
                } else {
                    "cash"
                },
            )
            .done()
    }

    /// The next time the liquidation rule runs, on or after ET day `day`.
    pub(super) fn next_liquidation(&self, day: i64) -> Option<u64> {
        let at = self.s.venue.liquidate_at?;
        (day..day + 14)
            .filter(|d| self.session(*d).is_some())
            .map(|d| et_ns(d, at))
            .find(|t| *t >= self.now)
    }

    /// Buys back every short option expiring today in a physically settled root, at
    /// its ask, as broker orders; working orders on them are canceled first.
    pub(super) fn liquidate(&mut self, ts: u64) {
        let today = et_day(ts);
        let shorts: Vec<(String, i64)> = self
            .pos
            .iter()
            .filter(|(s, p)| {
                // Its last trading day, and a root that delivers shares.
                p.0 < 0
                    && self.last_trading_day(s) == Some(today)
                    && crate::intent::occ(s).is_some_and(|(root, _)| self.physical(root))
            })
            .map(|(s, p)| (s.clone(), -p.0))
            .collect();
        for (symbol, qty) in shorts {
            let Some(ask) = self.market.as_ref().and_then(|m| {
                m.ids
                    .get(&symbol)
                    .and_then(|id| m.book.get(id))
                    .and_then(|q| q.ask)
            }) else {
                continue;
            };
            let working: Vec<usize> = (0..self.orders.len())
                .filter(|i| {
                    self.open(*i) && self.orders[*i].legs.iter().any(|l| l.symbol == symbol)
                })
                .collect();
            for i in working {
                self.set_status(i, "canceled");
            }
            let n = self.orders.len() + 1;
            let leg = super::SimLeg {
                symbol: symbol.clone(),
                ratio: 1,
                buy: true,
                open: false,
                filled: 0,
                cost: 0,
            };
            let i = self.create(
                format!("sim-liquidation-{n}"),
                false,
                vec![leg],
                u64::try_from(qty).unwrap_or(0),
                0,
                super::script::Rule::default(),
            );
            self.orders[i].active = true;
            self.push(i, "new", None, None, None);
            self.execute(i, u64::try_from(qty).unwrap_or(0), &[ask]);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::worst;

    const D: i64 = 1_000_000_000;

    #[test]
    fn the_worst_loss_of_spreads_and_condors() {
        // A put credit spread 570/565: worst −5 × 100.
        assert_eq!(
            worst(&[(false, 570 * D, -1), (false, 565 * D, 1)]),
            Some(-500 * i128::from(D))
        );
        // An iron condor 565/570/590/595: worst −5 × 100 on either side.
        let condor = [
            (false, 565 * D, 1),
            (false, 570 * D, -1),
            (true, 590 * D, -1),
            (true, 595 * D, 1),
        ];
        assert_eq!(worst(&condor), Some(-500 * i128::from(D)));
        // A naked short call has no bound; a long call can't lose at expiry.
        assert_eq!(worst(&[(true, 590 * D, -1)]), None);
        assert_eq!(worst(&[(true, 590 * D, 1)]), Some(0));
    }
}

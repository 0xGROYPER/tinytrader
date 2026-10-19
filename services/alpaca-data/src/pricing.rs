//! Option prices, implied volatilities and greeks, computed here from
//! quotes; Alpaca's own `impliedVolatility` and `greeks` are never used.
//!
//! - **Model.** Black-76 on each expiry's forward from put-call parity,
//!   discounted at a flat [`RATE`]. Equity options are American; on the
//!   forward, Black is close for the out-of-the-money side (early exercise
//!   matters mostly for deep in-the-money puts and calls before a
//!   dividend), so at-the-money and smile volatilities take the
//!   out-of-the-money side: puts below the forward, calls at or above.
//! - **Forward.** `F = K + e^(rT)(C − P)` at the three strikes where call and
//!   put mids are closest, their median; the price when fewer than three
//!   strikes have both, or when parity lands more than 10% away from it.
//! - **Quotes.** A contract's volatility is inverted from its two-sided mid
//!   (bid above zero, not crossed, not too wide: `chain::WIDE`); none when
//!   the mid is at or below its discounted intrinsic value, or for a
//!   one-sided quote.
//! - **Greeks** from that volatility: delta and gamma against the
//!   underlying's price (`F = S·e^((r−q)T)`, so `dF/dS = F/S`), theta per
//!   calendar day, vega per volatility point.

use crate::chain::Contract;

/// The flat interest rate for discounting, about US T-bill yields in 2026.
/// A point off moves a one-month at-the-money volatility by well under a
/// tenth of a point.
pub const RATE: f64 = 0.04;

/// `e^(−rT)`.
#[must_use]
pub fn discount(t: f64) -> f64 {
    (-RATE * t).exp()
}

pub fn norm_pdf(x: f64) -> f64 {
    (-0.5 * x * x).exp() / (2.0 * std::f64::consts::PI).sqrt()
}

/// The standard normal CDF.
pub fn norm_cdf(x: f64) -> f64 {
    0.5 * erfc(-x / std::f64::consts::SQRT_2)
}

fn erfc(x: f64) -> f64 {
    // Numerical Recipes' erfcc: fractional error below 1.2e-7 everywhere.
    let z = x.abs();
    let t = 1.0 / (1.0 + 0.5 * z);
    let r = t
        * (-z * z - 1.265_512_23
            + t * (1.000_023_68
                + t * (0.374_091_96
                    + t * (0.096_784_18
                        + t * (-0.186_288_06
                            + t * (0.278_868_07
                                + t * (-1.135_203_98
                                    + t * (1.488_515_87
                                        + t * (-0.822_152_23 + t * 0.170_872_77)))))))))
            .exp();
    if x >= 0.0 { r } else { 2.0 - r }
}

/// Black's undiscounted price on the forward with total variance `w`
/// (`σ²T`); multiply by [`discount`] for a premium.
pub fn black(f: f64, k: f64, w: f64, call: bool) -> f64 {
    if w <= 0.0 {
        return if call {
            (f - k).max(0.0)
        } else {
            (k - f).max(0.0)
        };
    }
    let s = w.sqrt();
    let d1 = ((f / k).ln() + w / 2.0) / s;
    let d2 = d1 - s;
    if call {
        f * norm_cdf(d1) - k * norm_cdf(d2)
    } else {
        k * norm_cdf(-d2) - f * norm_cdf(-d1)
    }
}

/// The total variance at which Black's undiscounted price is `price`, by
/// bisection; none at or below intrinsic value or at or above the bound.
pub fn implied_w(price: f64, f: f64, k: f64, call: bool) -> Option<f64> {
    let (lo_p, hi_p) = (black(f, k, 0.0, call), if call { f } else { k });
    if !(price > lo_p && price < hi_p) {
        return None;
    }
    let (mut lo, mut hi) = (1e-4_f64, 5.0_f64); // σ√T
    if black(f, k, hi * hi, call) < price {
        return None;
    }
    for _ in 0..100 {
        let mid = 0.5 * (lo + hi);
        if black(f, k, mid * mid, call) < price {
            lo = mid
        } else {
            hi = mid
        }
    }
    let s = 0.5 * (lo + hi);
    Some(s * s)
}

/// One expiry's forward, `t` years away (see the module notes); `spot` is
/// the fallback, and without either, none.
#[must_use]
pub fn forward(list: &[&Contract], t: f64, spot: Option<f64>) -> Option<f64> {
    let growth = 1.0 / discount(t);
    let mut pairs: Vec<(f64, f64)> = list
        .iter()
        .filter(|c| c.call)
        .filter_map(|c| {
            let p = list.iter().find(|p| !p.call && p.strike == c.strike)?;
            let (cm, pm) = (c.mid()?, p.mid()?);
            Some(((cm - pm).abs(), c.strike + growth * (cm - pm)))
        })
        .collect();
    pairs.sort_by(|a, b| a.0.total_cmp(&b.0));
    if pairs.len() >= 3 {
        let mut f: Vec<f64> = pairs[..3].iter().map(|x| x.1).collect();
        f.sort_by(f64::total_cmp);
        let f = f[1];
        if f > 0.0 && spot.is_none_or(|s| (f / s - 1.0).abs() < 0.1) {
            return Some(f);
        }
    }
    spot
}

/// A contract's implied volatility and greeks.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Greeks {
    pub iv: f64,
    pub delta: f64,
    pub gamma: f64,
    /// Per calendar day.
    pub theta: f64,
    /// Per volatility point (0.01).
    pub vega: f64,
}

/// The volatility and greeks of an option quoted at `mid` (a premium),
/// strike `k`, `t` years from expiry, on forward `f`, with the underlying
/// at `spot`; none when the mid can't be inverted.
#[must_use]
pub fn greeks(mid: f64, f: f64, k: f64, t: f64, call: bool, spot: f64) -> Option<Greeks> {
    if !(mid > 0.0 && f > 0.0 && k > 0.0 && t > 0.0 && spot > 0.0) {
        return None;
    }
    let df = discount(t);
    let w = implied_w(mid / df, f, k, call)?;
    let iv = (w / t).sqrt();
    if !(0.01..=5.0).contains(&iv) {
        return None;
    }
    let sw = w.sqrt();
    let d1 = ((f / k).ln() + w / 2.0) / sw;
    let fs = f / spot;
    let delta = df
        * fs
        * if call {
            norm_cdf(d1)
        } else {
            norm_cdf(d1) - 1.0
        };
    let gamma = df * fs * fs * norm_pdf(d1) / (f * sw);
    // Black-76 theta with the forward held: decay plus the discount's carry.
    let theta_year = -df * f * norm_pdf(d1) * iv / (2.0 * t.sqrt()) + RATE * mid;
    let vega = df * f * norm_pdf(d1) * t.sqrt() / 100.0;
    Some(Greeks {
        iv,
        delta,
        gamma,
        theta: theta_year / 365.0,
        vega,
    })
}

/// Set every contract's volatility and greeks from its own quote, by
/// expiry: `years` gives an expiry's time to its close, `spot` the
/// underlying's price (the forward stands in without one). Whatever the
/// contracts held before is replaced; a contract that can't be priced gets
/// none.
pub fn price(contracts: &mut [Contract], spot: Option<f64>, years: &dyn Fn(&str) -> Option<f64>) {
    let mut expiries: Vec<String> = contracts.iter().map(|c| c.expiry.clone()).collect();
    expiries.sort();
    expiries.dedup();
    for expiry in expiries {
        let t = years(&expiry);
        let fwd = t.and_then(|t| {
            let list: Vec<&Contract> = contracts.iter().filter(|c| c.expiry == expiry).collect();
            forward(&list, t, spot)
        });
        for c in contracts.iter_mut().filter(|c| c.expiry == expiry) {
            let g = match (t, fwd) {
                (Some(t), Some(f)) => c
                    .mid()
                    .and_then(|m| greeks(m, f, c.strike, t, c.call, spot.unwrap_or(f))),
                _ => None,
            };
            c.iv = g.map(|g| g.iv);
            c.delta = g.map(|g| g.delta);
            c.gamma = g.map(|g| g.gamma);
            c.theta = g.map(|g| g.theta);
            c.vega = g.map(|g| g.vega);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn quote(strike: f64, call: bool, mid: f64) -> Contract {
        Contract {
            symbol: String::new(),
            expiry: "2026-11-13".into(),
            call,
            strike,
            bid: Some(mid - 0.01),
            ask: Some(mid + 0.01),
            last: None,
            iv: Some(9.9),
            delta: Some(9.9),
            gamma: None,
            theta: None,
            vega: None,
        }
    }

    /// The premium Black-76 gives at volatility `iv`.
    fn premium(f: f64, k: f64, t: f64, iv: f64, call: bool) -> f64 {
        discount(t) * black(f, k, iv * iv * t, call)
    }

    #[test]
    fn recovers_the_volatility_it_was_priced_at() {
        let (f, t) = (100.0, 0.25);
        for (k, call) in [(90.0, false), (100.0, true), (100.0, false), (115.0, true)] {
            let g = greeks(premium(f, k, t, 0.3, call), f, k, t, call, f).unwrap();
            assert!((g.iv - 0.3).abs() < 1e-6, "{k} {call}: {}", g.iv);
        }
    }

    #[test]
    fn refuses_what_cant_be_inverted() {
        // Below discounted intrinsic, and no price at all.
        assert_eq!(greeks(9.0, 110.0, 100.0, 0.25, true, 110.0), None);
        assert_eq!(greeks(0.0, 100.0, 100.0, 0.25, true, 100.0), None);
        // A one-sided quote has no mid, so no volatility; Alpaca's is dropped.
        let mut c = vec![Contract {
            bid: None,
            ..quote(100.0, true, 2.0)
        }];
        price(&mut c, Some(100.0), &|_| Some(0.1));
        assert_eq!((c[0].iv, c[0].delta), (None, None));
    }

    #[test]
    fn greeks_are_sane() {
        let (f, t) = (100.0, 30.0 / 365.0);
        let call = greeks(premium(f, 100.0, t, 0.2, true), f, 100.0, t, true, f).unwrap();
        let put = greeks(premium(f, 100.0, t, 0.2, false), f, 100.0, t, false, f).unwrap();
        // At the money: deltas near ±0.5 a call's minus a put's is the discount.
        assert!((call.delta - 0.51).abs() < 0.02 && (put.delta + 0.49).abs() < 0.02);
        assert!((call.delta - put.delta - discount(t)).abs() < 1e-9);
        assert!((call.gamma - put.gamma).abs() < 1e-12 && call.gamma > 0.0);
        // Vega against a bumped price, per point.
        let up = premium(f, 100.0, t, 0.21, true) - premium(f, 100.0, t, 0.2, true);
        assert!((call.vega - up).abs() < 0.002, "{} {up}", call.vega);
        // A day's decay: roughly the price change one day closer.
        let day = premium(f, 100.0, t - 1.0 / 365.0, 0.2, true) - premium(f, 100.0, t, 0.2, true);
        assert!(
            call.theta < 0.0 && (call.theta - day).abs() < 0.005,
            "{} {day}",
            call.theta
        );
    }

    #[test]
    fn prices_a_chain_from_parity() {
        let t = 0.25;
        let mut c: Vec<Contract> = [95.0, 100.0, 105.0]
            .iter()
            .flat_map(|&k| {
                [
                    quote(k, true, premium(101.0, k, t, 0.25, true)),
                    quote(k, false, premium(101.0, k, t, 0.25, false)),
                ]
            })
            .collect();
        let list: Vec<&Contract> = c.iter().collect();
        assert!((forward(&list, t, Some(100.0)).unwrap() - 101.0).abs() < 1e-9);
        price(&mut c, Some(100.0), &|_| Some(t));
        for x in &c {
            // Alpaca's 9.9 replaced by our 25%.
            assert!((x.iv.unwrap() - 0.25).abs() < 1e-6);
        }
    }
}

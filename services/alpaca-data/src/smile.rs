//! The market-implied distribution of the price at one expiry, from the whole
//! chain rather than the at-the-money straddle alone.
//!
//! - **Forward.** From put-call parity at the strikes nearest the price
//!   (`F = K + C - P`, zero rates), so dividends and carry are in it; the
//!   price when no strike has both quotes.
//! - **Quotes used.** The out-of-the-money side at each strike (puts below the
//!   forward, calls at or above), with a bid, not crossed, a mid of at least
//!   [`MIN_MID`] and a spread of at most [`MAX_SPREAD`] of the mid. Implied
//!   volatility is inverted from the mid with Black's formula on the forward
//!   (zero rates, European exercise: Alpaca's own IVs use another model), and
//!   kept between 2% and 400%.
//! - **Smile.** Total variance `w = σ²T` against log-moneyness `k = ln(K/F)`,
//!   fitted as raw SVI, `w = a + c(k - m) + d·√((k - m)² + s²)`: for each
//!   (m, s) on a grid the rest is a weighted linear least squares; candidates
//!   breaking `d ≥ 0`, `|c| ≤ d`, the wing slopes' bound `d + |c| ≤ 2` (Lee)
//!   or a negative minimum variance are dropped. Residuals are weighted to
//!   equalise errors in volatility, and by quote tightness. Without a valid
//!   SVI fit, a quadratic in k, extended linearly past the quoted strikes.
//! - **Distribution.** `P(S_T > K) = N(d2) - φ(d2)·w'(k) / (2√w)`, the
//!   Black digital plus the smile's slope correction, on a grid of strikes;
//!   made monotone and kept within [0, 1].
//! - **Quality.** `good`: at least 8 strikes, 3 or more each side of the
//!   forward, and an RMS volatility error of at most 2 points; `rough`: at
//!   least 5 strikes, some each side, at most 5 points; else `poor`, which
//!   consumers should replace by the straddle.

use serde_json::{Value, json};

use crate::chain::Contract;

/// Widest bid-ask spread used, as a fraction of the mid.
pub const MAX_SPREAD: f64 = 0.6;
/// Smallest mid used: quotes of a cent or two are mostly spread.
pub const MIN_MID: f64 = 0.02;

fn norm_pdf(x: f64) -> f64 {
    (-0.5 * x * x).exp() / (2.0 * std::f64::consts::PI).sqrt()
}

/// The standard normal CDF.
fn norm_cdf(x: f64) -> f64 {
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

/// Black's price on the forward, zero rates, with total variance `w`.
fn black(f: f64, k: f64, w: f64, call: bool) -> f64 {
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

/// The total variance at which Black's price is `price`, by bisection.
fn implied_w(price: f64, f: f64, k: f64, call: bool) -> Option<f64> {
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

/// One quoted strike: log-moneyness, total variance, and the fit's weight.
#[derive(Clone, Copy, Debug)]
struct Point {
    k: f64,
    w: f64,
    weight: f64,
    put: bool,
}

/// A fitted smile in total variance.
#[derive(Clone, Copy, Debug)]
enum Smile {
    Svi {
        a: f64,
        c: f64,
        d: f64,
        m: f64,
        s: f64,
    },
    /// `q0 + q1 k + q2 k²` inside [lo, hi], linear beyond.
    Quad { q: [f64; 3], lo: f64, hi: f64 },
}

impl Smile {
    fn w(&self, k: f64) -> f64 {
        match *self {
            Smile::Svi { a, c, d, m, s } => a + c * (k - m) + d * ((k - m).powi(2) + s * s).sqrt(),
            Smile::Quad { q, lo, hi } => {
                let x = k.clamp(lo, hi);
                let at = q[0] + q[1] * x + q[2] * x * x;
                at + (k - x) * self.dw(x)
            }
        }
    }
    fn dw(&self, k: f64) -> f64 {
        match *self {
            Smile::Svi { c, d, m, s, .. } => c + d * (k - m) / ((k - m).powi(2) + s * s).sqrt(),
            Smile::Quad { q, lo, hi } => (q[1] + 2.0 * q[2] * k.clamp(lo, hi)).clamp(-2.0, 2.0),
        }
    }
    fn name(&self) -> &'static str {
        match self {
            Smile::Svi { .. } => "svi",
            Smile::Quad { .. } => "quadratic",
        }
    }
}

/// Weighted least squares for three coefficients.
fn lsq3(rows: &[([f64; 3], f64, f64)]) -> Option<[f64; 3]> {
    let mut a = [[0.0; 4]; 3];
    for (x, y, wt) in rows {
        for i in 0..3 {
            for j in 0..3 {
                a[i][j] += wt * x[i] * x[j];
            }
            a[i][3] += wt * x[i] * y;
        }
    }
    for col in 0..3 {
        let p = (col..3).max_by(|&r1, &r2| a[r1][col].abs().total_cmp(&a[r2][col].abs()))?;
        if a[p][col].abs() < 1e-14 {
            return None;
        }
        a.swap(col, p);
        for r in 0..3 {
            if r != col {
                let f = a[r][col] / a[col][col];
                let pivot = a[col];
                for (x, p) in a[r].iter_mut().zip(pivot).skip(col) {
                    *x -= f * p;
                }
            }
        }
    }
    Some([a[0][3] / a[0][0], a[1][3] / a[1][1], a[2][3] / a[2][2]])
}

fn sse(smile: &Smile, pts: &[Point]) -> f64 {
    pts.iter()
        .map(|p| p.weight * (smile.w(p.k) - p.w).powi(2))
        .sum()
}

fn fit_svi(pts: &[Point]) -> Option<Smile> {
    let (lo, hi) = pts
        .iter()
        .fold((f64::MAX, f64::MIN), |(l, h), p| (l.min(p.k), h.max(p.k)));
    let mut best: Option<(f64, Smile)> = None;
    for i in 0..=24 {
        let m = lo + (hi - lo) * f64::from(i) / 24.0;
        for s in [
            0.002, 0.005, 0.01, 0.02, 0.04, 0.07, 0.1, 0.15, 0.2, 0.3, 0.45, 0.7, 1.0,
        ] {
            let rows: Vec<_> = pts
                .iter()
                .map(|p| {
                    (
                        [1.0, p.k - m, ((p.k - m).powi(2) + s * s).sqrt()],
                        p.w,
                        p.weight,
                    )
                })
                .collect();
            let Some([a, c, d]) = lsq3(&rows) else {
                continue;
            };
            let min_w = if d > 0.0 {
                a + s * (d * d - c * c).max(0.0).sqrt()
            } else {
                a
            };
            if d < 0.0 || c.abs() > d || d + c.abs() > 2.0 || min_w <= 0.0 {
                continue;
            }
            let smile = Smile::Svi { a, c, d, m, s };
            let e = sse(&smile, pts);
            if best.is_none_or(|(b, _)| e < b) {
                best = Some((e, smile));
            }
        }
    }
    best.map(|(_, s)| s)
}

fn fit_quad(pts: &[Point]) -> Option<Smile> {
    let (lo, hi) = pts
        .iter()
        .fold((f64::MAX, f64::MIN), |(l, h), p| (l.min(p.k), h.max(p.k)));
    let rows: Vec<_> = pts
        .iter()
        .map(|p| ([1.0, p.k, p.k * p.k], p.w, p.weight))
        .collect();
    let q = lsq3(&rows)?;
    let smile = Smile::Quad { q, lo, hi };
    // Positive variance across a generous range.
    (0..=40)
        .all(|i| smile.w(lo - 0.5 + (hi - lo + 1.0) * f64::from(i) / 40.0) > 0.0)
        .then_some(smile)
}

/// The forward from put-call parity at the (up to) three strikes nearest
/// `spot` with both mids; the median, or `spot`.
fn forward(list: &[&Contract], spot: f64) -> f64 {
    let mut pairs: Vec<(f64, f64)> = list
        .iter()
        .filter(|c| c.call)
        .filter_map(|c| {
            let p = list.iter().find(|p| !p.call && p.strike == c.strike)?;
            Some(((c.strike - spot).abs(), c.strike + c.mid()? - p.mid()?))
        })
        .collect();
    pairs.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut f: Vec<f64> = pairs.iter().take(3).map(|x| x.1).collect();
    f.sort_by(f64::total_cmp);
    match f.get(f.len() / 2) {
        Some(&x) if x > 0.0 && (x / spot - 1.0).abs() < 0.1 => x,
        _ => spot,
    }
}

/// Linear interpolation of `ys` at `x` over increasing `xs`.
fn interp(xs: &[f64], ys: &[f64], x: f64) -> f64 {
    if x <= xs[0] {
        return ys[0];
    }
    let i = xs.partition_point(|&v| v < x);
    if i >= xs.len() {
        return ys[ys.len() - 1];
    }
    let t = (x - xs[i - 1]) / (xs[i] - xs[i - 1]);
    ys[i - 1] + t * (ys[i] - ys[i - 1])
}

const QUANTILES: [(&str, f64); 7] = [
    ("p2_5", 0.025),
    ("p16", 0.16),
    ("p25", 0.25),
    ("p50", 0.5),
    ("p75", 0.75),
    ("p84", 0.84),
    ("p97_5", 0.975),
];

/// The implied distribution of one expiry's contracts, `t` years away, with
/// the price `spot` and the at-the-money straddle (for the probability of
/// finishing inside it). Null when there is nothing to fit.
#[must_use]
pub fn distribution(list: &[&Contract], spot: f64, t: f64, straddle: Option<f64>) -> Value {
    if !(spot > 0.0 && t > 0.0) {
        return Value::Null;
    }
    let f = forward(list, spot);
    let otm: Vec<&&Contract> = list
        .iter()
        .filter(|c| if c.call { c.strike >= f } else { c.strike < f })
        .collect();
    let candidates = otm
        .iter()
        .filter(|c| c.bid.is_some() || c.ask.is_some())
        .count();
    let pts: Vec<Point> = otm
        .iter()
        .filter_map(|c| {
            let (bid, ask) = (c.bid?, c.ask?);
            let mid = c.mid()?;
            if bid <= 0.0 || ask < bid || mid < MIN_MID || (ask - bid) / mid > MAX_SPREAD {
                return None;
            }
            let w = implied_w(mid, f, c.strike, c.call)?;
            let iv = (w / t).sqrt();
            if !(0.02..=4.0).contains(&iv) {
                return None;
            }
            // Equal weight to errors in volatility (dσ ≈ dw / 2√(wT)), more to tight quotes.
            let tight = 1.0 / ((ask - bid) / mid).max(0.05);
            Some(Point {
                k: (c.strike / f).ln(),
                w,
                weight: tight / w,
                put: !c.call,
            })
        })
        .collect();
    let (puts, calls) = (
        pts.iter().filter(|p| p.put).count(),
        pts.iter().filter(|p| !p.put).count(),
    );
    let usable = if candidates > 0 {
        pts.len() as f64 / candidates as f64
    } else {
        0.0
    };
    let base = json!({ "forward": f, "strikes": pts.len(), "puts": puts, "calls": calls, "usable": usable });
    let poor = |why: &str| {
        let mut v = base.clone();
        v["quality"] = json!("poor");
        v["reason"] = json!(why);
        v
    };
    if pts.len() < 5 {
        return poor("fewer than 5 usable strikes");
    }
    let Some(smile) = fit_svi(&pts).or_else(|| fit_quad(&pts)) else {
        return poor("no smile fits");
    };
    let rms_iv = (pts
        .iter()
        .map(|p| ((smile.w(p.k).max(0.0) / t).sqrt() - (p.w / t).sqrt()).powi(2))
        .sum::<f64>()
        / pts.len() as f64)
        .sqrt();
    let quality = if pts.len() >= 8 && puts >= 3 && calls >= 3 && rms_iv <= 0.02 {
        "good"
    } else if pts.len() >= 5 && puts >= 1 && calls >= 1 && rms_iv <= 0.05 {
        "rough"
    } else {
        "poor"
    };

    // The CDF on a grid of ±7 at-the-money standard deviations.
    let sd = smile.w(0.0).max(1e-8).sqrt().max(0.003);
    let n = 600;
    let (mut ks, mut cdf) = (Vec::with_capacity(n + 1), Vec::with_capacity(n + 1));
    let mut run = 0.0_f64;
    for i in 0..=n {
        let k = -7.0 * sd + 14.0 * sd * i as f64 / n as f64;
        let w = smile.w(k).max(1e-12);
        let sw = w.sqrt();
        let d2 = -k / sw - sw / 2.0;
        let above = norm_cdf(d2) - norm_pdf(d2) * smile.dw(k) / (2.0 * sw);
        run = run.max((1.0 - above).clamp(0.0, 1.0));
        ks.push(f * k.exp());
        cdf.push(run);
    }
    let at = |x: f64| interp(&ks, &cdf, x);
    let quantile = |p: f64| {
        let i = cdf.partition_point(|&c| c < p);
        if i == 0 {
            ks[0]
        } else if i >= cdf.len() {
            ks[ks.len() - 1]
        } else if cdf[i] - cdf[i - 1] < 1e-12 {
            ks[i]
        } else {
            ks[i - 1] + (p - cdf[i - 1]) / (cdf[i] - cdf[i - 1]) * (ks[i] - ks[i - 1])
        }
    };
    // E|S_T - S| = ∫ below S of F(x) dx + ∫ above S of (1 - F(x)) dx.
    let mut mad = 0.0;
    for i in 1..ks.len() {
        let (a, b) = (ks[i - 1], ks[i]);
        let mid = 0.5 * (cdf[i - 1] + cdf[i]);
        if b <= spot {
            mad += mid * (b - a);
        } else if a >= spot {
            mad += (1.0 - mid) * (b - a);
        } else {
            mad += at(0.5 * (a + spot)) * (spot - a) + (1.0 - at(0.5 * (spot + b))) * (b - spot);
        }
    }
    let q: serde_json::Map<String, Value> = QUANTILES
        .iter()
        .map(|(name, p)| ((*name).to_owned(), json!(quantile(*p))))
        .collect();
    let (q2, q97) = (quantile(0.025), quantile(0.975));
    // The 25-delta risk reversal: the fitted volatility where a put's delta
    // is -0.25 less where a call's is 0.25 (forward deltas, N(d1)).
    let at_delta = |target: f64| {
        let d1 = |k: f64| {
            let sw = smile.w(k).max(1e-12).sqrt();
            -k / sw + sw / 2.0
        };
        let (mut lo, mut hi) = (-7.0 * sd, 7.0 * sd);
        for _ in 0..80 {
            let mid = 0.5 * (lo + hi);
            if norm_cdf(d1(mid)) > target {
                lo = mid
            } else {
                hi = mid
            }
        }
        (smile.w(0.5 * (lo + hi)).max(0.0) / t).sqrt()
    };
    let rr25 = at_delta(0.75) - at_delta(0.25);
    // A thinned CDF between the 0.1% and 99.9% quantiles, for consumers.
    let (lo, hi) = (quantile(0.001), quantile(0.999));
    let thin: Vec<Value> = (0..=80)
        .map(|i| {
            let x = lo + (hi - lo) * f64::from(i) / 80.0;
            json!([(x * 1e4).round() / 1e4, (at(x) * 1e5).round() / 1e5])
        })
        .collect();
    let mut v = base;
    v["quality"] = json!(quality);
    v["model"] = json!(smile.name());
    v["rms_iv"] = json!(rms_iv);
    v["quantiles"] = Value::Object(q);
    v["p_inside_straddle"] = json!(straddle.map(|m| at(spot + m) - at(spot - m)));
    v["mean_abs_move"] = json!(mad / spot);
    v["rr25"] = json!(rr25);
    v["tail_ratio"] = json!((q97 > spot && q2 < spot).then(|| (spot - q2) / (q97 - spot)));
    v["cdf"] = Value::Array(thin);
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    fn contract(strike: f64, call: bool, price: f64, half_spread: f64) -> Contract {
        Contract {
            symbol: String::new(),
            expiry: "2026-11-13".into(),
            call,
            strike,
            bid: Some((price - half_spread).max(0.0)),
            ask: Some(price + half_spread),
            last: None,
            iv: None,
            delta: None,
            gamma: None,
            theta: None,
            vega: None,
        }
    }

    /// A chain priced by Black with volatility `vol(k)` at forward 100.
    fn chain(t: f64, vol: impl Fn(f64) -> f64) -> Vec<Contract> {
        let mut out = Vec::new();
        for i in 0..=40 {
            let k = 70.0 + 1.5 * f64::from(i);
            let w = vol((k / 100.0_f64).ln()).powi(2) * t;
            for call in [true, false] {
                let p = black(100.0, k, w, call);
                out.push(contract(k, call, p, (p * 0.03).max(0.01)));
            }
        }
        out
    }

    fn dist(c: &[Contract], t: f64) -> Value {
        let refs: Vec<&Contract> = c.iter().collect();
        distribution(&refs, 100.0, t, Some(4.0))
    }

    #[test]
    fn flat_volatility_gives_lognormal_probabilities() {
        let t = 30.0 / 365.0;
        let d = dist(&chain(t, |_| 0.3), t);
        assert_eq!(d["quality"], "good", "{d}");
        assert!((d["forward"].as_f64().unwrap() - 100.0).abs() < 0.05);
        // P(S_T <= K) under the lognormal: N((ln(K/F) + w/2) / √w).
        let sw = 0.3 * t.sqrt();
        for (name, p) in [("p16", 0.16), ("p50", 0.5), ("p84", 0.84)] {
            let k = d["quantiles"][name].as_f64().unwrap();
            let model = norm_cdf(((k / 100.0).ln() + sw * sw / 2.0) / sw);
            assert!((model - p).abs() < 0.01, "{name}: {k} gives {model}");
        }
        // The straddle's move is about 0.8σ: about 57% inside.
        let straddle = 2.0 * black(100.0, 100.0, sw * sw, true);
        let refs: Vec<&Contract> = chain(t, |_| 0.3).leak().iter().collect();
        let inside = distribution(&refs, 100.0, t, Some(straddle))["p_inside_straddle"]
            .as_f64()
            .unwrap();
        assert!((inside - 0.575).abs() < 0.02, "{inside}");
        // The mean absolute move is the straddle (zero rates, at the money).
        assert!((d["mean_abs_move"].as_f64().unwrap() - straddle / 100.0).abs() < 0.002);
    }

    #[test]
    fn a_skewed_smile_fattens_the_left_tail() {
        let t = 30.0 / 365.0;
        let flat = dist(&chain(t, |_| 0.3), t);
        let skew = dist(&chain(t, |k| 0.3 - 0.5 * k + 0.8 * k * k), t);
        assert_ne!(skew["quality"], "poor", "{skew}");
        let lo = |d: &Value| d["quantiles"]["p2_5"].as_f64().unwrap();
        assert!(
            lo(&skew) < lo(&flat) - 1.0,
            "{} vs {}",
            lo(&skew),
            lo(&flat)
        );
        assert!(skew["tail_ratio"].as_f64().unwrap() > flat["tail_ratio"].as_f64().unwrap() + 0.2);
        // Puts richer than calls by the smile's slope: about 0.5 × the 25-delta strikes' gap.
        assert!(flat["rr25"].as_f64().unwrap().abs() < 0.005);
        assert!(skew["rr25"].as_f64().unwrap() > 0.03, "{}", skew["rr25"]);
    }

    #[test]
    fn junk_quotes_are_left_out() {
        let t = 30.0 / 365.0;
        let mut c = chain(t, |_| 0.3);
        let n0 = dist(&c, t)["strikes"].as_u64().unwrap();
        // A crossed quote, one without a bid and one far too wide.
        c.push(Contract {
            strike: 131.0,
            bid: Some(2.0),
            ask: Some(1.0),
            ..contract(131.0, true, 1.0, 0.0)
        });
        c.push(Contract {
            strike: 132.0,
            bid: None,
            ..contract(132.0, true, 0.5, 0.1)
        });
        c.push(Contract {
            strike: 133.0,
            bid: Some(0.05),
            ask: Some(0.9),
            ..contract(133.0, true, 0.5, 0.0)
        });
        let d = dist(&c, t);
        assert_eq!(d["strikes"].as_u64().unwrap(), n0);
        assert_eq!(d["quality"], "good");
    }

    #[test]
    fn a_thin_chain_is_poor() {
        let t = 30.0 / 365.0;
        let c: Vec<Contract> = chain(t, |_| 0.3)
            .into_iter()
            .filter(|c| (c.strike - 100.0).abs() < 2.0)
            .collect();
        assert_eq!(dist(&c, t)["quality"], "poor");
        // Wildly inconsistent quotes are poor too.
        let noisy = chain(t, |k| {
            if (k * 100.0).round() as i64 % 2 == 0 {
                0.2
            } else {
                0.6
            }
        });
        assert_eq!(dist(&noisy, t)["quality"], "poor");
    }

    #[test]
    fn the_cdf_is_monotone_and_bounded() {
        let t = 7.0 / 365.0;
        let d = dist(&chain(t, |k| 0.25 - 0.8 * k + 2.0 * k * k), t);
        let pts: Vec<(f64, f64)> = d["cdf"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| (p[0].as_f64().unwrap(), p[1].as_f64().unwrap()))
            .collect();
        assert!(pts.windows(2).all(|w| w[1].0 > w[0].0 && w[1].1 >= w[0].1));
        assert!(pts.iter().all(|p| (0.0..=1.0).contains(&p.1)));
        assert!(pts[0].1 < 0.01 && pts[pts.len() - 1].1 > 0.99);
    }

    #[test]
    fn inverts_black() {
        for call in [true, false] {
            let p = black(100.0, 95.0, 0.01, call);
            assert!((implied_w(p, 100.0, 95.0, call).unwrap() - 0.01).abs() < 1e-8);
        }
    }
}

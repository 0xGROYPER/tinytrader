//! The options screener's pieces that don't need the network: Nasdaq's stock
//! screener (every US listing with its market capitalisation) and Alpaca's
//! most-active and movers lists parsed, and the measures computed per stock
//! from daily bars and the option chain.

use serde_json::Value;

/// One listing from Nasdaq's screener.
#[derive(Clone, Debug, PartialEq)]
pub struct Stock {
    /// As Alpaca writes it (`BRK.B`, where Nasdaq writes `BRK/B`).
    pub symbol: String,
    pub name: String,
    pub sector: String,
    pub price: Option<f64>,
    /// The day's change as a fraction (Nasdaq gives percent).
    pub change_pct: Option<f64>,
    pub volume: Option<f64>,
    pub market_cap: Option<f64>,
}

/// A number from Nasdaq's text: `$230.48`, `-2.944%`, `5554568000000.00`.
fn number(v: &Value) -> Option<f64> {
    let s = v
        .as_str()?
        .trim()
        .trim_start_matches('$')
        .trim_end_matches('%')
        .replace(',', "");
    s.parse::<f64>().ok().filter(|x| x.is_finite())
}

/// Whether a listing is a company's shares (common stock, an ADR, partnership
/// units), not its notes, preferreds, warrants or rights, which Nasdaq lists
/// with the company's market capitalisation.
#[must_use]
pub fn shares(name: &str) -> bool {
    let n = name.to_lowercase();
    if [
        "common stock",
        "ordinary shares",
        "american depositary",
        " adr",
    ]
    .iter()
    .any(|x| n.contains(x))
    {
        return true;
    }
    let words: Vec<&str> = n
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|w| !w.is_empty())
        .collect();
    let has = |w: &str| words.contains(&w);
    let pair = |a: &str, b: &[&str]| words.windows(2).any(|x| x[0] == a && b.contains(&x[1]));
    !(has("warrant")
        || has("warrants")
        || has("rights")
        || has("notes")
        || has("debentures")
        || has("zones")
        || pair("preferred", &["stock", "share", "shares", "units", "unit"])
        || pair("equity", &["unit", "units"]))
}

/// `/api/screener/stocks?tableonly=true&download=true`'s rows: shares only
/// (see [`shares`]).
#[must_use]
pub fn nasdaq(v: &Value) -> Vec<Stock> {
    v["data"]["rows"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|r| {
            let symbol = r["symbol"].as_str()?.trim().replace('/', ".");
            let name = r["name"].as_str().unwrap_or_default().trim().to_owned();
            let ok = shares(&name)
                && !symbol.is_empty()
                && symbol.len() <= 10
                && symbol
                    .bytes()
                    .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'.');
            ok.then(|| Stock {
                name,
                sector: r["sector"].as_str().unwrap_or_default().trim().to_owned(),
                price: number(&r["lastsale"]),
                change_pct: number(&r["pctchange"]).map(|p| p / 100.0),
                volume: number(&r["volume"]),
                market_cap: number(&r["marketCap"]).filter(|c| *c > 0.0),
                symbol,
            })
        })
        .collect()
}

/// Symbols from Alpaca's `/v1beta1/screener/stocks/most-actives`.
#[must_use]
pub fn most_actives(v: &Value) -> Vec<String> {
    v["most_actives"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|r| r["symbol"].as_str().map(str::to_owned))
        .collect()
}

/// (symbol, "gainer" | "loser") from Alpaca's `/v1beta1/screener/stocks/movers`.
#[must_use]
pub fn movers(v: &Value) -> Vec<(String, &'static str)> {
    let side = |key: &str, label: &'static str| {
        v[key]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(move |r| r["symbol"].as_str().map(|s| (s.to_owned(), label)))
            .collect::<Vec<_>>()
    };
    let mut out = side("gainers", "gainer");
    out.extend(side("losers", "loser"));
    out
}

/// What a scan of one stock found.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Measures {
    /// At-the-money IV of the expiry nearest 30 days, and that expiry.
    pub atm_iv_30d: Option<f64>,
    pub iv_expiry: Option<String>,
    /// The straddle over the price for the first expiry after today, and it.
    pub implied_move: Option<f64>,
    pub move_expiry: Option<String>,
    /// Annualised realized volatility of the last 20 daily log returns.
    pub rv_20d: Option<f64>,
    /// Average daily volume over the 20 sessions before today.
    pub avg_volume_20d: Option<f64>,
    /// The 30-day expiry's 25-delta risk reversal from its fitted smile: put
    /// less call implied volatility (positive: downside priced richer).
    pub skew: Option<f64>,
    /// Why a part is missing, when it is.
    pub note: Option<String>,
}

/// Annualised realized volatility of the daily log returns of `closes`
/// (oldest first): the root mean square, as volatility is quoted.
#[must_use]
pub fn realized_vol(closes: &[f64]) -> Option<f64> {
    let r: Vec<f64> = closes
        .windows(2)
        .filter(|w| w[0] > 0.0 && w[1] > 0.0)
        .map(|w| (w[1] / w[0]).ln())
        .collect();
    (r.len() >= 10).then(|| (252.0 * r.iter().map(|x| x * x).sum::<f64>() / r.len() as f64).sqrt())
}

/// The mean of `xs`, if there are any.
#[must_use]
pub fn mean(xs: &[f64]) -> Option<f64> {
    (!xs.is_empty()).then(|| xs.iter().sum::<f64>() / xs.len() as f64)
}

/// Days since 1970-01-01 of a `YYYY-MM-DD` date.
#[must_use]
pub fn days(d: &str) -> Option<i64> {
    let p = |a: usize, b: usize| d.get(a..b)?.parse::<i64>().ok();
    let (y, m, day) = (p(0, 4)?, p(5, 7)?, p(8, 10)?);
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (m + if m > 2 { -3 } else { 9 }) + 2) / 5 + day - 1;
    Some(era * 146_097 + yoe * 365 + yoe / 4 - yoe / 100 + doy - 719_468)
}

/// A measure and the expiry it was read from.
pub type AtExpiry = Option<(f64, String)>;

/// Weekdays after `today` up to and including `expiry`: the sessions an
/// option expiring then still trades through (holidays aside).
#[must_use]
pub fn sessions(today: &str, expiry: &str) -> Option<i64> {
    let (a, b) = (days(today)?, days(expiry)?);
    // 1970-01-01 was a Thursday: day 0 is weekday 3 (Monday 0).
    Some((a + 1..=b).filter(|d| (d + 3).rem_euclid(7) < 5).count() as i64)
}

/// Expiries closer than this many sessions are skipped for the implied move:
/// a day or two of straddle is mostly noise, or one event.
pub const MIN_SESSIONS: i64 = 3;

/// From the chain's per-expiry summaries (`chain::expiries`): the at-the-money
/// IV of the expiry nearest 30 days after `today`, and the implied move of the
/// first expiry at least [`MIN_SESSIONS`] away, each with its expiry.
#[must_use]
pub fn pick(expiries: &[Value], today: &str) -> (AtExpiry, AtExpiry) {
    let t0 = days(today).unwrap_or(0);
    let later = || {
        expiries
            .iter()
            .filter_map(|e| Some((e["expiry"].as_str()?, e)))
            .filter(move |(x, _)| *x > today)
    };
    let iv = later()
        .filter_map(|(x, e)| Some((e["atm_iv"].as_f64()?, x, (days(x)? - t0 - 30).abs())))
        .min_by_key(|(_, _, gap)| *gap)
        .map(|(v, x, _)| (v, x.to_owned()));
    let mv = later()
        .filter(|(x, _)| sessions(today, x).is_some_and(|n| n >= MIN_SESSIONS))
        .find_map(|(x, e)| e["implied_move"].as_f64().map(|m| (m, x.to_owned())));
    (iv, mv)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn reads_nasdaq_rows() {
        let v = json!({ "data": { "rows": [
            { "symbol": "NVDA", "name": "NVIDIA Corporation Common Stock", "lastsale": "$230.48", "pctchange": "-2.944%", "volume": "118700296", "marketCap": "5554568000000.00", "sector": "Technology" },
            { "symbol": "BRK/B", "name": "Berkshire", "lastsale": "$480.10", "pctchange": "0.5%", "volume": "3,100,200", "marketCap": "1000000000000", "sector": "Finance" },
            { "symbol": "ABC^", "lastsale": "$1", "pctchange": "", "volume": "", "marketCap": "" },
            { "symbol": "ZZZ", "lastsale": "NA", "pctchange": "UNCH", "volume": "10", "marketCap": "0.00" },
        ]}});
        let s = nasdaq(&v);
        assert_eq!(s.len(), 3);
        assert_eq!(s[0].symbol, "NVDA");
        assert_eq!(s[0].price, Some(230.48));
        assert!((s[0].change_pct.unwrap() + 0.029_44).abs() < 1e-12);
        assert_eq!(s[0].market_cap, Some(5_554_568_000_000.0));
        assert_eq!(
            (s[1].symbol.as_str(), s[1].volume),
            ("BRK.B", Some(3_100_200.0))
        );
        assert_eq!(
            (s[2].price, s[2].change_pct, s[2].market_cap),
            (None, None, None)
        );
    }

    #[test]
    fn keeps_shares_only() {
        for name in [
            "NVIDIA Corporation Common Stock",
            "Alibaba Group Holding Limited American Depositary Shares each representing eight Ordinary share",
            "Grupo Aval Acciones y Valores S.A. ADR (Each representing 20 preferred shares)",
            "Brookfield Renewable Partners L.P. Limited Partnership Units",
            "Rights Holdings Inc. Common Stock",
        ] {
            assert!(shares(name), "{name}");
        }
        for name in [
            "AT&T Inc. 5.350% Global Notes due 2066",
            "Comcast Holdings ZONES",
            "Revolution Medicines Inc. Warrant",
            "New Fortress Energy Inc. Series A Mandatorily Convertible Preferred Stock",
            "Alphabet Inc. Depositary Shares representing a 1/20th Interest in a Share of Series A Mandatory Convertible Preferred Stock",
            "Athene Holding Ltd. 7.250% Fixed-Rate Reset Junior Subordinated Debentures due 2064",
            "Brookfield Property Partners L.P. 6.25% Class A Cumulative Redeemable Preferred Units Series 1",
            "BrightSpring Health Services Inc. Tangible Equity Unit",
        ] {
            assert!(!shares(name), "{name}");
        }
    }

    #[test]
    fn reads_alpaca_screeners() {
        let a = json!({ "most_actives": [{ "symbol": "T", "volume": 1 }, { "symbol": "WFF" }] });
        assert_eq!(most_actives(&a), ["T", "WFF"]);
        let m = json!({ "gainers": [{ "symbol": "WFF", "percent_change": 499.51 }], "losers": [{ "symbol": "SXTC" }] });
        assert_eq!(
            movers(&m),
            [("WFF".to_owned(), "gainer"), ("SXTC".to_owned(), "loser")]
        );
    }

    #[test]
    fn realized_vol_is_annualised_rms() {
        // Alternating +1% and -1% log returns: 1% a day, about 15.9% a year.
        let mut closes = vec![100.0];
        for i in 0..20 {
            let last = *closes.last().unwrap();
            closes.push(
                last * if i % 2 == 0 {
                    0.01_f64.exp()
                } else {
                    (-0.01_f64).exp()
                },
            );
        }
        let v = realized_vol(&closes).unwrap();
        assert!((v - 0.01 * 252f64.sqrt()).abs() < 1e-9, "{v}");
        assert_eq!(realized_vol(&[100.0, 101.0]), None);
        assert_eq!(mean(&[1.0, 3.0]), Some(2.0));
        assert_eq!(mean(&[]), None);
    }

    #[test]
    fn picks_the_30_day_iv_and_the_next_move() {
        assert_eq!(days("1970-01-02"), Some(1));
        assert_eq!(
            days("2026-10-11").unwrap() - days("2026-09-11").unwrap(),
            30
        );
        let e = vec![
            json!({ "expiry": "2026-10-09", "atm_iv": 0.9, "implied_move": 0.01 }),
            json!({ "expiry": "2026-10-16", "atm_iv": 0.30, "implied_move": null }),
            json!({ "expiry": "2026-10-23", "atm_iv": 0.32, "implied_move": 0.031 }),
            json!({ "expiry": "2026-11-13", "atm_iv": 0.35, "implied_move": 0.06 }),
            json!({ "expiry": "2026-12-18", "atm_iv": 0.40, "implied_move": 0.09 }),
        ];
        let (iv, mv) = pick(&e, "2026-10-11");
        assert_eq!(iv, Some((0.35, "2026-11-13".to_owned())));
        assert_eq!(mv, Some((0.031, "2026-10-23".to_owned())));
        // From Sunday: Monday's expiry is one session away, too close.
        let e = vec![
            json!({ "expiry": "2026-10-12", "implied_move": 0.03 }),
            json!({ "expiry": "2026-10-16", "implied_move": 0.05 }),
        ];
        assert_eq!(
            pick(&e, "2026-10-11").1,
            Some((0.05, "2026-10-16".to_owned()))
        );
    }

    #[test]
    fn counts_sessions_to_expiry() {
        // Sunday 11 October 2026 to Friday the 16th: five sessions.
        assert_eq!(sessions("2026-10-11", "2026-10-16"), Some(5));
        // Friday to the next Monday: one; to the same day: none.
        assert_eq!(sessions("2026-10-16", "2026-10-19"), Some(1));
        assert_eq!(sessions("2026-10-16", "2026-10-16"), Some(0));
    }
}

//! Performance, for the console's Performance view: an account's daily P/L
//! (Alpaca's portfolio history, a day's change in equity) and a day's trades,
//! rebuilt from its fills. Read on request and kept a minute.
//!
//! A trade is a round trip: from flat to flat in one underlying and expiry
//! (an option group) or one stock, so scaling in and out stays one trade.
//! Its P/L is the cash its fills moved, before fees. Fills that never return
//! to flat by the day's end (still open, or closing a position carried in
//! from an earlier day) make a trade marked `open`, with no P/L.

use std::collections::BTreeMap;

use serde_json::{Value, json};

/// One fill, as Alpaca's FILL activity gives it.
#[derive(Clone, Debug, PartialEq)]
pub struct Fill {
    pub time: String,
    pub symbol: String,
    /// Signed: + bought, − sold.
    pub qty: f64,
    pub price: f64,
}

impl Fill {
    pub fn from_activity(a: &Value) -> Option<Self> {
        let num = |k: &str| {
            a[k].as_str()
                .and_then(|s| s.parse::<f64>().ok())
                .or_else(|| a[k].as_f64())
        };
        let side = if a["side"].as_str()? == "buy" {
            1.0
        } else {
            -1.0
        };
        Some(Self {
            time: a["transaction_time"].as_str()?.to_owned(),
            symbol: a["symbol"].as_str()?.to_owned(),
            qty: side * num("qty")?,
            price: num("price")?,
        })
    }
}

/// An OCC option symbol's root and expiry (`SPY`, `2026-10-15`).
fn option(symbol: &str) -> Option<(&str, String)> {
    let n = symbol
        .len()
        .checked_sub(15)
        .filter(|n| (1..=6).contains(n))?;
    let (root, rest) = symbol.split_at(n);
    let b = rest.as_bytes();
    (b[..6].iter().all(u8::is_ascii_digit)
        && matches!(b[6], b'C' | b'P')
        && b[7..].iter().all(u8::is_ascii_digit))
    .then(|| {
        (
            root,
            format!("20{}-{}-{}", &rest[0..2], &rest[2..4], &rest[4..6]),
        )
    })
}

/// The day's fills (oldest first) as round trips.
#[must_use]
pub fn trades(fills: &[Fill]) -> Vec<Value> {
    struct Open {
        start: String,
        end: String,
        held: BTreeMap<String, f64>,
        cash: f64,
        fills: usize,
        contracts: Vec<String>,
    }
    let mut open: BTreeMap<String, Open> = BTreeMap::new();
    let mut done = Vec::new();
    let out = |key: &str, t: &Open, closed: bool| {
        json!({
            "key": key, "start": t.start, "end": t.end, "fills": t.fills, "contracts": t.contracts,
            "open": !closed, "pnl": closed.then(|| (t.cash * 100.0).round() / 100.0),
        })
    };
    for f in fills {
        let (key, mult) = match option(&f.symbol) {
            Some((root, expiry)) => (format!("{root} {expiry}"), 100.0),
            None => (f.symbol.clone(), 1.0),
        };
        let t = open.entry(key.clone()).or_insert_with(|| Open {
            start: f.time.clone(),
            end: f.time.clone(),
            held: BTreeMap::new(),
            cash: 0.0,
            fills: 0,
            contracts: Vec::new(),
        });
        t.end.clone_from(&f.time);
        t.fills += 1;
        t.cash -= f.qty * f.price * mult;
        if !t.contracts.contains(&f.symbol) {
            t.contracts.push(f.symbol.clone());
        }
        let q = t.held.entry(f.symbol.clone()).or_insert(0.0);
        *q += f.qty;
        if q.abs() < 1e-9 {
            t.held.remove(&f.symbol);
        }
        if t.held.is_empty()
            && let Some(t) = open.remove(&key)
        {
            done.push(out(&key, &t, true));
        }
    }
    done.extend(open.iter().map(|(k, t)| out(k, t, false)));
    done.sort_by(|a, b| a["start"].as_str().cmp(&b["start"].as_str()));
    done
}

/// Money moved in or out rather than made: deposits, withdrawals and journals.
const TRANSFERS: &[&str] = &["CSD", "CSW", "JNLC", "JNLS"];

/// Daily P/L from a portfolio history (`timeframe=1D&cashflow_types=ALL`):
/// `[{date, pnl, pnl_pct, equity, transfers?}]`, each by its session's date,
/// days with no equity left out. A day's P/L is its change in equity less the
/// money moved in or out (fees and option cash stay in): Alpaca's own P/L
/// counts deposits as gains, except the first, which it takes as its base.
/// `pnl_pct` is against the equity the day started with, as a fraction. The
/// day in progress isn't in it yet.
#[must_use]
pub fn daily(h: &Value, ny_date: impl Fn(i64) -> String) -> Vec<Value> {
    let col = |k: &str| h[k].as_array().cloned().unwrap_or_default();
    let (t, e, pl) = (col("timestamp"), col("equity"), col("profit_loss"));
    let eq = |i: usize| e.get(i).and_then(Value::as_f64);
    let moved = |i: usize| -> f64 {
        TRANSFERS
            .iter()
            .filter_map(|k| h["cashflow"][k].get(i).and_then(Value::as_f64))
            .sum()
    };
    t.iter()
        .enumerate()
        .filter_map(|(i, t)| {
            let equity = e.get(i)?.as_f64().filter(|e| *e > 0.0)?;
            let transfers = moved(i);
            // Against the day before. After an empty day (or with none before),
            // a day funded is all transfer; one that isn't has only Alpaca's P/L.
            let gain = match i.checked_sub(1).and_then(eq) {
                Some(prev) if prev > 0.0 => Some(equity - prev - transfers),
                _ if transfers != 0.0 => Some(equity - transfers),
                _ => pl.get(i).and_then(Value::as_f64),
            };
            let pnl = gain.map(|g| (g * 100.0).round() / 100.0);
            let before = pnl.map(|p| equity - p - transfers);
            let mut day = json!({
                // A day's bar is stamped at the midnight that ends it (UTC): Monday's
                // session is Tuesday 00:00. Noon before it names the session's date.
                "date": ny_date(t.as_i64()? - 12 * 3600),
                "equity": equity,
                "pnl": pnl,
                "pnl_pct": pnl.zip(before).filter(|(_, b)| *b > 0.0).map(|(p, b)| (p / b * 1e4).round() / 1e4),
            });
            if transfers != 0.0 {
                day["transfers"] = json!(transfers);
            }
            Some(day)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fill(time: &str, symbol: &str, qty: f64, price: f64) -> Fill {
        Fill {
            time: time.into(),
            symbol: symbol.into(),
            qty,
            price,
        }
    }

    #[test]
    fn scaling_in_and_out_is_one_round_trip() {
        // Two call spreads opened, both closed together: one trade.
        let f = [
            fill("09:32", "SPY261015C00769000", 1.0, 0.61),
            fill("09:32", "SPY261015C00765000", -1.0, 2.36),
            fill("09:41", "SPY261015C00765000", -1.0, 3.02),
            fill("09:41", "SPY261015C00769000", 1.0, 0.89),
            fill("09:52", "SPY261015C00765000", 2.0, 2.11),
            fill("09:52", "SPY261015C00769000", -2.0, 0.54),
            fill("10:05", "AAPL", 10.0, 200.0),
        ];
        let t = trades(&f);
        assert_eq!(t.len(), 2);
        assert_eq!(t[0]["key"], "SPY 2026-10-15");
        assert_eq!(t[0]["pnl"], 74.0);
        assert_eq!(
            (
                t[0]["start"].as_str(),
                t[0]["end"].as_str(),
                t[0]["fills"].as_u64()
            ),
            (Some("09:32"), Some("09:52"), Some(6))
        );
        // Still held at the day's end: open, no P/L.
        assert_eq!(
            (
                t[1]["key"].as_str(),
                t[1]["open"].as_bool(),
                t[1]["pnl"].is_null()
            ),
            (Some("AAPL"), Some(true), true)
        );
    }

    #[test]
    fn reads_fill_activities_and_daily_history() {
        let a = json!({ "transaction_time": "2026-10-15T13:32:09.782Z", "symbol": "SPY261015C00765000", "side": "sell", "qty": "1", "price": "2.36" });
        assert_eq!(
            Fill::from_activity(&a),
            Some(fill(
                "2026-10-15T13:32:09.782Z",
                "SPY261015C00765000",
                -1.0,
                2.36
            ))
        );
        let h = json!({ "timestamp": [1, 2], "equity": [0.0, 886.17], "profit_loss": [0.0, 289.05], "profit_loss_pct": [0.0, 0.4841] });
        let d = daily(&h, |t| format!("d{t}"));
        assert_eq!(
            d,
            vec![json!({ "date": "d-43198", "equity": 886.17, "pnl": 289.05, "pnl_pct": 0.4841 })]
        );
        // A deposit isn't a gain.
        let h = json!({ "timestamp": [0, 86_400], "equity": [2.62, 502.62], "profit_loss": [0.0, 500.0], "cashflow": { "CSD": [0.0, 500.0], "FEE": [0.0, -0.5] } });
        let d = daily(&h, |t| format!("d{t}"));
        assert_eq!(
            (d[1]["pnl"].as_f64(), d[1]["transfers"].as_f64()),
            (Some(0.0), Some(500.0))
        );
    }

    #[test]
    fn deposits_are_not_gains_even_the_first() {
        // Funded from nothing (Alpaca's P/L is 0 that day, its base), then a gain, then a deposit.
        let h = json!({ "timestamp": [86_400, 172_800, 259_200, 345_600], "equity": [0.0, 500.0, 520.0, 1020.0],
                        "profit_loss": [0.0, 0.0, 20.0, 500.0], "cashflow": { "CSD": [0.0, 500.0, 0.0, 500.0], "FEE": [0.0, 0.0, -0.5, 0.0] } });
        let d = daily(&h, |t| format!("d{t}"));
        let pnl: Vec<_> = d.iter().map(|x| x["pnl"].as_f64()).collect();
        assert_eq!(pnl, vec![Some(0.0), Some(20.0), Some(0.0)]);
        assert_eq!(d[1]["pnl_pct"].as_f64(), Some(0.04));
        assert_eq!(d[2]["transfers"].as_f64(), Some(500.0));
    }
}

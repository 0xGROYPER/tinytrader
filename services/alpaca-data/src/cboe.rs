//! Bars for indexes (XSP, SPX, VIX, ...), which Alpaca doesn't serve, from
//! the delayed quotes behind cboe.com's charts (15 minutes late; for
//! personal use under Cboe's site terms): daily bars back to 2009, and the
//! current session's minutes. Cboe keeps only today's minutes, so each
//! day's are saved under `<dir>/cboe/<SYMBOL>/<YYYY-MM-DD>.json` as they
//! arrive, and intraday history reaches back to the first day recorded.
//!
//! Bars are `[t, o, h, l, c, v]` like Alpaca's, `t` each bar's start (Cboe
//! labels a minute by its end), daily bars at New York midnight; indexes
//! have no volume, so `v` is 0.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};
use tinytrader_schedule::{ny_date_time, ny_to_unix};

const BASE: &str = "https://cdn-api.cboe.com/api/global/delayed_quotes/charts";

/// The indexes served from Cboe; any other symbol goes to Alpaca.
pub const INDEXES: &[&str] = &[
    "XSP", "SPX", "VIX", "VIX9D", "VIX1D", "VIX3M", "NDX", "XND", "RUT", "MRUT", "DJX", "OEX",
    "XEO",
];

#[must_use]
pub fn is_index(symbol: &str) -> bool {
    INDEXES.contains(&symbol)
}

/// One of Cboe's chart files for `symbol` (`historical` or `intraday`).
pub async fn fetch(http: &reqwest::Client, kind: &str, symbol: &str) -> Result<Value, String> {
    let r = http
        .get(format!("{BASE}/{kind}/_{symbol}.json"))
        .header("User-Agent", "Mozilla/5.0 (tinytrader)")
        .send()
        .await
        .map_err(|e| format!("Cboe {kind} {symbol}: request failed: {e}"))?;
    if !r.status().is_success() {
        return Err(format!("Cboe {kind} {symbol} answered {}", r.status()));
    }
    r.json()
        .await
        .map_err(|e| format!("Cboe {kind} {symbol}: unreadable: {e}"))
}

/// Daily bars from Cboe's historical file, oldest first.
#[must_use]
pub fn daily(v: &Value) -> Vec<Value> {
    let num = |x: &Value| {
        x.as_str()
            .and_then(|s| s.parse::<f64>().ok())
            .or_else(|| x.as_f64())
    };
    v["data"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|d| {
            let t = midnight(d["date"].as_str()?)?;
            bar(
                t,
                num(&d["open"]),
                num(&d["high"]),
                num(&d["low"]),
                num(&d["close"])?,
            )
        })
        .collect()
}

/// Minute bars from Cboe's intraday file, oldest first.
#[must_use]
pub fn minutes(v: &Value) -> Vec<Value> {
    v["data"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|d| {
            let t = wall(d["datetime"].as_str()?)? - 60;
            let p = &d["price"];
            bar(
                t,
                p["open"].as_f64(),
                p["high"].as_f64(),
                p["low"].as_f64(),
                p["close"].as_f64()?,
            )
        })
        .collect()
}

/// A bar, where Cboe's missing or zero prices (VIX's first minute has a
/// zero open and low) are taken from the others; none without a close.
fn bar(t: i64, o: Option<f64>, h: Option<f64>, l: Option<f64>, c: f64) -> Option<Value> {
    if c <= 0.0 {
        return None;
    }
    let ok = |x: Option<f64>| x.filter(|&x| x > 0.0);
    let o = ok(o).unwrap_or(c);
    let h = ok(h).unwrap_or(c).max(o).max(c);
    let l = ok(l).unwrap_or(c).min(o).min(c);
    Some(json!([t, o, h, l, c, 0]))
}

/// New York midnight of `YYYY-MM-DD`.
fn midnight(d: &str) -> Option<i64> {
    wall(&format!("{d}T00:00:00"))
}

/// A New York wall-clock `YYYY-MM-DDTHH:MM[:SS]` as Unix seconds.
fn wall(s: &str) -> Option<i64> {
    let p = |a: usize, b: usize| s.get(a..b)?.parse::<i64>().ok();
    Some(ny_to_unix(
        p(0, 4)?,
        p(5, 7)?,
        p(8, 10)?,
        p(11, 13)?,
        p(14, 16)?,
    ))
}

/// Minute bars combined into bars of `secs` (5, 15 or 60 minutes), each at
/// the start of its period; New York's offset is whole hours, so periods
/// line up with its clock.
#[must_use]
pub fn combine(bars: &[Value], secs: i64) -> Vec<Value> {
    let mut out: BTreeMap<i64, [f64; 4]> = BTreeMap::new();
    for b in bars {
        let (Some(t), Some(o), Some(h), Some(l), Some(c)) = (
            b[0].as_i64(),
            b[1].as_f64(),
            b[2].as_f64(),
            b[3].as_f64(),
            b[4].as_f64(),
        ) else {
            continue;
        };
        out.entry(t.div_euclid(secs) * secs)
            .and_modify(|x| {
                x[1] = x[1].max(h);
                x[2] = x[2].min(l);
                x[3] = c;
            })
            .or_insert([o, h, l, c]);
    }
    out.into_iter()
        .map(|(t, [o, h, l, c])| json!([t, o, h, l, c, 0]))
        .collect()
}

/// Daily bars combined into weeks, each at its Monday's midnight.
#[must_use]
pub fn weekly(days: &[Value]) -> Vec<Value> {
    let mut out: BTreeMap<i64, (i64, [f64; 4])> = BTreeMap::new();
    for b in days {
        let (Some(t), Some(o), Some(h), Some(l), Some(c)) = (
            b[0].as_i64(),
            b[1].as_f64(),
            b[2].as_f64(),
            b[3].as_f64(),
            b[4].as_f64(),
        ) else {
            continue;
        };
        // Days since the epoch at New York noon, then back to Monday
        // (1970-01-01 was a Thursday).
        let day = (t + 12 * 3600).div_euclid(86_400);
        let monday = day - (day + 3).rem_euclid(7);
        let (y, m, d) = civil(monday);
        out.entry(monday)
            .and_modify(|(_, x)| {
                x[1] = x[1].max(h);
                x[2] = x[2].min(l);
                x[3] = c;
            })
            .or_insert((ny_to_unix(y, m, d, 0, 0), [o, h, l, c]));
    }
    out.into_values()
        .map(|(t, [o, h, l, c])| json!([t, o, h, l, c, 0]))
        .collect()
}

fn civil(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (yoe + era * 400 + i64::from(m <= 2), m, d)
}

/// The day's bar from its minutes, when there are any.
#[must_use]
pub fn day_of(minutes: &[Value]) -> Option<Value> {
    let first = minutes.first()?;
    let date = ny_date_time(first[0].as_i64()?).0;
    let f = |i: usize| minutes.iter().filter_map(move |b| b[i].as_f64());
    Some(json!([
        midnight(&date)?,
        first[1],
        f(2).fold(f64::MIN, f64::max),
        f(3).fold(f64::MAX, f64::min),
        minutes.last()?[4],
        0
    ]))
}

/// Where a symbol's minutes are kept.
fn folder(dir: &Path, symbol: &str) -> PathBuf {
    dir.join("cboe").join(symbol)
}

/// Save minutes, merged with those already saved for their day (a later
/// fetch replaces a minute), one file per New York date.
pub fn save(dir: &Path, symbol: &str, minutes: &[Value]) -> std::io::Result<()> {
    let mut by: BTreeMap<String, Vec<&Value>> = BTreeMap::new();
    for b in minutes {
        if let Some(t) = b[0].as_i64() {
            by.entry(ny_date_time(t).0).or_default().push(b);
        }
    }
    let folder = folder(dir, symbol);
    std::fs::create_dir_all(&folder)?;
    for (date, new) in by {
        let path = folder.join(format!("{date}.json"));
        let mut all: BTreeMap<i64, Value> = load_file(&path)
            .into_iter()
            .filter_map(|b| Some((b[0].as_i64()?, b)))
            .collect();
        for b in new {
            if let Some(t) = b[0].as_i64() {
                all.insert(t, b.clone());
            }
        }
        let rows: Vec<Value> = all.into_values().collect();
        // Write beside, then rename, so a reader never sees half a file.
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec(&rows).unwrap_or_default())?;
        std::fs::rename(&tmp, &path)?;
    }
    Ok(())
}

/// Saved minutes from `from` (a New York date) on, oldest first.
#[must_use]
pub fn load(dir: &Path, symbol: &str, from: &str) -> Vec<Value> {
    let Ok(rd) = std::fs::read_dir(folder(dir, symbol)) else {
        return Vec::new();
    };
    let mut files: Vec<PathBuf> = rd
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| {
            p.extension().is_some_and(|x| x == "json")
                && p.file_stem()
                    .and_then(|s| s.to_str())
                    .is_some_and(|s| s >= from)
        })
        .collect();
    files.sort();
    files.iter().flat_map(|p| load_file(p)).collect()
}

/// The first date with saved minutes.
#[must_use]
pub fn first_saved(dir: &Path, symbol: &str) -> Option<String> {
    std::fs::read_dir(folder(dir, symbol))
        .ok()?
        .filter_map(Result::ok)
        .filter_map(|e| e.path().file_stem()?.to_str().map(str::to_owned))
        .filter(|s| s.len() == 10)
        .min()
}

fn load_file(path: &Path) -> Vec<Value> {
    std::fs::read(path)
        .ok()
        .and_then(|b| serde_json::from_slice::<Vec<Value>>(&b).ok())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_cboe_files() {
        let d = daily(&json!({ "data": [
            { "date": "2026-10-09", "volume": "0.0", "open": "778.64", "high": "782.06", "low": "777.93", "close": "781.15" },
        ] }));
        // New York midnight, daylight saving time: 04:00 UTC.
        assert_eq!(
            d,
            [json!([
                ny_to_unix(2026, 10, 9, 0, 0),
                778.64,
                782.06,
                777.93,
                781.15,
                0
            ])]
        );
        assert_eq!(d[0][0], 1_791_518_400);
        let m = minutes(&json!({ "data": [
            { "datetime": "2026-10-12T09:31:00", "price": { "open": 781.3, "high": 782.11, "low": 781.24, "close": 782.11 } },
        ] }));
        // Labelled by its end: the 09:30 minute.
        assert_eq!(ny_date_time(m[0][0].as_i64().unwrap()).1, "09:30");
        // VIX's first minute: zeros filled from the close.
        let v = minutes(&json!({ "data": [
            { "datetime": "2026-10-12T09:31:00", "price": { "open": 0.0, "high": 15.44, "low": 0.0, "close": 15.37 } },
            { "datetime": "2026-10-12T09:32:00", "price": { "open": 0.0, "high": 0.0, "low": 0.0, "close": 0.0 } },
        ] }));
        assert_eq!(v.len(), 1);
        assert_eq!(v[0], json!([m[0][0], 15.37, 15.44, 15.37, 15.37, 0]));
    }

    #[test]
    fn combines_bars() {
        let t0 = ny_to_unix(2026, 10, 12, 9, 30);
        let mins: Vec<Value> = (0..7)
            .map(|i| json!([t0 + i * 60, 10 + i, 20 + i, 5 - i, 11 + i, 0]))
            .collect();
        let five = combine(&mins, 300);
        assert_eq!(five.len(), 2);
        assert_eq!(five[0], json!([t0, 10.0, 24.0, 1.0, 15.0, 0]));
        assert_eq!(five[1], json!([t0 + 300, 15.0, 26.0, -1.0, 17.0, 0]));
        // The hour bar starts at 09:00, as Alpaca's do.
        assert_eq!(
            ny_date_time(combine(&mins, 3600)[0][0].as_i64().unwrap()).1,
            "09:00"
        );
        assert_eq!(
            day_of(&mins).unwrap(),
            json!([ny_to_unix(2026, 10, 12, 0, 0), 10, 26.0, -1.0, 17, 0])
        );
    }

    #[test]
    fn combines_weeks_from_monday() {
        // Thursday 8th, Friday 9th, Monday 12th October 2026.
        let day =
            |d: i64, c: f64| json!([ny_to_unix(2026, 10, d, 0, 0), c, c + 1.0, c - 1.0, c, 0]);
        let w = weekly(&[day(8, 1.0), day(9, 2.0), day(12, 3.0)]);
        assert_eq!(w.len(), 2);
        assert_eq!(
            w[0],
            json!([ny_to_unix(2026, 10, 5, 0, 0), 1.0, 3.0, 0.0, 2.0, 0])
        );
        assert_eq!(w[1][0], ny_to_unix(2026, 10, 12, 0, 0));
    }

    #[test]
    fn saves_and_loads_days() {
        let dir = std::env::temp_dir().join(format!("cboe-test-{}", std::process::id()));
        let t = ny_to_unix(2026, 10, 12, 9, 30);
        save(&dir, "XSP", &[json!([t, 1.0, 1.0, 1.0, 1.0, 0])]).unwrap();
        // A later fetch: the same minute revised, and the next.
        save(
            &dir,
            "XSP",
            &[
                json!([t, 2.0, 2.0, 2.0, 2.0, 0]),
                json!([t + 60, 3.0, 3.0, 3.0, 3.0, 0]),
            ],
        )
        .unwrap();
        let got = load(&dir, "XSP", "2026-10-01");
        assert_eq!(got.len(), 2);
        assert_eq!(got[0][1], 2.0);
        assert!(load(&dir, "XSP", "2026-10-13").is_empty());
        assert_eq!(first_saved(&dir, "XSP").as_deref(), Some("2026-10-12"));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}

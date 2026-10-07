//! Weekly oil and gas releases, by their published rules (category "oil"),
//! and the US federal holidays those rules move around:
//!
//! - API Weekly Statistical Bulletin (crude stocks, American Petroleum
//!   Institute): Tuesdays 16:30 ET; Wednesday when Monday is a holiday.
//! - EIA Weekly Petroleum Status Report: Wednesdays 10:30 ET; Thursday 11:00
//!   when Monday to Wednesday holds a holiday.
//! - EIA Weekly Natural Gas Storage Report: Thursdays 10:30 ET; Wednesday
//!   12:00 when Thursday is a holiday.
//! - Baker Hughes rig count: Fridays 13:00 ET; the last business day before
//!   when Friday is a holiday or the day after Thanksgiving.
//!
//! Rules, not the agencies' own calendars: an unusual week (a shutdown, a
//! one-off move) can differ.

use tinytrader_schedule::{ny_date_time, ny_to_unix};

use crate::sources::{Event, Importance};

/// Weekday of a date, 0 = Sunday.
fn weekday(y: i64, m: i64, d: i64) -> u8 {
    ny_date_time(ny_to_unix(y, m, d, 12, 0)).2
}

/// The `n`th (1-based) `wd` of a month, or with `n == 0` the last.
fn nth(y: i64, m: i64, wd: u8, n: i64) -> i64 {
    let days = if m == 12 {
        31
    } else {
        (ny_to_unix(y, m + 1, 1, 12, 0) - ny_to_unix(y, m, 1, 12, 0)) / 86_400
    };
    let first = (1..=7).find(|&d| weekday(y, m, d) == wd).unwrap_or(1);
    if n == 0 {
        let mut d = first;
        while d + 7 <= days {
            d += 7;
        }
        d
    } else {
        first + 7 * (n - 1)
    }
}

/// A fixed-date holiday as observed: a Saturday's on Friday, a Sunday's on Monday.
fn observed(y: i64, m: i64, d: i64) -> (i64, i64, i64) {
    let t = ny_to_unix(y, m, d, 12, 0);
    let shift = match weekday(y, m, d) {
        6 => -86_400,
        0 => 86_400,
        _ => 0,
    };
    let date = ny_date_time(t + shift).0;
    let p = |a: usize, b: usize| date[a..b].parse().unwrap_or(0);
    (p(0, 4), p(5, 7), p(8, 10))
}

/// The US federal holidays observed in a year, as (month, day); New Year's
/// Day of the next year can be observed on 31 December.
#[must_use]
pub fn federal_holidays(y: i64) -> Vec<(i64, i64)> {
    let fixed = [(1, 1), (6, 19), (7, 4), (11, 11), (12, 25)];
    let mut out: Vec<(i64, i64)> = fixed
        .iter()
        .map(|&(m, d)| observed(y, m, d))
        .chain(std::iter::once(observed(y + 1, 1, 1)))
        .filter(|o| o.0 == y)
        .map(|o| (o.1, o.2))
        .collect();
    out.extend([
        (1, nth(y, 1, 1, 3)),   // Martin Luther King Jr. Day
        (2, nth(y, 2, 1, 3)),   // Washington's Birthday
        (5, nth(y, 5, 1, 0)),   // Memorial Day
        (9, nth(y, 9, 1, 1)),   // Labor Day
        (10, nth(y, 10, 1, 2)), // Columbus Day
        (11, nth(y, 11, 4, 4)), // Thanksgiving
    ]);
    out.sort_unstable();
    out
}

/// The oil and gas releases over the days from `from` to `to` (Unix seconds).
#[must_use]
pub fn events(from: i64, to: i64) -> Vec<Event> {
    let mut out = Vec::new();
    let mut years: Vec<(i64, Vec<(i64, i64)>)> = Vec::new();
    let mut is_holiday = |t: i64| {
        let date = ny_date_time(t).0;
        let (y, m, d): (i64, i64, i64) = (
            date[0..4].parse().unwrap_or(0),
            date[5..7].parse().unwrap_or(0),
            date[8..10].parse().unwrap_or(0),
        );
        if !years.iter().any(|(yy, _)| *yy == y) {
            years.push((y, federal_holidays(y)));
        }
        years
            .iter()
            .find(|(yy, _)| *yy == y)
            .is_some_and(|(_, h)| h.contains(&(m, d)))
    };
    let at = |t: i64, h: i64, m: i64| {
        let date = ny_date_time(t).0;
        let p = |a: usize, b: usize| date[a..b].parse::<i64>().unwrap_or(0);
        ny_to_unix(p(0, 4), p(5, 7), p(8, 10), h, m)
    };
    let ev = |at: i64, title: &str, source: &'static str, importance| Event {
        at,
        title: title.into(),
        source,
        importance,
        category: "oil",
    };
    // Each Monday of the range, at noon New York time, and that week's days from it.
    let mut monday = from.div_euclid(86_400) * 86_400 + 12 * 3600;
    while ny_date_time(monday).2 != 1 {
        monday += 86_400;
    }
    while monday <= to + 7 * 86_400 {
        let day = |n: i64| monday + n * 86_400;
        let hol: Vec<bool> = (0..5).map(|n| is_holiday(day(n))).collect();
        // API: Tuesday 16:30, Wednesday after a Monday holiday.
        let api = if hol[0] {
            at(day(2), 16, 30)
        } else {
            at(day(1), 16, 30)
        };
        out.push(ev(
            api,
            "API weekly crude inventories",
            "API",
            Importance::Medium,
        ));
        // EIA petroleum: Wednesday 10:30, Thursday 11:00 after a holiday early in the week.
        let wpsr = if hol[0..3].iter().any(|h| *h) {
            at(day(3), 11, 0)
        } else {
            at(day(2), 10, 30)
        };
        out.push(ev(
            wpsr,
            "EIA Weekly Petroleum Status Report",
            "EIA",
            Importance::High,
        ));
        // EIA natural gas: Thursday 10:30, Wednesday 12:00 when Thursday is a holiday.
        let gas = if hol[3] {
            at(day(2), 12, 0)
        } else {
            at(day(3), 10, 30)
        };
        out.push(ev(
            gas,
            "EIA Weekly Natural Gas Storage Report",
            "EIA",
            Importance::Medium,
        ));
        // Baker Hughes: Friday 13:00, else the last business day before (Wednesday in Thanksgiving week).
        let rig_day = (0..5)
            .rev()
            .find(|&n| !(hol[n as usize] || (n == 4 && hol[3])))
            .unwrap_or(4);
        out.push(ev(
            at(day(rig_day), 13, 0),
            "Baker Hughes rig count",
            "Baker Hughes",
            Importance::Medium,
        ));
        monday += 7 * 86_400;
    }
    out.retain(|e| e.at >= from && e.at <= to);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn knows_the_federal_holidays() {
        let h = federal_holidays(2026);
        // 4 July 2026 is a Saturday: observed Friday the 3rd. Thanksgiving is the 26th.
        assert!(
            h.contains(&(7, 3))
                && h.contains(&(11, 26))
                && h.contains(&(9, 7))
                && h.contains(&(5, 25))
        );
        assert_eq!(h.len(), 11);
        // 1 January 2028 is a Saturday: observed on 31 December 2027.
        assert!(federal_holidays(2027).contains(&(12, 31)));
    }

    fn days(e: &[Event], title: &str) -> Vec<(String, String)> {
        e.iter()
            .filter(|x| x.title.starts_with(title))
            .map(|x| {
                let (d, t, _) = ny_date_time(x.at);
                (d, t)
            })
            .collect()
    }

    #[test]
    fn weekly_releases_move_for_holidays() {
        // The week of Columbus Day (Monday 12 October 2026) and the one after.
        let from = ny_to_unix(2026, 10, 12, 0, 0);
        let e = events(from, from + 14 * 86_400 - 1);
        assert_eq!(
            days(&e, "API"),
            [
                ("2026-10-14".into(), "16:30".into()),
                ("2026-10-20".into(), "16:30".into())
            ]
        );
        assert_eq!(
            days(&e, "EIA Weekly Petroleum"),
            [
                ("2026-10-15".into(), "11:00".into()),
                ("2026-10-21".into(), "10:30".into())
            ]
        );
        // Thanksgiving week: gas on Wednesday at noon, rigs on Wednesday.
        let from = ny_to_unix(2026, 11, 23, 0, 0);
        let e = events(from, from + 7 * 86_400 - 1);
        assert_eq!(
            days(&e, "EIA Weekly Natural Gas"),
            [("2026-11-25".into(), "12:00".into())]
        );
        assert_eq!(
            days(&e, "Baker Hughes"),
            [("2026-11-25".into(), "13:00".into())]
        );
        assert_eq!(e.iter().filter(|x| x.category == "oil").count(), 4);
    }
}

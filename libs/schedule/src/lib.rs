//! When a relay is active: a daily window in New York time on trading days.
//!
//! The window is set relative to the regular session (09:30 to 16:00), so an
//! early close ends it early by the same amount. Trading days and early
//! closes come from a market calendar (`{ "days": [{ "date", "open",
//! "close" }] }`, times in New York) when one is available; days beyond the
//! calendar's range count as trading days if they are weekdays.

use serde::Deserialize;

const NS: i64 = 1_000_000_000;
/// The regular session, in minutes after midnight, New York time.
const OPEN: i64 = 9 * 60 + 30;
const CLOSE: i64 = 16 * 60;

/// A daily window, in minutes after midnight New York time.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Schedule {
    pub start: i64,
    pub end: i64,
}

impl std::str::FromStr for Schedule {
    type Err = String;
    /// `HH:MM-HH:MM`, e.g. `09:25-16:20`.
    fn from_str(s: &str) -> Result<Self, String> {
        let bad = || format!("hours {s:?}: expected HH:MM-HH:MM");
        let (a, b) = s.split_once('-').ok_or_else(bad)?;
        let min = |t: &str| -> Result<i64, String> {
            let (h, m) = t.trim().split_once(':').ok_or_else(bad)?;
            let (h, m): (i64, i64) = (h.parse().map_err(|_| bad())?, m.parse().map_err(|_| bad())?);
            if h > 23 || m > 59 {
                return Err(bad());
            }
            Ok(h * 60 + m)
        };
        let (start, end) = (min(a)?, min(b)?);
        if start >= end {
            return Err(bad());
        }
        Ok(Self { start, end })
    }
}

/// A trading day from a market calendar.
#[derive(Clone, Debug, Deserialize)]
pub struct Day {
    pub date: String,
    pub open: String,
    pub close: String,
}

/// A market calendar: the trading days it covers.
#[derive(Clone, Debug, Default, Deserialize)]
pub struct Calendar {
    #[serde(default)]
    pub days: Vec<Day>,
}

impl Calendar {
    /// The session of a day: `Some((open, close))` in minutes for a trading
    /// day, `None` for a holiday or weekend, or the regular session for a
    /// weekday the calendar doesn't cover.
    fn session(&self, days_since_epoch: i64) -> Option<(i64, i64)> {
        let date = civil(days_since_epoch);
        if let Some(d) = self.days.iter().find(|d| d.date == date) {
            return Some((hhmm(&d.open)?, hhmm(&d.close)?));
        }
        let covered = |f: fn(&[Day]) -> Option<&Day>| f(&self.days).map(|d| d.date.as_str());
        let (first, last) = (covered(<[Day]>::first), covered(<[Day]>::last));
        let inside =
            matches!((first, last), (Some(a), Some(b)) if a <= date.as_str() && date.as_str() <= b);
        let weekday = (days_since_epoch + 3).rem_euclid(7) < 5; // 1970-01-01 was a Thursday
        (!inside && weekday).then_some((OPEN, CLOSE))
    }
}

fn hhmm(t: &str) -> Option<i64> {
    let (h, m) = t.split_once(':')?;
    Some(h.parse::<i64>().ok()? * 60 + m.parse::<i64>().ok()?)
}

impl Schedule {
    /// The window on one day, in ns since the epoch, if it is a trading day.
    fn window(&self, day: i64, calendar: &Calendar) -> Option<(u64, u64)> {
        let (open, close) = calendar.session(day)?;
        let start = open - (OPEN - self.start);
        let end = close + (self.end - CLOSE);
        let midnight_utc = day * 86_400 - ny_offset_secs(day);
        let at = |min: i64| u64::try_from((midnight_utc + min * 60) * NS).ok();
        Some((at(start)?, at(end)?))
    }

    /// The current or next window from `now` (ns since the epoch): `(start,
    /// end)`, where `start <= now` means the relay should be active now.
    #[must_use]
    pub fn next(&self, now: u64, calendar: &Calendar) -> Option<(u64, u64)> {
        let today = i64::try_from(now / 1_000_000_000).ok()? / 86_400;
        (today - 1..today + 15)
            .filter_map(|d| self.window(d, calendar))
            .find(|&(_, end)| end > now)
    }
}

/// New York's offset from
/// UTC on a day: -4 hours in daylight saving time (second Sunday in March to
/// first Sunday in November), else -5.
fn ny_offset_secs(day: i64) -> i64 {
    let (y, _, _) = ymd(day);
    let march = days_from_civil(y, 3, 1);
    let nov = days_from_civil(y, 11, 1);
    // First Sunday on or after a day; 1970-01-04 was a Sunday.
    let sunday = |d: i64| d + (7 - (d - 3).rem_euclid(7)) % 7;
    let dst_start = sunday(march) + 7;
    let dst_end = sunday(nov);
    if (dst_start..dst_end).contains(&day) {
        -4 * 3600
    } else {
        -5 * 3600
    }
}

fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * ((m + 9) % 12) + 2) / 5 + d - 1;
    era * 146_097 + yoe * 365 + yoe / 4 - yoe / 100 + doy - 719_468
}

fn ymd(days: i64) -> (i64, i64, i64) {
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

fn civil(days: i64) -> String {
    let (y, m, d) = ymd(days);
    format!("{y:04}-{m:02}-{d:02}")
}

/// A time as `Tue 09:25 ET`, for status messages.
#[must_use]
pub fn describe(ns: u64) -> String {
    let secs = i64::try_from(ns / 1_000_000_000).unwrap_or(0);
    let day = secs / 86_400;
    let local = secs + ny_offset_secs(day);
    let (lday, min) = (local.div_euclid(86_400), local.rem_euclid(86_400) / 60);
    let wd = ["Thu", "Fri", "Sat", "Sun", "Mon", "Tue", "Wed"][lday.rem_euclid(7) as usize];
    format!("{wd} {:02}:{:02} ET", min / 60, min % 60)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `YYYY-MM-DDTHH:MM:SSZ` as ns since the epoch.
    fn ns(text: &str) -> u64 {
        let n = |a: usize, b: usize| text[a..b].parse::<i64>().unwrap();
        let days = days_from_civil(n(0, 4), n(5, 7), n(8, 10));
        u64::try_from((days * 86_400 + n(11, 13) * 3600 + n(14, 16) * 60 + n(17, 19)) * NS).unwrap()
    }

    #[test]
    fn parses_hours() {
        assert_eq!(
            "09:25-16:20".parse::<Schedule>(),
            Ok(Schedule {
                start: 565,
                end: 980
            })
        );
        assert!("16:20-09:25".parse::<Schedule>().is_err());
        assert!("9-5".parse::<Schedule>().is_err());
    }

    #[test]
    fn windows_follow_new_york_time_and_daylight_saving() {
        let s: Schedule = "09:25-16:20".parse().unwrap();
        let none = Calendar::default();
        // Tuesday 2026-10-06, daylight saving: 09:25 ET is 13:25 UTC.
        let (a, b) = s.next(ns("2026-10-06T05:00:00Z"), &none).unwrap();
        assert_eq!(
            (a, b),
            (ns("2026-10-06T13:25:00Z"), ns("2026-10-06T20:20:00Z"))
        );
        // Monday 2026-12-07, standard time: 14:25 UTC.
        let (a, _) = s.next(ns("2026-12-07T05:00:00Z"), &none).unwrap();
        assert_eq!(a, ns("2026-12-07T14:25:00Z"));
        // Saturday evening: the next window is Monday's.
        let (a, _) = s.next(ns("2026-10-10T22:00:00Z"), &none).unwrap();
        assert_eq!(a, ns("2026-10-12T13:25:00Z"));
        // Inside a window: it started already and hasn't ended.
        let now = ns("2026-10-06T15:00:00Z");
        let (a, b) = s.next(now, &none).unwrap();
        assert!(a <= now && now < b);
        assert_eq!(describe(a), "Tue 09:25 ET");
    }

    #[test]
    fn calendars_skip_holidays_and_shorten_early_closes() {
        let s: Schedule = "09:25-16:20".parse().unwrap();
        let cal = Calendar {
            days: vec![
                Day {
                    date: "2026-11-25".into(),
                    open: "09:30".into(),
                    close: "16:00".into(),
                },
                // 26 November is Thanksgiving: absent.
                Day {
                    date: "2026-11-27".into(),
                    open: "09:30".into(),
                    close: "13:00".into(),
                },
            ],
        };
        let (a, b) = s.next(ns("2026-11-26T05:00:00Z"), &cal).unwrap();
        // Friday's early close at 13:00 ends the window at 13:20 ET (18:20 UTC).
        assert_eq!(
            (a, b),
            (ns("2026-11-27T14:25:00Z"), ns("2026-11-27T18:20:00Z"))
        );
    }
}

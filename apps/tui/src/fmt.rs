//! Numbers, times and events as the screens show them. Times are New York
//! time, as on the console.

use ratatui::style::Color;
use tinytrader_schedule::ny_date_time;

use crate::theme;

pub fn duration(s: f64) -> String {
    let s = s.max(0.0) as u64;
    let (d, h, m) = (s / 86_400, s % 86_400 / 3600, s % 3600 / 60);
    if d > 0 {
        format!("{d}d {h}h")
    } else if h > 0 {
        format!("{h}h {m}m")
    } else {
        format!("{m}m {}s", s % 60)
    }
}

/// `HH:MM:SS` New York time for a Unix time in milliseconds.
pub fn clock(ms: f64) -> String {
    let secs = (ms / 1000.0).floor() as i64;
    let (_, hm, _) = ny_date_time(secs);
    format!("{hm}:{:02}", secs.rem_euclid(60))
}

/// `HH:MM` New York time now, and today's date there.
pub fn now_ny() -> (String, String) {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64);
    let (date, hm, _) = ny_date_time(secs);
    (date, hm)
}

pub fn bytes(n: f64) -> String {
    let units = ["B", "KB", "MB", "GB", "TB"];
    let (mut n, mut i) = (n.max(0.0), 0);
    while n >= 1024.0 && i < units.len() - 1 {
        n /= 1024.0;
        i += 1;
    }
    if i == 0 {
        format!("{n:.0} {}", units[i])
    } else {
        format!("{n:.1} {}", units[i])
    }
}

/// `$12,345.67`, with a minus sign when negative.
pub fn money(v: f64) -> String {
    let cents = (v.abs() * 100.0).round() as u64;
    let (whole, frac) = (cents / 100, cents % 100);
    let digits = whole.to_string();
    let mut grouped = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            grouped.push(',');
        }
        grouped.push(c);
    }
    format!("{}${grouped}.{frac:02}", if v < 0.0 { "−" } else { "" })
}

pub fn signed_money(v: f64) -> String {
    if v > 0.0 {
        format!("+{}", money(v))
    } else {
        money(v)
    }
}

pub fn pct(v: f64) -> String {
    format!(
        "{}{:.2}%",
        if v > 0.0 {
            "+"
        } else if v < 0.0 {
            "−"
        } else {
            ""
        },
        v.abs() * 100.0
    )
}

/// A relay's or service's event summary, said plainly, with a colour for its
/// kind: the relay's status messages carry nanosecond times, which become
/// durations and New York times.
pub fn event(summary: &str) -> (String, Color) {
    let field = |k: &str| {
        summary
            .split_whitespace()
            .find_map(|w| w.strip_prefix(&format!("{k}=")))
            .and_then(|v| v.parse::<u64>().ok())
    };
    let at = |ns: u64| clock(ns as f64 / 1e6);
    if summary.starts_with("upstream-gap") {
        if let (Some(a), Some(b)) = (field("gap_from"), field("gap_to")) {
            let secs = b.abs_diff(a) as f64 / 1e9;
            return (
                format!(
                    "Databento skipped {secs:.2} s of records ({})",
                    at(a.min(b))
                ),
                theme::WARN,
            );
        }
        return ("Databento skipped records".into(), theme::WARN);
    }
    if summary.starts_with("upstream-down") {
        return (
            format!(
                "upstream down{}",
                field("since")
                    .map(|s| format!(" since {}", at(s)))
                    .unwrap_or_default()
            ),
            theme::CRIT,
        );
    }
    if summary.starts_with("upstream-up") {
        let gap = match (field("gap_from"), field("gap_to")) {
            (Some(a), Some(b)) => format!(", missed {:.1} s", b.abs_diff(a) as f64 / 1e9),
            _ => String::new(),
        };
        return (format!("upstream back{gap}"), theme::OK);
    }
    if summary.starts_with("off-schedule") {
        return (
            format!(
                "off schedule{}",
                field("until")
                    .map(|u| format!(" until {}", at(u)))
                    .unwrap_or_default()
            ),
            theme::MUTED,
        );
    }
    if summary.starts_with("on-schedule") {
        return ("on schedule".into(), theme::OK);
    }
    if summary.contains("disconnected")
        || summary.contains("fell behind")
        || summary.contains("stopped reading")
    {
        return (summary.to_owned(), theme::WARN);
    }
    if summary.contains("joined") {
        return (summary.to_owned(), theme::OK);
    }
    (summary.to_owned(), theme::FG_2)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_numbers_and_times() {
        assert_eq!(duration(3_725.0), "1h 2m");
        assert_eq!(duration(90_000.0), "1d 1h");
        assert_eq!(money(1_234_567.891), "$1,234,567.89");
        assert_eq!(money(-42.5), "−$42.50");
        assert_eq!(pct(0.0123), "+1.23%");
        // 2026-10-07 19:51:02 UTC is 15:51:02 in New York.
        assert_eq!(clock(1_791_402_662_000.0), "15:51:02");
    }

    #[test]
    fn says_relay_events_plainly() {
        let (t, c) = event("upstream-gap gap_from=1791402662557578805 gap_to=1791402663099803569");
        assert_eq!(t, "Databento skipped 0.54 s of records (15:51:02)");
        assert_eq!(c, theme::WARN);
        assert_eq!(event("client 1.2.3.4:5 joined").1, theme::OK);
    }
}

//! Where economic releases come from, and how each is read. Each source
//! yields events with a time; importance is given by name.

use tinytrader_schedule::{ny_date_time, ny_to_unix};

/// One scheduled release.
#[derive(Clone, Debug, PartialEq)]
pub struct Event {
    /// Unix seconds.
    pub at: i64,
    pub title: String,
    pub source: &'static str,
    pub importance: Importance,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Importance {
    Low,
    Medium,
    High,
}

impl Importance {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
        }
    }
    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "low" => Some(Self::Low),
            "medium" => Some(Self::Medium),
            "high" => Some(Self::High),
            _ => None,
        }
    }
}

/// How much a release moves markets, by its name: the few that move them
/// most are high; a handful more are medium; the rest are low.
#[must_use]
pub fn importance(title: &str) -> Importance {
    const HIGH: [&str; 8] = [
        "Consumer Price Index",
        "Employment Situation",
        "FOMC decision",
        "GDP (",
        "Gross Domestic Product",
        "Personal Income and Outlays",
        "Producer Price Index",
        "Job Openings and Labor Turnover Survey",
    ];
    const MEDIUM: [&str; 6] = [
        "Employment Cost Index",
        "Productivity and Costs,",
        "U.S. Import and Export Price Indexes",
        "U.S. International Trade in Goods and Services",
        "Initial jobless claims",
        "FOMC minutes",
    ];
    let starts = |list: &[&str]| list.iter().any(|p| title.starts_with(p));
    if title.starts_with("State ") || title.contains("by State") || title.contains("by Industry") {
        Importance::Low
    } else if starts(&HIGH) {
        Importance::High
    } else if starts(&MEDIUM) || title == "Productivity and Costs" {
        Importance::Medium
    } else {
        Importance::Low
    }
}

/// The events of an iCalendar file. Times are UTC (`...Z`) or New York local
/// (`TZID=US-Eastern` and the like; any `TZID` is taken as New York, which
/// both the BLS and BEA feeds use).
#[must_use]
pub fn ics(text: &str, source: &'static str) -> Vec<Event> {
    // Unfold continuation lines (RFC 5545: a line starting with a space continues the last).
    let mut lines: Vec<String> = Vec::new();
    for raw in text.split('\n') {
        let raw = raw.strip_suffix('\r').unwrap_or(raw);
        match raw.strip_prefix(' ').or_else(|| raw.strip_prefix('\t')) {
            Some(rest) if !lines.is_empty() => lines.last_mut().expect("not empty").push_str(rest),
            _ => lines.push(raw.to_owned()),
        }
    }
    let mut out = Vec::new();
    let (mut at, mut title, mut inside): (Option<i64>, Option<String>, bool) = (None, None, false);
    for line in &lines {
        match line.as_str() {
            "BEGIN:VEVENT" => (at, title, inside) = (None, None, true),
            "END:VEVENT" if inside => {
                if let (Some(at), Some(title)) = (at.take(), title.take()) {
                    let importance = importance(&title);
                    out.push(Event {
                        at,
                        title,
                        source,
                        importance,
                    });
                }
                inside = false;
            }
            _ if inside => {
                let Some((key, value)) = line.split_once(':') else {
                    continue;
                };
                let name = key.split(';').next().unwrap_or(key);
                if name == "DTSTART" {
                    at = ics_time(value);
                } else if name == "SUMMARY" {
                    title = Some(
                        value
                            .replace("\\,", ",")
                            .replace("\\;", ";")
                            .replace("\\n", " ")
                            .trim()
                            .to_owned(),
                    );
                }
            }
            _ => {}
        }
    }
    out
}

/// `20251107T083000` (New York) or `20251107T133000Z` (UTC) as Unix seconds.
fn ics_time(v: &str) -> Option<i64> {
    let n = |a: usize, b: usize| v.get(a..b)?.parse::<i64>().ok();
    let (y, mo, d) = (n(0, 4)?, n(4, 6)?, n(6, 8)?);
    let (h, mi) = if v.len() >= 13 {
        (n(9, 11)?, n(11, 13)?)
    } else {
        (0, 0)
    };
    if v.ends_with('Z') {
        Some(days_from_civil(y, mo, d) * 86_400 + h * 3600 + mi * 60)
    } else {
        Some(ny_to_unix(y, mo, d, h, mi))
    }
}

/// Days since 1970-01-01 of a date.
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * ((m + 9) % 12) + 2) / 5 + d - 1;
    era * 146_097 + yoe * 365 + yoe / 4 - yoe / 100 + doy - 719_468
}

const MONTHS: [&str; 12] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];

/// FOMC decisions from the Federal Reserve's meeting calendar page: each
/// meeting's last day at 14:00 New York time, marked when the meeting
/// publishes projections (`*` on the page).
#[must_use]
pub fn fomc(html: &str) -> Vec<Event> {
    let mut out = Vec::new();
    let mut year: Option<i64> = None;
    let mut month: Option<i64> = None;
    let mut at = 0;
    while at < html.len() {
        let rest = &html[at..];
        let next = [
            rest.find(" FOMC Meetings"),
            rest.find("fomc-meeting__month"),
            rest.find("fomc-meeting__date"),
        ]
        .into_iter()
        .flatten()
        .min();
        let Some(i) = next else { break };
        let here = &rest[i..];
        if here.starts_with(" FOMC Meetings") {
            // The year is the four digits just before.
            year = rest[..i]
                .get(i.saturating_sub(4)..)
                .and_then(|y| y.parse().ok());
        } else if here.starts_with("fomc-meeting__month") {
            let text = inner_text(here);
            // "Apr/May": the meeting ends in the second month.
            let name = text.rsplit('/').next().unwrap_or("").trim();
            month = MONTHS
                .iter()
                .position(|m| m.starts_with(name) && !name.is_empty())
                .map(|m| m as i64 + 1);
        } else if let (Some(y), Some(m)) = (year, month) {
            let text = inner_text(here);
            let projections = text.contains('*');
            let last_day = text
                .trim_end_matches('*')
                .rsplit('-')
                .next()
                .and_then(|d| d.trim().parse::<i64>().ok());
            if let Some(d) = last_day {
                out.push(Event {
                    at: ny_to_unix(y, m, d, 14, 0),
                    title: if projections {
                        "FOMC decision, with projections".into()
                    } else {
                        "FOMC decision".into()
                    },
                    source: "Fed",
                    importance: Importance::High,
                });
            }
            month = None;
        }
        at += i + 1;
    }
    out.sort_by_key(|e| e.at);
    out.dedup_by_key(|e| e.at);
    out
}

/// The text inside the element whose attributes start `here`: from the
/// first `>` to the next `</`, without tags.
fn inner_text(here: &str) -> String {
    let start = here.find('>').map_or(0, |i| i + 1);
    let end = here[start..].find("</d").map_or(here.len(), |e| start + e);
    let mut out = String::new();
    let mut tag = false;
    for c in here[start..end].chars() {
        match c {
            '<' => tag = true,
            '>' => tag = false,
            c if !tag => out.push(c),
            _ => {}
        }
    }
    out.trim().to_owned()
}

/// Weekly initial jobless claims (the Labor Department): Thursdays at 08:30
/// New York time, on Wednesday when the Thursday is a federal holiday
/// (Thanksgiving, Christmas, New Year's Day, Independence Day), over the
/// days from `from` to `to` (Unix seconds).
#[must_use]
pub fn claims(from: i64, to: i64) -> Vec<Event> {
    let mut out = Vec::new();
    let mut day = from.div_euclid(86_400);
    while day * 86_400 <= to {
        let (date, _, weekday) = ny_date_time(day * 86_400 + 12 * 3600);
        if weekday == 4 {
            let (y, m, d): (i64, i64, i64) = (
                date[0..4].parse().unwrap_or(0),
                date[5..7].parse().unwrap_or(0),
                date[8..10].parse().unwrap_or(0),
            );
            let thanksgiving = m == 11 && (22..=28).contains(&d);
            let holiday =
                thanksgiving || (m, d) == (12, 25) || (m, d) == (1, 1) || (m, d) == (7, 4);
            let at = ny_to_unix(y, m, d, 8, 30) - if holiday { 86_400 } else { 0 };
            out.push(Event {
                at,
                title: "Initial jobless claims".into(),
                source: "DOL",
                importance: Importance::Medium,
            });
        }
        day += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_bls_and_bea_calendars() {
        let bls = "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nUID:a\r\nDTSTART;TZID=US-Eastern:20261014T083000\r\nSUMMARY:Consumer Price Index\r\nEND:VEVENT\r\nBEGIN:VEVENT\r\nDTSTART;TZID=US-Eastern:20261021T100000\r\nSUMMARY:State Employment and \r\n Unemployment (Monthly)\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
        let e = ics(bls, "BLS");
        assert_eq!(e.len(), 2);
        assert_eq!(
            ny_date_time(e[0].at),
            ("2026-10-14".into(), "08:30".into(), 3)
        );
        assert_eq!(e[0].importance, Importance::High);
        assert_eq!(e[1].title, "State Employment and Unemployment (Monthly)");
        assert_eq!(e[1].importance, Importance::Low);
        let bea = "BEGIN:VEVENT\nSUMMARY:GDP (Advance Estimate)\\, 3rd Quarter 2026\nDTSTART;VALUE=DATE-TIME:20261029T123000Z\nEND:VEVENT\n";
        let e = ics(bea, "BEA");
        assert_eq!(e[0].title, "GDP (Advance Estimate), 3rd Quarter 2026");
        assert_eq!(ny_date_time(e[0].at).1, "08:30");
        assert_eq!(e[0].importance, Importance::High);
    }

    #[test]
    fn reads_the_fomc_page() {
        let html = r#"<h4><a id="1">2026 FOMC Meetings</a></h4>
          <div class="fomc-meeting__month col-xs-5"><strong>January</strong></div><div class="fomc-meeting__date col-xs-4">27-28</div>
          <div class="fomc-meeting__month col-xs-5"><strong>Apr/May</strong></div><div class="fomc-meeting__date col-xs-4">28-29*</div>"#;
        let e = fomc(html);
        assert_eq!(e.len(), 2);
        assert_eq!(
            ny_date_time(e[0].at),
            ("2026-01-28".into(), "14:00".into(), 3)
        );
        assert_eq!(e[1].title, "FOMC decision, with projections");
        assert_eq!(ny_date_time(e[1].at).0, "2026-05-29");
    }

    #[test]
    fn claims_are_thursdays_and_move_for_thanksgiving() {
        let from = ny_to_unix(2026, 11, 16, 0, 0);
        let e = claims(from, from + 14 * 86_400);
        let days: Vec<String> = e.iter().map(|x| ny_date_time(x.at).0).collect();
        // Thanksgiving 2026 is Thursday the 26th: claims come out on the 25th.
        assert_eq!(days, ["2026-11-19", "2026-11-25"]);
        assert_eq!(ny_date_time(e[0].at).1, "08:30");
    }
}

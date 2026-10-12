//! Headlines from RSS feeds (Bloomberg's, for one), as stories shaped like
//! Alpaca's: `id` (`rss:` and the item's guid or link), `headline`,
//! `summary`, `source` (the feed's site, e.g. `bloomberg`), `url`, no
//! `symbols`, and `created_at` from `pubDate` in UTC.

use serde_json::{Value, json};

/// The stories in an RSS document; items without a title, link or
/// readable date are left out.
pub fn parse(xml: &str, source: &str) -> Vec<Value> {
    xml.split("<item>")
        .skip(1)
        .filter_map(|rest| {
            let item = rest.split("</item>").next()?;
            let title = tag(item, "title")?;
            let link = tag(item, "link")?;
            let created = rfc3339(&tag(item, "pubDate")?)?;
            let id = tag(item, "guid").unwrap_or_else(|| link.clone());
            Some(json!({
                "id": format!("rss:{id}"),
                "headline": title,
                "summary": tag(item, "description").unwrap_or_default(),
                "author": tag(item, "dc:creator").unwrap_or_default(),
                "source": source,
                "url": link,
                "symbols": [],
                "created_at": created,
                "updated_at": created,
            }))
        })
        .collect()
}

/// The site a feed belongs to: `https://www.bloomberg.com/feeds/news.rss`
/// gives `bloomberg`.
pub fn source(url: &str) -> String {
    let host = url.split("://").nth(1).unwrap_or(url);
    let host = host.split(['/', ':']).next().unwrap_or(host);
    let labels: Vec<&str> = host.split('.').collect();
    let name = if labels.len() >= 2 {
        labels[labels.len() - 2]
    } else {
        host
    };
    name.to_lowercase()
}

/// The text of the first `<name ...>...</name>` in `s`, unwrapped from
/// CDATA and unescaped, or `None` when absent or empty.
fn tag(s: &str, name: &str) -> Option<String> {
    let open = format!("<{name}");
    let mut from = 0;
    let start = loop {
        let at = from + s[from..].find(&open)?;
        let after = at + open.len();
        // `<link` must not match `<linkage`.
        match s[after..].chars().next() {
            Some('>' | ' ' | '\t' | '\n' | '\r') => break after + s[after..].find('>')? + 1,
            _ => from = after,
        }
    };
    let end = start + s[start..].find(&format!("</{name}>"))?;
    let raw = s[start..end].trim();
    let text = match raw
        .strip_prefix("<![CDATA[")
        .and_then(|r| r.strip_suffix("]]>"))
    {
        Some(c) => c.to_owned(),
        None => unescape(raw),
    };
    let text = text.trim().to_owned();
    (!text.is_empty()).then_some(text)
}

fn unescape(s: &str) -> String {
    s.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&#39;", "'")
        .replace("&amp;", "&")
}

/// An RFC 822 date (`Mon, 12 Oct 2026 15:20:30 GMT`, or with `+0100`) as
/// `2026-10-12T15:20:30Z`.
fn rfc3339(date: &str) -> Option<String> {
    let parts: Vec<&str> = date.split_whitespace().collect();
    // The weekday is optional.
    let parts = if parts.first()?.ends_with(',') {
        &parts[1..]
    } else {
        &parts[..]
    };
    let [day, month, year, time, zone, ..] = parts else {
        return None;
    };
    let day: i64 = day.parse().ok()?;
    let month = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ]
    .iter()
    .position(|m| month.eq_ignore_ascii_case(m))? as i64
        + 1;
    let year: i64 = year.parse().ok()?;
    let hms: Vec<i64> = time
        .split(':')
        .map(|x| x.parse().ok())
        .collect::<Option<_>>()?;
    let (h, m, s) = match hms[..] {
        [h, m] => (h, m, 0),
        [h, m, s] => (h, m, s),
        _ => return None,
    };
    let offset = match *zone {
        "GMT" | "UT" | "UTC" | "Z" => 0,
        "EDT" => -4 * 60,
        "EST" | "CDT" => -5 * 60,
        "CST" | "MDT" => -6 * 60,
        "MST" | "PDT" => -7 * 60,
        "PST" => -8 * 60,
        _ => {
            let sign = match zone.as_bytes().first()? {
                b'+' => 1,
                b'-' => -1,
                _ => return None,
            };
            let digits = zone.get(1..5)?;
            sign * (digits[..2].parse::<i64>().ok()? * 60 + digits[2..].parse::<i64>().ok()?)
        }
    };
    let secs = days_from_civil(year, month, day) * 86_400 + h * 3600 + m * 60 + s - offset * 60;
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    let (y, mo, d) = civil_from_days(days);
    Some(format!(
        "{y:04}-{mo:02}-{d:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        rem / 60 % 60,
        rem % 60
    ))
}

// Howard Hinnant's days-from-civil and its inverse.
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (yoe + era * 400 + i64::from(m <= 2), m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    const FEED: &str = r#"<rss><channel><title><![CDATA[Bloomberg.com]]></title>
        <link>http://www.bloomberg.com/</link>
        <item>
            <title><![CDATA[Big Banks Set to Report Trading Haul of $19 Billion ]]></title>
            <description><![CDATA[Banks report Tuesday.]]></description>
            <link>https://www.bloomberg.com/news/videos/2026-10-12/big-banks</link>
            <guid isPermaLink="false">TOFZ4HT1UM0W00</guid>
            <dc:creator><![CDATA[Sridhar Natarajan]]></dc:creator>
            <pubDate>Mon, 12 Oct 2026 15:20:30 GMT</pubDate>
        </item>
        <item><title>Oil &amp; gas</title><link>https://x.com/a</link><pubDate>Sat, 31 Dec 2022 23:30:00 -0500</pubDate></item>
        <item><title>No date</title><link>https://x.com/b</link></item>
    </channel></rss>"#;

    #[test]
    fn items_become_stories() {
        let s = parse(FEED, "bloomberg");
        assert_eq!(s.len(), 2);
        assert_eq!(s[0]["id"], "rss:TOFZ4HT1UM0W00");
        assert_eq!(
            s[0]["headline"],
            "Big Banks Set to Report Trading Haul of $19 Billion"
        );
        assert_eq!(s[0]["author"], "Sridhar Natarajan");
        assert_eq!(s[0]["created_at"], "2026-10-12T15:20:30Z");
        assert_eq!(s[0]["source"], "bloomberg");
        assert_eq!(s[1]["headline"], "Oil & gas");
        assert_eq!(s[1]["id"], "rss:https://x.com/a");
        // An offset that crosses midnight and the year.
        assert_eq!(s[1]["created_at"], "2023-01-01T04:30:00Z");
    }

    #[test]
    fn names_the_site() {
        assert_eq!(
            source("https://www.bloomberg.com/feeds/news.rss"),
            "bloomberg"
        );
        assert_eq!(source("http://feeds.reuters.com:80/x"), "reuters");
    }
}

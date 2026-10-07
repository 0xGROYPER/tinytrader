//! Earnings report dates from three sources, compared.
//!
//! No source is authoritative: companies announce their own dates, and every
//! calendar aggregates them, often as estimates until the company confirms.
//! So each report is listed with the date each source gives, and is marked
//! disputed when they differ.
//!
//! - Alpha Vantage: one CSV of the next three months (`ALPHAVANTAGE_API_KEY`).
//! - Finnhub: JSON by date range, US companies on the free plan, with actual
//!   EPS once reported (`FINNHUB_API_KEY`).
//! - Nasdaq.com: the JSON behind its earnings calendar, one day at a time, with
//!   market capitalisation. Unofficial: it may change without notice.

use std::collections::BTreeMap;

use serde_json::Value;

/// Before the open, after the close, or not given.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum When {
    Before,
    After,
    Unknown,
}

impl When {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Before => "before-open",
            Self::After => "after-close",
            Self::Unknown => "unknown",
        }
    }
    fn parse(s: &str) -> Self {
        match s.trim() {
            "pre-market" | "time-pre-market" | "bmo" => Self::Before,
            "post-market" | "time-after-hours" | "amc" => Self::After,
            _ => Self::Unknown,
        }
    }
}

/// One source's line about one report.
#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    pub source: &'static str,
    pub symbol: String,
    pub name: Option<String>,
    /// YYYY-MM-DD.
    pub date: String,
    pub when: When,
    pub eps_estimate: Option<f64>,
    pub eps_actual: Option<f64>,
    /// US dollars.
    pub market_cap: Option<f64>,
}

/// One report, as the sources together describe it.
#[derive(Clone, Debug, PartialEq)]
pub struct Report {
    pub symbol: String,
    pub name: Option<String>,
    /// The date most sources give (ties: Nasdaq, then Alpha Vantage, then Finnhub).
    pub date: String,
    pub when: When,
    pub eps_estimate: Option<f64>,
    pub eps_actual: Option<f64>,
    pub market_cap: Option<f64>,
    /// Each source's date for this report.
    pub dates: BTreeMap<&'static str, String>,
    /// The sources give different dates.
    pub disputed: bool,
}

pub const ALPHA_VANTAGE: &str = "Alpha Vantage";
pub const FINNHUB: &str = "Finnhub";
pub const NASDAQ: &str = "Nasdaq";

fn rank(source: &str) -> u8 {
    match source {
        NASDAQ => 0,
        ALPHA_VANTAGE => 1,
        _ => 2,
    }
}

/// Split one CSV line, honouring double quotes.
fn csv_fields(line: &str) -> Vec<String> {
    let (mut out, mut cur, mut quoted) = (Vec::new(), String::new(), false);
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' if quoted && chars.peek() == Some(&'"') => {
                cur.push('"');
                chars.next();
            }
            '"' => quoted = !quoted,
            ',' if !quoted => out.push(std::mem::take(&mut cur)),
            _ => cur.push(c),
        }
    }
    out.push(cur);
    out
}

/// Alpha Vantage's EARNINGS_CALENDAR CSV.
///
/// # Errors
/// When it isn't that CSV (Alpha Vantage answers errors and rate limits as
/// JSON or a note).
pub fn alpha_vantage(text: &str) -> Result<Vec<Row>, String> {
    let mut lines = text.lines();
    let header = csv_fields(lines.next().unwrap_or_default());
    let col = |name: &str| header.iter().position(|h| h.trim() == name);
    let (Some(sym), Some(name), Some(date), Some(est), Some(time)) = (
        col("symbol"),
        col("name"),
        col("reportDate"),
        col("estimate"),
        col("timeOfTheDay"),
    ) else {
        return Err(format!(
            "not the earnings CSV: {}",
            text.chars().take(120).collect::<String>()
        ));
    };
    Ok(lines
        .map(csv_fields)
        .filter(|f| f.len() > sym.max(name).max(date).max(est).max(time))
        .map(|f| Row {
            source: ALPHA_VANTAGE,
            symbol: f[sym].trim().to_owned(),
            name: Some(f[name].trim().to_owned()).filter(|n| !n.is_empty()),
            date: f[date].trim().to_owned(),
            when: When::parse(&f[time]),
            eps_estimate: f[est].trim().parse().ok(),
            eps_actual: None,
            market_cap: None,
        })
        .filter(|r| !r.symbol.is_empty() && r.date.len() == 10)
        .collect())
}

/// Finnhub's `/calendar/earnings` answer.
///
/// # Errors
/// When it has no `earningsCalendar` list.
pub fn finnhub(v: &Value) -> Result<Vec<Row>, String> {
    let list = v["earningsCalendar"].as_array().ok_or_else(|| {
        format!(
            "unexpected answer: {}",
            v.to_string().chars().take(120).collect::<String>()
        )
    })?;
    Ok(list
        .iter()
        .filter_map(|r| {
            Some(Row {
                source: FINNHUB,
                symbol: r["symbol"].as_str()?.to_owned(),
                name: None,
                date: r["date"].as_str()?.to_owned(),
                when: When::parse(r["hour"].as_str().unwrap_or("")),
                eps_estimate: r["epsEstimate"].as_f64(),
                eps_actual: r["epsActual"].as_f64(),
                market_cap: None,
            })
        })
        .collect())
}

/// "$706,894,560,000" or "$12.47" as a number.
fn dollars(s: &str) -> Option<f64> {
    let t: String = s
        .chars()
        .filter(|c| c.is_ascii_digit() || *c == '.' || *c == '-')
        .collect();
    let n: f64 = t.parse().ok()?;
    Some(if s.trim_start().starts_with('(') {
        -n
    } else {
        n
    })
}

/// Nasdaq.com's earnings calendar for one day (`date`, YYYY-MM-DD).
///
/// # Errors
/// When the answer has no `data` (Nasdaq reports problems in `status`).
pub fn nasdaq(v: &Value, date: &str) -> Result<Vec<Row>, String> {
    let data = v["data"].as_object().ok_or_else(|| {
        format!(
            "unexpected answer: {}",
            v["status"]
                .to_string()
                .chars()
                .take(120)
                .collect::<String>()
        )
    })?;
    // A day without reports has `rows: null`.
    Ok(data
        .get("rows")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|r| {
            Some(Row {
                source: NASDAQ,
                symbol: r["symbol"].as_str()?.to_owned(),
                name: r["name"].as_str().map(str::to_owned),
                date: date.to_owned(),
                when: When::parse(r["time"].as_str().unwrap_or("")),
                eps_estimate: r["epsForecast"].as_str().and_then(dollars),
                eps_actual: None,
                market_cap: r["marketCap"].as_str().and_then(dollars),
            })
        })
        .collect())
}

/// Days between two YYYY-MM-DD dates (approximate is enough for grouping).
fn day_number(d: &str) -> i64 {
    let p = |a: usize, b: usize| d.get(a..b).and_then(|s| s.parse::<i64>().ok()).unwrap_or(0);
    p(0, 4) * 372 + p(5, 7) * 31 + p(8, 10)
}

/// Combine every source's rows into reports: a company's rows within ten
/// days of each other describe one report.
#[must_use]
pub fn merge(mut rows: Vec<Row>) -> Vec<Report> {
    rows.sort_by(|a, b| (&a.symbol, &a.date).cmp(&(&b.symbol, &b.date)));
    let mut out = Vec::new();
    let mut group: Vec<Row> = Vec::new();
    let flush = |group: &mut Vec<Row>, out: &mut Vec<Report>| {
        if group.is_empty() {
            return;
        }
        let mut dates: BTreeMap<&'static str, String> = BTreeMap::new();
        for r in group.iter() {
            dates.entry(r.source).or_insert_with(|| r.date.clone());
        }
        // The date most sources give; ties go to the better-ranked source.
        let mut votes: BTreeMap<&str, (usize, u8)> = BTreeMap::new();
        for (src, d) in &dates {
            let v = votes.entry(d.as_str()).or_insert((0, u8::MAX));
            v.0 += 1;
            v.1 = v.1.min(rank(src));
        }
        let date = votes
            .iter()
            .max_by(|a, b| {
                (a.1.0, std::cmp::Reverse(a.1.1)).cmp(&(b.1.0, std::cmp::Reverse(b.1.1)))
            })
            .map(|(d, _)| (*d).to_owned())
            .unwrap_or_default();
        let mut by_rank: Vec<&Row> = group.iter().collect();
        by_rank.sort_by_key(|r| rank(r.source));
        let first = |f: &dyn Fn(&Row) -> Option<f64>| by_rank.iter().find_map(|r| f(r));
        out.push(Report {
            symbol: group[0].symbol.clone(),
            name: by_rank.iter().find_map(|r| r.name.clone()),
            when: by_rank
                .iter()
                .map(|r| r.when)
                .find(|w| *w != When::Unknown)
                .unwrap_or(When::Unknown),
            eps_estimate: first(&|r| r.eps_estimate),
            eps_actual: first(&|r| r.eps_actual),
            market_cap: first(&|r| r.market_cap),
            disputed: votes.len() > 1,
            date,
            dates,
        });
        group.clear();
    };
    for r in rows {
        let same = group.last().is_some_and(|g| {
            g.symbol == r.symbol && day_number(&r.date) - day_number(&g.date) <= 10
        });
        if !same {
            flush(&mut group, &mut out);
        }
        group.push(r);
    }
    flush(&mut group, &mut out);
    out.sort_by(|a, b| (&a.date, &a.symbol).cmp(&(&b.date, &b.symbol)));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn reads_each_source() {
        let av = alpha_vantage(
            "symbol,name,reportDate,fiscalDateEnding,estimate,currency,timeOfTheDay\r\n\
             BAC,\"BANK OF AMERICA, CORP\",2026-10-14,2026-09-30,1.16,USD,pre-market\r\n",
        )
        .unwrap();
        assert_eq!(av[0].name.as_deref(), Some("BANK OF AMERICA, CORP"));
        assert_eq!((av[0].when, av[0].eps_estimate), (When::Before, Some(1.16)));
        assert!(alpha_vantage("{\"Information\": \"rate limit\"}").is_err());

        let fh = finnhub(&json!({ "earningsCalendar": [{ "symbol": "MS", "date": "2026-10-14", "hour": "bmo", "epsEstimate": 3.17, "epsActual": null }] })).unwrap();
        assert_eq!((fh[0].when, fh[0].eps_actual), (When::Before, None));

        let nq = nasdaq(&json!({ "data": { "rows": [{ "symbol": "ASML", "name": "ASML Holding N.V.", "time": "time-pre-market", "marketCap": "$706,894,560,000", "epsForecast": "$12.47" }] } }), "2026-10-14").unwrap();
        assert_eq!(nq[0].market_cap, Some(706_894_560_000.0));
        assert_eq!(nq[0].eps_estimate, Some(12.47));
        assert!(
            nasdaq(&json!({ "data": { "rows": null } }), "2026-10-17")
                .unwrap()
                .is_empty()
        );
    }

    fn row(source: &'static str, symbol: &str, date: &str) -> Row {
        Row {
            source,
            symbol: symbol.into(),
            name: None,
            date: date.into(),
            when: When::Unknown,
            eps_estimate: None,
            eps_actual: None,
            market_cap: None,
        }
    }

    #[test]
    fn merges_by_company_and_marks_disagreements() {
        let reports = merge(vec![
            row(ALPHA_VANTAGE, "EQBK", "2026-10-13"),
            row(FINNHUB, "EQBK", "2026-10-14"),
            row(NASDAQ, "EQBK", "2026-10-14"),
            row(ALPHA_VANTAGE, "BAC", "2026-10-14"),
            row(NASDAQ, "BAC", "2026-10-14"),
            // A later quarter is a separate report.
            row(ALPHA_VANTAGE, "BAC", "2027-01-15"),
        ]);
        assert_eq!(reports.len(), 3);
        let eqbk = reports.iter().find(|r| r.symbol == "EQBK").unwrap();
        assert_eq!(eqbk.date, "2026-10-14");
        assert!(eqbk.disputed);
        assert_eq!(eqbk.dates[ALPHA_VANTAGE], "2026-10-13");
        let bac: Vec<_> = reports.iter().filter(|r| r.symbol == "BAC").collect();
        assert!(!bac[0].disputed && bac[0].dates.len() == 2);
        assert_eq!(bac[1].date, "2027-01-15");
        // A two-way tie goes to Nasdaq.
        let tie = merge(vec![
            row(ALPHA_VANTAGE, "X", "2026-10-13"),
            row(NASDAQ, "X", "2026-10-15"),
        ]);
        assert_eq!(tie[0].date, "2026-10-15");
    }
}

//! Mechanical safety: caps profile, request budget, civil/ET time and the expiry-day
//! opening cutoff. None of this is portfolio or strategy state.
use crate::intent::{Intent, fmt_cents, occ};
use crate::protocol::Obj;
use std::time::{Duration, Instant};

const NS: i64 = 1_000_000_000;

/// Operator profile: allowlist and per-account caps (section 8).
#[derive(Clone, Debug)]
pub struct Profile {
    /// Allowed option roots.
    pub underlyings: Vec<String>,
    /// Allowed stocks (single orders, `day` or `cls`); none by default.
    pub equities: Vec<String>,
    /// Whether a stock may be sold to open (a short sale).
    pub allow_short: bool,
    /// Shares per stock order.
    pub max_shares: u32,
    /// Maximum combo legs.
    pub max_legs: usize,
    /// Contracts (parent units) per order.
    pub max_qty: u32,
    /// Working orders, including unknown outcomes.
    pub max_open: usize,
    /// |limit| × qty × the multiplier (100 an option contract, 1 a share), in cents.
    pub max_notional_cents: i64,
    /// Admitted submits in any 60 s.
    pub submits_per_min: usize,
    /// Admitted submits per UTC day.
    pub submits_per_day: usize,
    /// Consecutive broker rejects that trip `halt_opens`.
    pub max_rejects: u32,
    /// Requests per minute for this account process.
    pub budget_per_min: u32,
    /// Minimum `options_trading_level` for the start gate.
    pub min_options_level: u32,
}

impl Default for Profile {
    fn default() -> Self {
        Self {
            underlyings: vec!["SPY".into()],
            equities: Vec::new(),
            allow_short: false,
            max_shares: 1_000,
            max_legs: 4,
            max_qty: 10,
            max_open: 4,
            max_notional_cents: 250_000,
            submits_per_min: 60,
            submits_per_day: 500,
            max_rejects: 5,
            budget_per_min: 150,
            min_options_level: 3,
        }
    }
}

impl Profile {
    pub(crate) fn json(&self) -> String {
        let u = crate::protocol::arr(self.underlyings.iter().map(|s| crate::protocol::quote(s)));
        let e = crate::protocol::arr(self.equities.iter().map(|s| crate::protocol::quote(s)));
        Obj::new()
            .raw("underlyings", &u)
            .raw("equities", &e)
            .s("allow_short", if self.allow_short { "yes" } else { "no" })
            .n("max_shares", self.max_shares)
            .n("max_legs", self.max_legs)
            .n("max_qty", self.max_qty)
            .n("max_open", self.max_open)
            .s("max_notional", &fmt_cents(self.max_notional_cents))
            .n("submits_per_min", self.submits_per_min)
            .n("submits_per_day", self.submits_per_day)
            .n("max_rejects", self.max_rejects)
            .n("budget_per_min", self.budget_per_min)
            .n("min_options_level", self.min_options_level)
            .done()
    }
}

/// One token bucket per account with critical and poll priorities; polls keep a 20%
/// reserve for critical requests and are shed first.
pub(crate) struct Budget {
    per_min: f64,
    tokens: f64,
    last: Instant,
    pub(crate) blocked_until: Option<Instant>,
}

impl Budget {
    pub(crate) fn new(per_min: u32) -> Self {
        Self {
            per_min: f64::from(per_min),
            tokens: f64::from(per_min),
            last: Instant::now(),
            blocked_until: None,
        }
    }
    fn refill(&mut self) {
        let now = Instant::now();
        let gained = now.duration_since(self.last).as_secs_f64() * self.per_min / 60.0;
        self.tokens = (self.tokens + gained).min(self.per_min);
        self.last = now;
    }
    pub(crate) fn blocked(&mut self) -> bool {
        self.blocked_until = self.blocked_until.filter(|t| *t > Instant::now());
        self.blocked_until.is_some()
    }
    /// Takes one token; `false` means throttled before any send.
    pub(crate) fn take(&mut self, critical: bool) -> bool {
        self.refill();
        let floor = if critical {
            1.0
        } else {
            1.0 + self.per_min * 0.2
        };
        if self.blocked() || self.tokens < floor {
            return false;
        }
        self.tokens -= 1.0;
        true
    }
    /// Charges reconciliation, which is never shed (the start gate depends on it).
    pub(crate) fn charge(&mut self) {
        self.refill();
        self.tokens = (self.tokens - 1.0).max(0.0);
    }
    /// After a 429: `blocked_until = max(Retry-After, reset, backoff)`.
    pub(crate) fn block(&mut self, ms: Option<u64>) {
        let until = Instant::now() + Duration::from_millis(ms.unwrap_or(0).max(1000));
        self.blocked_until = Some(self.blocked_until.map_or(until, |b| b.max(until)));
    }
}

/// Days since 1970-01-01 of a proleptic Gregorian date.
pub(crate) fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (i64::from(m) + 9) % 12;
    let doy = (153 * mp + 2) / 5 + i64::from(d) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

pub(crate) fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = u32::try_from(doy - (153 * mp + 2) / 5 + 1).unwrap_or(1);
    let m = u32::try_from(if mp < 10 { mp + 3 } else { mp - 9 }).unwrap_or(1);
    (
        if m <= 2 {
            yoe + era * 400 + 1
        } else {
            yoe + era * 400
        },
        m,
        d,
    )
}

pub(crate) fn date_string(days: i64) -> String {
    let (y, m, d) = civil_from_days(days);
    format!("{y:04}-{m:02}-{d:02}")
}

#[expect(clippy::many_single_char_names, reason = "civil date arithmetic")]
pub(crate) fn parse_date(s: &str) -> Option<i64> {
    let b = s.as_bytes();
    if b.len() != 10 || b[4] != b'-' || b[7] != b'-' {
        return None;
    }
    let (y, m, d) = (
        s[..4].parse().ok()?,
        s[5..7].parse().ok()?,
        s[8..].parse().ok()?,
    );
    let days = days_from_civil(y, m, d);
    (civil_from_days(days) == (y, m, d)).then_some(days)
}

/// Minutes after midnight of an `HH:MM` string.
pub(crate) fn hhmm(s: &str) -> Option<i64> {
    let (h, m) = s.split_once(':')?;
    let (h, m): (i64, i64) = (h.parse().ok()?, m.parse().ok()?);
    (s.len() == 5 && h < 24 && m < 60).then_some(h * 60 + m)
}

/// `America/New_York` UTC offset in seconds at a UTC instant (US rules since 2007).
fn et_offset(utc_secs: i64) -> i64 {
    let (y, _, _) = civil_from_days(utc_secs.div_euclid(86_400));
    let sunday_on_or_after = |d: i64| d + (7 - (d + 4).rem_euclid(7)) % 7;
    let start = (sunday_on_or_after(days_from_civil(y, 3, 1)) + 7) * 86_400 + 7 * 3600;
    let end = sunday_on_or_after(days_from_civil(y, 11, 1)) * 86_400 + 6 * 3600;
    if (start..end).contains(&utc_secs) {
        -4 * 3600
    } else {
        -5 * 3600
    }
}

fn ns_secs(ts: u64) -> i64 {
    i64::try_from(ts / 1_000_000_000).unwrap_or(i64::MAX)
}

/// The `America/New_York` calendar day (days since the epoch) at `ts` (ns).
pub(crate) fn et_day(ts: u64) -> i64 {
    let s = ns_secs(ts);
    (s + et_offset(s)).div_euclid(86_400)
}

/// UTC nanoseconds of `minutes` after local midnight on ET day `day`.
pub(crate) fn et_ns(day: i64, minutes: i64) -> u64 {
    let local = day * 86_400 + minutes * 60;
    u64::try_from((local - et_offset(local + 5 * 3600)) * NS).unwrap_or(0)
}

/// Section 9 opening cutoff. `close` gives the session close (minutes ET) of a day, or
/// `None` when the calendar is unknown. Returns the `not_sent{cutoff}` detail.
pub(crate) fn cutoff(
    intent: &Intent,
    now: Option<u64>,
    close: &mut dyn FnMut(i64) -> Option<i64>,
) -> Result<(), String> {
    // Same-day option expiries only; stocks don't expire.
    for leg in intent.legs.iter().filter(|l| l.open && !l.equity) {
        let (root, expiry) = occ(&leg.symbol).unwrap_or(("", 0));
        let Some(now) = now else {
            return Err(Obj::new()
                .s("leg", &leg.symbol)
                .s("unknown", "clock")
                .done());
        };
        let today = et_day(now);
        if expiry != today {
            continue;
        }
        let Some(close) = close(today) else {
            return Err(Obj::new()
                .s("leg", &leg.symbol)
                .s("unknown", "calendar")
                .done());
        };
        let guard_s = if matches!(root, "SPY" | "QQQ") {
            30 * 60 + 30
        } else {
            45 * 60 + 30
        };
        let at = close * 60 - guard_s;
        if now >= et_ns(today, 0) + u64::try_from(at).unwrap_or(0) * 1_000_000_000 {
            let cut = format!("{:02}:{:02}:{:02}", at / 3600, at / 60 % 60, at % 60);
            return Err(Obj::new()
                .s("expiry", &date_string(expiry))
                .s("cutoff_et", &cut)
                .s("leg", &leg.symbol)
                .done());
        }
    }
    Ok(())
}

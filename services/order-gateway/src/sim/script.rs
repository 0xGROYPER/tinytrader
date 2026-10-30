//! Scripted simulator input: FakeBroker-style per-submit rules, account fields,
//! preloaded broker state, the clock/calendar and the model profile.
use crate::broker::SessionDay;
use crate::intent::parse_cents;
use crate::protocol::{Fields, Invalid, invalid};
use crate::safety::{hhmm, parse_date};
use serde_json::Value;
use std::collections::BTreeMap;

/// Programmed submit outcome.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) enum OnSubmit {
    #[default]
    Accept,
    Fill,
    Partial(u64),
    Reject(u16, String, String),
    ErrorBeforeSend,
    TimeoutAfterAccept,
    Http429(Option<u64>),
    Stall,
}

/// Programmed cancel outcome.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum OnCancel {
    #[default]
    Ok,
    Ignore,
    Reject,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct Rule {
    pub(crate) submit: OnSubmit,
    pub(crate) cancel: OnCancel,
    pub(crate) fill_on_query: bool,
    pub(crate) skew: BTreeMap<String, u64>,
}

pub(crate) enum Match {
    Nth(u64),
    Id(String),
}

pub(crate) struct Preload {
    pub(crate) client_order_id: String,
    pub(crate) symbol: String,
    pub(crate) buy: bool,
    pub(crate) qty: u64,
    pub(crate) limit_cents: i64,
    pub(crate) status: String,
}

/// The parsed script; every field has a default, so `{}` is a valid script.
pub(crate) struct Script {
    pub(crate) clock_ts: Option<u64>,
    /// `None`: no calendar (session checks fail closed).
    pub(crate) calendar: Option<Vec<SessionDay>>,
    pub(crate) weekdays: bool,
    pub(crate) status: String,
    pub(crate) level: u32,
    pub(crate) cash_cents: i64,
    pub(crate) trading_blocked: bool,
    pub(crate) positions: BTreeMap<String, i64>,
    pub(crate) orders: Vec<Preload>,
    pub(crate) quotes: BTreeMap<String, (Option<i64>, Option<i64>)>,
    pub(crate) default: Rule,
    pub(crate) rules: Vec<(Match, Rule)>,
    pub(crate) drop_after: Option<(u64, u64)>,
    pub(crate) fee_cents: i64,
    pub(crate) latency_ms: u64,
    pub(crate) strategy_latency_ms: u64,
    pub(crate) size_cap: bool,
}

fn price(v: &Value, path: &str) -> Result<i64, Invalid> {
    v.as_str()
        .and_then(parse_cents)
        .ok_or_else(|| invalid(path, "decimal price"))
}

#[expect(clippy::many_single_char_names, reason = "flat rule grammar")]
fn rule(v: Value, path: &str) -> Result<(Option<Match>, Rule), Invalid> {
    let mut f = Fields::new(v, path)?;
    let m = match (f.opt_int("nth")?, f.opt_string("client_order_id")?) {
        (Some(n), None) => Some(Match::Nth(n)),
        (None, Some(id)) => Some(Match::Id(id)),
        (None, None) => None,
        _ => return Err(invalid(path, "nth or client_order_id, not both")),
    };
    let mut r = Rule::default();
    if let Some(s) = f.take("submit") {
        let p = f.at("submit");
        r.submit = match s {
            Value::String(s) => match s.as_str() {
                "accept" => OnSubmit::Accept,
                "fill" => OnSubmit::Fill,
                "error_before_send" => OnSubmit::ErrorBeforeSend,
                "timeout_after_accept" => OnSubmit::TimeoutAfterAccept,
                "stall" => OnSubmit::Stall,
                _ => return Err(invalid(&p, "unknown submit outcome")),
            },
            obj => {
                let mut o = Fields::new(obj, &p)?;
                let out = if let Some(n) = o.opt_int("partial")? {
                    OnSubmit::Partial(n)
                } else if let Some(rej) = o.take("reject") {
                    let mut j = Fields::new(rej, &o.at("reject"))?;
                    let http = u16::try_from(j.int("http", 400, 599)?).unwrap_or(422);
                    let code = j.int("code", 0, u64::MAX)?.to_string();
                    let out = OnSubmit::Reject(http, code, j.string("message")?);
                    j.finish()?;
                    out
                } else if let Some(h) = o.take("http_429") {
                    let mut j = Fields::new(h, &o.at("http_429"))?;
                    let mut retry = None;
                    if let Some(Value::Object(hdr)) = j.take("headers") {
                        for (k, v) in hdr {
                            let secs = v.as_str().and_then(|s| s.parse::<u64>().ok());
                            if k.eq_ignore_ascii_case("retry-after") {
                                retry = secs.map(|s| s * 1000);
                            }
                        }
                    }
                    j.finish()?;
                    OnSubmit::Http429(retry)
                } else {
                    return Err(invalid(&p, "partial, reject or http_429"));
                };
                o.finish()?;
                out
            }
        };
    }
    if let Some(c) = f.opt_string("cancel")? {
        r.cancel = match c.as_str() {
            "ok" => OnCancel::Ok,
            "ignore" => OnCancel::Ignore,
            "reject" => OnCancel::Reject,
            _ => return Err(invalid(&f.at("cancel"), "ok, ignore or reject")),
        };
    }
    r.fill_on_query = f.opt_bool("fill_on_query")?.unwrap_or(false);
    if let Some(Value::Object(skew)) = f.take("skew") {
        for (k, v) in skew {
            r.skew
                .insert(k, v.as_u64().ok_or_else(|| invalid(path, "skew counts"))?);
        }
    }
    f.finish()?;
    Ok((m, r))
}

impl Script {
    pub(crate) fn parse(text: &str) -> Result<Self, String> {
        let v: Value = serde_json::from_str(text).map_err(|e| format!("sim script: {e}"))?;
        Self::from_value(v).map_err(|e| format!("sim script {}: {}", e.path, e.message))
    }

    #[expect(clippy::many_single_char_names, reason = "flat script grammar")]
    #[expect(clippy::too_many_lines, reason = "flat script grammar")]
    fn from_value(v: Value) -> Result<Self, Invalid> {
        let mut f = Fields::new(v, "")?;
        let mut s = Self {
            clock_ts: f.opt_int("clock_ts")?,
            calendar: Some(Vec::new()),
            weekdays: true,
            status: "ACTIVE".into(),
            level: 3,
            cash_cents: 200_000,
            trading_blocked: false,
            positions: BTreeMap::new(),
            orders: Vec::new(),
            quotes: BTreeMap::new(),
            default: Rule::default(),
            rules: Vec::new(),
            drop_after: None,
            fee_cents: 2,
            latency_ms: f.opt_int("latency_ms")?.unwrap_or(5),
            strategy_latency_ms: f.opt_int("strategy_latency_ms")?.unwrap_or(0),
            size_cap: false,
        };
        match f.map_entry("calendar") {
            None => {}
            Some(Value::Null) => s.calendar = None,
            Some(Value::Array(days)) => {
                s.weekdays = false;
                for d in days {
                    let mut df = Fields::new(d, "calendar")?;
                    let day = SessionDay {
                        date: df.string("date")?,
                        open: df.string("open")?,
                        close: df.string("close")?,
                    };
                    if parse_date(&day.date).is_none()
                        || hhmm(&day.open).is_none()
                        || hhmm(&day.close).is_none()
                    {
                        return Err(invalid("calendar", "date YYYY-MM-DD, open/close HH:MM"));
                    }
                    df.finish()?;
                    s.calendar.get_or_insert_with(Vec::new).push(day);
                }
            }
            Some(_) => return Err(invalid("calendar", "null or a list of days")),
        }
        if let Some(a) = f.take("account") {
            let mut a = Fields::new(a, "account")?;
            s.status = a.opt_string("status")?.unwrap_or(s.status);
            s.level = u32::try_from(a.opt_int("options_trading_level")?.unwrap_or(3)).unwrap_or(0);
            if let Some(c) = a.take("cash") {
                s.cash_cents = price(&c, "account.cash")?;
            }
            s.trading_blocked = a.opt_bool("blocked")?.unwrap_or(false);
            a.finish()?;
        }
        if let Some(Value::Object(p)) = f.take("positions") {
            for (k, v) in p {
                s.positions.insert(
                    k,
                    v.as_i64().ok_or_else(|| invalid("positions", "integers"))?,
                );
            }
        }
        if let Some(Value::Array(orders)) = f.take("orders") {
            for o in orders {
                let mut o = Fields::new(o, "orders")?;
                s.orders.push(Preload {
                    client_order_id: o.string("client_order_id")?,
                    symbol: o.string("symbol")?,
                    buy: o.string("side")? == "buy",
                    qty: o.int("qty", 1, 1_000_000)?,
                    limit_cents: price(&o.req("limit_price")?, "orders.limit_price")?,
                    status: o.opt_string("status")?.unwrap_or_else(|| "new".into()),
                });
                o.finish()?;
            }
        }
        if let Some(Value::Object(q)) = f.take("quotes") {
            for (k, v) in q {
                let side = |n: &str| v.get(n).map(|p| price(p, "quotes")).transpose();
                s.quotes.insert(k.clone(), (side("bid")?, side("ask")?));
            }
        }
        if let Some(d) = f.take("default") {
            s.default = rule(d, "default")?.1;
        }
        if let Some(Value::Array(rules)) = f.take("rules") {
            for (i, r) in rules.into_iter().enumerate() {
                let path = format!("rules[{i}]");
                let (m, r) = rule(r, &path)?;
                s.rules.push((
                    m.ok_or_else(|| invalid(&path, "nth or client_order_id"))?,
                    r,
                ));
            }
        }
        if let Some(st) = f.take("stream") {
            let mut st = Fields::new(st, "stream")?;
            s.drop_after = Some((
                st.int("drop_after_events", 0, u64::MAX)?,
                st.opt_int("down_ms")?.unwrap_or(200),
            ));
            st.finish()?;
        }
        if let Some(fee) = f.take("fee_per_contract") {
            s.fee_cents = price(&fee, "fee_per_contract")?;
        }
        s.size_cap = match f.opt_string("size_cap")?.as_deref() {
            None | Some("none") => false,
            Some("displayed") => true,
            Some(_) => return Err(invalid("size_cap", "none or displayed")),
        };
        f.finish()?;
        Ok(s)
    }

    /// The rule for the `nth` submit of `cid`: an id rule wins over an nth rule.
    pub(crate) fn rule_for(&self, nth: u64, cid: &str) -> Rule {
        let by_id = self
            .rules
            .iter()
            .find(|(m, _)| matches!(m, Match::Id(i) if i == cid));
        let by_nth = || {
            self.rules
                .iter()
                .find(|(m, _)| matches!(m, Match::Nth(n) if *n == nth))
        };
        by_id
            .or_else(by_nth)
            .map_or_else(|| self.default.clone(), |(_, r)| r.clone())
    }
}

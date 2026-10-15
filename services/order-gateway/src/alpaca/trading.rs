//! Alpaca trading (paper, or live with the operator's explicit `--allow-live`) for `gateway orders`: endpoint constants, auth construction from
//! injected secrets, order request bodies, and parsing of order, account, position,
//! clock, calendar, error and `trade_updates` bodies. No I/O: the gateway sends and
//! receives. Raw texts keep key order and number lexemes; only whitespace outside
//! strings is removed.
use serde_json::Value;
use std::fmt;
use std::net::SocketAddr;

const PAPER_REST: &str = "https://paper-api.alpaca.markets";
const PAPER_STREAM: &str = "wss://paper-api.alpaca.markets/stream";
/// The live host, the only place it appears (operator decision, 2026-10-12: the console's
/// Spread builder trades the live account through its own gateway, under small caps).
const LIVE_HOST: &str = "api.alpaca.markets";

/// The `trade_updates` subscription message.
pub const LISTEN: &str = r#"{"action":"listen","data":{"streams":["trade_updates"]}}"#;

/// A REST and stream endpoint pair: paper, a loopback test server, or live, which the
/// gateway builds only for `--broker alpaca-live --allow-live`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Endpoints {
    rest: String,
    stream: String,
    environment: &'static str,
}

impl Endpoints {
    /// `paper-api.alpaca.markets`.
    #[must_use]
    pub fn paper() -> Self {
        Self {
            rest: PAPER_REST.into(),
            stream: PAPER_STREAM.into(),
            environment: "paper",
        }
    }

    /// The live account's endpoints. Real money: only for `--allow-live`.
    #[must_use]
    pub fn live() -> Self {
        Self {
            rest: format!("https://{LIVE_HOST}"),
            stream: format!("wss://{LIVE_HOST}/stream"),
            environment: "live",
        }
    }

    /// A plain-text fake server for offline tests; `None` unless `addr` is loopback.
    #[must_use]
    pub fn loopback(addr: SocketAddr) -> Option<Self> {
        addr.ip().is_loopback().then(|| Self {
            rest: format!("http://{addr}"),
            stream: format!("ws://{addr}/stream"),
            environment: "loopback",
        })
    }

    /// REST base URL without a trailing slash.
    #[must_use]
    pub fn rest(&self) -> &str {
        &self.rest
    }

    /// `trade_updates` WebSocket URL.
    #[must_use]
    pub fn stream(&self) -> &str {
        &self.stream
    }

    /// `paper`, `live` or `loopback`.
    #[must_use]
    pub fn environment(&self) -> &'static str {
        self.environment
    }
}

/// API key pair. `Debug` is redacted and no error names a value.
#[derive(Clone)]
pub struct Credentials {
    key: String,
    secret: String,
}

impl fmt::Debug for Credentials {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Credentials(<redacted>)")
    }
}

const KEY_NAMES: [&str; 3] = ["APCA_API_KEY_ID", "ALPACA_API_KEY_ID", "ALPACA_API_KEY"];
const SECRET_NAMES: [&str; 3] = [
    "APCA_API_SECRET_KEY",
    "ALPACA_API_SECRET_KEY",
    "ALPACA_SECRET_KEY",
];

impl Credentials {
    /// Parses `NAME=value` lines (`export`, quotes and `#` comments allowed) with the
    /// `APCA_*` or `ALPACA_*` names.
    ///
    /// # Errors
    /// A missing name or a value with whitespace or control bytes; the message names
    /// the variable, never its value.
    pub fn from_env_text(text: &str) -> Result<Self, String> {
        let mut found: Vec<(&str, &str)> = Vec::new();
        for line in text.lines().map(str::trim) {
            let line = line.strip_prefix("export ").unwrap_or(line);
            if let Some((k, v)) = line.split_once('=')
                && !line.starts_with('#')
            {
                found.push((k.trim(), v.trim().trim_matches(['"', '\''])));
            }
        }
        let pick = |names: &[&str]| -> Result<String, String> {
            let v = names
                .iter()
                .find_map(|n| found.iter().rev().find(|(k, _)| k == n).map(|(_, v)| *v))
                .filter(|v| !v.is_empty())
                .ok_or_else(|| format!("credentials need one of {}", names.join(", ")))?;
            if v.bytes().all(|b| b.is_ascii_graphic()) {
                Ok(v.to_owned())
            } else {
                Err(format!("{} has whitespace or control bytes", names[0]))
            }
        };
        Ok(Self {
            key: pick(&KEY_NAMES)?,
            secret: pick(&SECRET_NAMES)?,
        })
    }

    /// REST authentication headers.
    #[must_use]
    pub fn headers(&self) -> [(&'static str, &str); 2] {
        [
            ("APCA-API-KEY-ID", &self.key),
            ("APCA-API-SECRET-KEY", &self.secret),
        ]
    }

    /// The stream `auth` message.
    #[must_use]
    pub fn stream_auth(&self) -> String {
        format!(
            r#"{{"action":"auth","key":{},"secret":{}}}"#,
            quote(&self.key),
            quote(&self.secret)
        )
    }
}

fn quote(s: &str) -> String {
    serde_json::to_string(s).unwrap_or_default()
}

/// One leg of an order request.
#[derive(Clone, Copy, Debug)]
pub struct LegSpec<'a> {
    /// Compact OCC symbol.
    pub symbol: &'a str,
    /// Ratio per parent unit.
    pub ratio: u32,
    /// `buy` (else `sell`).
    pub buy: bool,
    /// Opens a position (else closes).
    pub open: bool,
    /// A stock (sent without `position_intent`; Alpaca infers it from the position).
    pub equity: bool,
}

/// The `POST /v2/orders` body for a limit/day order, with Reactor's field order: a single
/// `{symbol, qty, side, type, limit_price, time_in_force, client_order_id, position_intent}`
/// or an MLEG `{order_class, qty, type, time_in_force, limit_price, client_order_id,
/// legs:[{symbol, ratio_qty, side, position_intent}]}`. Quantities are strings. A stock
/// single has no `position_intent`, and `time_in_force` may be `cls` (limit-on-close).
#[must_use]
pub fn order_body(
    client_order_id: &str,
    combo: bool,
    legs: &[LegSpec<'_>],
    qty: u32,
    limit_price: Option<&str>,
    on_close: bool,
) -> String {
    let side = |l: &LegSpec<'_>| if l.buy { "buy" } else { "sell" };
    let intent = |l: &LegSpec<'_>| {
        let effect = if l.open { "open" } else { "close" };
        format!("\"{}_to_{effect}\"", side(l))
    };
    let cid = quote(client_order_id);
    // A limit order's type and price; a market order has no price.
    let ty = if limit_price.is_some() {
        "limit"
    } else {
        "market"
    };
    let price = limit_price.map_or_else(String::new, |p| format!(r#","limit_price":{}"#, quote(p)));
    if combo {
        let legs: Vec<String> = legs
            .iter()
            .map(|l| {
                format!(
                    r#"{{"symbol":{},"ratio_qty":"{}","side":"{}","position_intent":{}}}"#,
                    quote(l.symbol),
                    l.ratio,
                    side(l),
                    intent(l)
                )
            })
            .collect();
        format!(
            r#"{{"order_class":"mleg","qty":"{qty}","type":"{ty}","time_in_force":"day"{price},"client_order_id":{cid},"legs":[{}]}}"#,
            legs.join(",")
        )
    } else if legs[0].equity {
        let l = &legs[0];
        let tif = if on_close { "cls" } else { "day" };
        format!(
            r#"{{"symbol":{},"qty":"{qty}","side":"{}","type":"{ty}"{price},"time_in_force":"{tif}","client_order_id":{cid}}}"#,
            quote(l.symbol),
            side(l)
        )
    } else {
        let l = &legs[0];
        format!(
            r#"{{"symbol":{},"qty":"{qty}","side":"{}","type":"{ty}"{price},"time_in_force":"day","client_order_id":{cid},"position_intent":{}}}"#,
            quote(l.symbol),
            side(l),
            intent(l)
        )
    }
}

/// JSON text with whitespace outside strings removed (key order and lexemes kept).
#[must_use]
pub fn compact(text: &str) -> String {
    let (mut out, mut in_str, mut esc) = (String::with_capacity(text.len()), false, false);
    for c in text.chars() {
        if in_str {
            if esc {
                esc = false;
            } else {
                esc = c == '\\';
                in_str = c != '"';
            }
        } else if c.is_ascii_whitespace() {
            continue;
        } else {
            in_str = c == '"';
        }
        out.push(c);
    }
    out
}

/// End of the compact JSON value starting at `i`.
fn skip(b: &[u8], mut i: usize) -> Option<usize> {
    let mut depth = 0_usize;
    loop {
        match *b.get(i)? {
            b'"' => {
                i += 1;
                while *b.get(i)? != b'"' {
                    i += if b[i] == b'\\' { 2 } else { 1 };
                }
                i += 1;
            }
            b'{' | b'[' => {
                depth += 1;
                i += 1;
            }
            b'}' | b']' | b',' if depth == 0 => return Some(i),
            b'}' | b']' => {
                depth -= 1;
                i += 1;
            }
            _ => i += 1,
        }
        if depth == 0 && matches!(b.get(i), None | Some(b',' | b'}' | b']' | b':')) {
            return Some(i);
        }
    }
}

/// Raw elements of a compact JSON array.
#[must_use]
pub fn elements(array: &str) -> Option<Vec<&str>> {
    let b = array.as_bytes();
    let mut out = Vec::new();
    if b.first() != Some(&b'[') {
        return None;
    }
    let mut i = 1;
    while b.get(i) != Some(&b']') {
        let end = skip(b, i).filter(|e| *e > i)?;
        out.push(&array[i..end]);
        match b.get(end) {
            Some(b',') => i = end + 1,
            Some(b']') => i = end,
            _ => return None,
        }
    }
    Some(out)
}

/// Raw value of a top-level member of a compact JSON object.
#[must_use]
pub fn member<'a>(object: &'a str, key: &str) -> Option<&'a str> {
    let b = object.as_bytes();
    let mut i = 1;
    while b.get(i) == Some(&b'"') {
        let k_end = skip(b, i)?;
        let v_end = skip(b, k_end + 1)?;
        if &object[i + 1..k_end - 1] == key {
            return Some(&object[k_end + 1..v_end]);
        }
        i = v_end + 1;
    }
    None
}

/// RFC 3339 with any `±HH:MM` offset, to Unix nanoseconds.
#[must_use]
pub fn parse_time(s: &str) -> Option<u64> {
    let at = s.len().checked_sub(6)?;
    let (sign, rest) = match s.as_bytes().get(at) {
        Some(b'+') => (1_i64, &s[at + 1..]),
        Some(b'-') if at > 10 => (-1, &s[at + 1..]),
        _ => return parse_rfc3339_ns(s).ok(),
    };
    let (h, m) = rest.split_once(':')?;
    let off = sign * (h.parse::<i64>().ok()? * 3600 + m.parse::<i64>().ok()? * 60);
    let utc = i64::try_from(parse_rfc3339_ns(&format!("{}Z", &s[..at])).ok()?).ok()?;
    u64::try_from(utc - off * 1_000_000_000).ok()
}

fn text(v: &Value, k: &str) -> Option<String> {
    v.get(k).and_then(Value::as_str).map(String::from)
}

/// Integer part of a quantity given as a string or a number.
fn int(v: &Value) -> Option<i64> {
    let s = match v {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        _ => return None,
    };
    s.split('.').next()?.parse().ok()
}

fn uint(v: &Value, k: &str) -> u64 {
    v.get(k)
        .and_then(int)
        .and_then(|n| u64::try_from(n).ok())
        .unwrap_or(0)
}

/// One nested leg of an MLEG order.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OrderLeg {
    /// OCC symbol.
    pub symbol: String,
    /// Cumulative filled contracts.
    pub filled_qty: u64,
    /// Cumulative average price.
    pub filled_avg_price: Option<String>,
}

/// An order (`GET`/`POST /v2/orders` or `trade_updates.order`).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Order {
    /// Broker id.
    pub id: String,
    /// Client id.
    pub client_order_id: String,
    /// Status string as sent.
    pub status: String,
    /// Cumulative filled quantity (parent units).
    pub filled_qty: u64,
    /// Cumulative average price.
    pub filled_avg_price: Option<String>,
    /// `order_class: mleg`.
    pub mleg: bool,
    /// Nested legs (MLEG only).
    pub legs: Vec<OrderLeg>,
    /// `updated_at` (else `created_at`) in ns.
    pub updated_ns: u64,
    /// Compact raw order text.
    pub raw: String,
}

/// Parses one order object.
#[must_use]
pub fn parse_order(raw: &str) -> Option<Order> {
    let raw = compact(raw);
    let v: Value = serde_json::from_str(&raw).ok()?;
    let legs = v
        .get("legs")
        .and_then(Value::as_array)
        .map_or(Vec::new(), |a| {
            a.iter()
                .map(|l| OrderLeg {
                    symbol: text(l, "symbol").unwrap_or_default(),
                    filled_qty: uint(l, "filled_qty"),
                    filled_avg_price: text(l, "filled_avg_price"),
                })
                .collect()
        });
    let time = |k| text(&v, k).as_deref().and_then(parse_time);
    Some(Order {
        id: text(&v, "id")?,
        client_order_id: text(&v, "client_order_id")?,
        status: text(&v, "status")?,
        filled_qty: uint(&v, "filled_qty"),
        filled_avg_price: text(&v, "filled_avg_price"),
        mleg: v["order_class"] == "mleg",
        legs,
        updated_ns: time("updated_at")
            .or_else(|| time("created_at"))
            .unwrap_or(0),
        raw,
    })
}

/// Parses an order array.
#[must_use]
pub fn parse_orders(raw: &str) -> Option<Vec<Order>> {
    elements(&compact(raw))?
        .into_iter()
        .map(parse_order)
        .collect()
}

/// `{code, message}` of an error body; a non-JSON body becomes `http_<status>` and its
/// first 200 characters.
#[must_use]
pub fn parse_error(status: u16, body: &str) -> (String, String) {
    let v: Value = serde_json::from_str(body).unwrap_or(Value::Null);
    let code = match &v["code"] {
        Value::Number(n) => n.to_string(),
        Value::String(s) => s.clone(),
        _ => format!("http_{status}"),
    };
    let message = text(&v, "message").unwrap_or_else(|| body.chars().take(200).collect());
    (code, message)
}

/// Account fields used by the gateway.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Account {
    /// e.g. `ACTIVE`.
    pub status: String,
    /// Decimal string.
    pub cash: String,
    /// Decimal string.
    pub buying_power: String,
    /// `options_trading_level` (else `options_approved_level`).
    pub options_trading_level: u32,
    /// `trading_blocked || account_blocked`.
    pub blocked: bool,
}

/// Parses `GET /v2/account`.
#[must_use]
pub fn parse_account(raw: &str) -> Option<Account> {
    let v: Value = serde_json::from_str(raw).ok()?;
    let level = ["options_trading_level", "options_approved_level"]
        .iter()
        .find_map(|k| v.get(*k).and_then(int))
        .and_then(|n| u32::try_from(n).ok())
        .unwrap_or(0);
    let flag = |k: &str| v[k].as_bool().unwrap_or(false);
    Some(Account {
        status: text(&v, "status")?,
        cash: text(&v, "cash").unwrap_or_default(),
        buying_power: text(&v, "buying_power").unwrap_or_default(),
        options_trading_level: level,
        blocked: flag("trading_blocked") || flag("account_blocked"),
    })
}

/// Parses `GET /v2/positions` into `(symbol, signed qty, avg_entry_price)`.
#[must_use]
pub fn parse_positions(raw: &str) -> Option<Vec<(String, i64, String)>> {
    let v: Value = serde_json::from_str(raw).ok()?;
    v.as_array()?
        .iter()
        .map(|p| {
            let qty = p.get("qty").and_then(int)?;
            let short = p["side"] == "short" && qty > 0;
            let avg = text(p, "avg_entry_price").unwrap_or_default();
            Some((text(p, "symbol")?, if short { -qty } else { qty }, avg))
        })
        .collect()
}

/// `GET /v2/clock` as `(ts, is_open, next_open, next_close)` in ns.
#[must_use]
pub fn parse_clock(raw: &str) -> Option<(u64, bool, u64, u64)> {
    let v: Value = serde_json::from_str(raw).ok()?;
    let t = |k| text(&v, k).as_deref().and_then(parse_time);
    Some((
        t("timestamp")?,
        v["is_open"].as_bool()?,
        t("next_open")?,
        t("next_close")?,
    ))
}

/// `GET /v2/calendar` as `(date, open, close)` (`America/New_York`).
#[must_use]
pub fn parse_calendar(raw: &str) -> Option<Vec<(String, String, String)>> {
    let v: Value = serde_json::from_str(raw).ok()?;
    v.as_array()?
        .iter()
        .map(|d| Some((text(d, "date")?, text(d, "open")?, text(d, "close")?)))
        .collect()
}

/// One leg execution of an MLEG fill.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LegFill {
    /// OCC symbol.
    pub symbol: String,
    /// Execution id when sent.
    pub execution_id: Option<String>,
    /// Increment.
    pub qty: u64,
    /// Execution price.
    pub price: Option<String>,
}

/// One `trade_updates` event.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TradeUpdate {
    /// Exact broker event string.
    pub event: String,
    /// The order after the event.
    pub order: Order,
    /// `timestamp` (else `at`) in ns.
    pub timestamp_ns: Option<u64>,
    /// Execution id (fills).
    pub execution_id: Option<String>,
    /// Fill increment.
    pub qty: Option<u64>,
    /// Fill price (net per unit for MLEG).
    pub price: Option<String>,
    /// Signed position after a single-leg fill.
    pub position_qty: Option<i64>,
    /// Per-leg executions of an MLEG fill.
    pub legs: Vec<LegFill>,
    /// Compact raw `data` object.
    pub raw: String,
}

/// A parsed stream message.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum StreamMessage {
    /// `authorization` with `status: authorized`.
    Authorized,
    /// `authorization` with any other status.
    Unauthorized,
    /// `listening` acknowledgement.
    Listening,
    /// An order event.
    Update(Box<TradeUpdate>),
    /// `action: error` with its message.
    Error(String),
    /// Anything else (kept by the caller as raw text if needed).
    Other,
}

/// Parses one stream message (text or binary frame payload).
#[must_use]
pub fn parse_stream(frame: &str) -> StreamMessage {
    let frame = compact(frame);
    let Ok(v) = serde_json::from_str::<Value>(&frame) else {
        return StreamMessage::Other;
    };
    let d = &v["data"];
    match (v["stream"].as_str(), v["action"].as_str()) {
        (Some("authorization"), _) if d["status"] == "authorized" => StreamMessage::Authorized,
        (Some("authorization"), _) => StreamMessage::Unauthorized,
        (Some("listening"), _) => StreamMessage::Listening,
        (_, Some("error")) => StreamMessage::Error(text(d, "error_message").unwrap_or_default()),
        (Some("trade_updates"), _) => {
            let raw = member(&frame, "data").unwrap_or("null").to_owned();
            let Some(order) = member(&raw, "order").and_then(parse_order) else {
                return StreamMessage::Other;
            };
            let legs = d["legs"].as_array().map_or(Vec::new(), |a| {
                a.iter()
                    .map(|l| LegFill {
                        symbol: text(l, "symbol").unwrap_or_default(),
                        execution_id: text(l, "execution_id"),
                        qty: uint(l, "qty"),
                        price: text(l, "price"),
                    })
                    .collect()
            });
            let time = |k| text(d, k).as_deref().and_then(parse_time);
            StreamMessage::Update(Box::new(TradeUpdate {
                event: text(d, "event").unwrap_or_default(),
                timestamp_ns: time("timestamp").or_else(|| time("at")),
                execution_id: text(d, "execution_id"),
                qty: d
                    .get("qty")
                    .and_then(int)
                    .and_then(|n| u64::try_from(n).ok()),
                price: text(d, "price"),
                position_qty: d.get("position_qty").and_then(int),
                legs,
                order,
                raw,
            }))
        }
        _ => StreamMessage::Other,
    }
}

/// Parses an Alpaca RFC-3339 timestamp to exact Unix nanoseconds.
///
/// Fractional seconds run one through nine digits (trailing zeros are dropped by the
/// source) and are right-padded to nine; the `Z` or `+00:00` offset is required.
///
/// # Errors
/// Rejects any other shape, out-of-range field, or more than nine fractional digits.
fn parse_rfc3339_ns(value: &str) -> Result<u64, ()> {
    let malformed = ();
    let bytes = value.as_bytes();
    if bytes.len() < 20 || bytes[4] != b'-' || bytes[7] != b'-' || bytes[10] != b'T' {
        return Err(malformed);
    }
    let year = value[0..4].parse::<i64>().map_err(|_| malformed)?;
    let month = value[5..7].parse::<u8>().map_err(|_| malformed)?;
    let day = value[8..10].parse::<u8>().map_err(|_| malformed)?;
    let hour = value[11..13].parse::<u8>().map_err(|_| malformed)?;
    let minute = value[14..16].parse::<u8>().map_err(|_| malformed)?;
    let second = value[17..19].parse::<u8>().map_err(|_| malformed)?;
    if bytes[13] != b':'
        || bytes[16] != b':'
        || !(1..=12).contains(&month)
        || day == 0
        || day > 31
        || hour > 23
        || minute > 59
        || second > 60
    {
        return Err(malformed);
    }
    let rest = &value[19..];
    let (fraction, offset) = match rest.find(['Z', '+', '-']) {
        Some(index) => (&rest[..index], &rest[index..]),
        None => return Err(malformed),
    };
    match offset {
        "Z" | "+00:00" => {}
        _ => return Err(malformed),
    }
    let fraction = fraction.strip_prefix('.').unwrap_or(fraction);
    if fraction.len() > 9 || !fraction.bytes().all(|b| b.is_ascii_digit()) {
        return Err(malformed);
    }
    let mut nanos = [b'0'; 9];
    nanos[..fraction.len()].copy_from_slice(fraction.as_bytes());
    let nanos = std::str::from_utf8(&nanos)
        .map_err(|_| malformed)?
        .parse::<u64>()
        .map_err(|_| malformed)?;
    let days = days_from_civil(year, month, day);
    let seconds = days
        .checked_mul(86_400)
        .and_then(|d| d.checked_add(u64::from(hour) * 3_600 + u64::from(minute) * 60))
        .and_then(|s| s.checked_add(u64::from(second)))
        .ok_or(malformed)?;
    seconds
        .checked_mul(1_000_000_000)
        .and_then(|s| s.checked_add(nanos))
        .ok_or(malformed)
}

/// Days since 1970-01-01 for a proleptic-Gregorian civil date (Hinnant's algorithm).
fn days_from_civil(year: i64, month: u8, day: u8) -> u64 {
    let y = year - i64::from(month <= 2);
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (i64::from(month) + 9) % 12;
    let doy = (153 * mp + 2) / 5 + i64::from(day) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    u64::try_from(days).unwrap_or(0)
}

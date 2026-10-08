//! `tinytrader.gateway/1` wire: strict client-line parsing and ordered server JSON.
//!
//! Server messages are written with [`Obj`], so field order is fixed and `broker_raw`
//! text is embedded verbatim. Client lines are parsed strictly through [`Fields`].
use serde_json::{Map, Value};
use std::fmt::Display;

/// The protocol identifier carried by every message.
pub const SCHEMA: &str = "tinytrader.gateway/1";
/// Maximum line length in bytes, including the newline.
pub const MAX_LINE: usize = 65_536;

/// An ordered JSON object writer.
pub(crate) struct Obj(String);

impl Obj {
    pub(crate) fn new() -> Self {
        Self(String::from("{"))
    }
    /// A server message `{schema, type, ...}`.
    pub(crate) fn msg(ty: &str) -> Self {
        Self::new().s("schema", SCHEMA).s("type", ty)
    }
    fn key(&mut self, k: &str) {
        if self.0.len() > 1 {
            self.0.push(',');
        }
        self.0.push('"');
        self.0.push_str(k);
        self.0.push_str("\":");
    }
    pub(crate) fn s(mut self, k: &str, v: &str) -> Self {
        self.key(k);
        self.0.push_str(&quote(v));
        self
    }
    pub(crate) fn n(mut self, k: &str, v: impl Display) -> Self {
        self.key(k);
        self.0.push_str(&v.to_string());
        self
    }
    pub(crate) fn raw(mut self, k: &str, v: &str) -> Self {
        self.key(k);
        self.0.push_str(v);
        self
    }
    pub(crate) fn b(self, k: &str, v: bool) -> Self {
        self.raw(k, if v { "true" } else { "false" })
    }
    pub(crate) fn opt_s(self, k: &str, v: Option<&str>) -> Self {
        match v {
            Some(v) => self.s(k, v),
            None => self.raw(k, "null"),
        }
    }
    /// Appends the fields of another finished object (`{...}`).
    pub(crate) fn merge(mut self, other: &str) -> Self {
        let inner = &other[1..other.len() - 1];
        if !inner.is_empty() {
            if self.0.len() > 1 {
                self.0.push(',');
            }
            self.0.push_str(inner);
        }
        self
    }
    pub(crate) fn done(mut self) -> String {
        self.0.push('}');
        self.0
    }
}

pub(crate) fn quote(v: &str) -> String {
    serde_json::to_string(v).unwrap_or_else(|_| String::from("\"\""))
}

pub(crate) fn arr(items: impl IntoIterator<Item = String>) -> String {
    let mut out = String::from("[");
    for (i, item) in items.into_iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        out.push_str(&item);
    }
    out.push(']');
    out
}

/// A `type:"error"` connection message.
pub(crate) fn error_line(code: &str, message: &str) -> String {
    Obj::msg("error")
        .s("code", code)
        .s("message", message)
        .done()
}

/// A request refusal: the path and reason of the first violation.
#[derive(Debug)]
pub(crate) struct Invalid {
    pub(crate) path: String,
    pub(crate) message: String,
}

/// Strict field access over one JSON object; every key must be consumed.
pub(crate) struct Fields {
    map: Map<String, Value>,
    path: String,
}

impl Fields {
    pub(crate) fn new(v: Value, path: &str) -> Result<Self, Invalid> {
        match v {
            Value::Object(map) => Ok(Self {
                map,
                path: path.to_owned(),
            }),
            _ => Err(invalid(path, "expected an object")),
        }
    }
    pub(crate) fn at(&self, k: &str) -> String {
        if self.path.is_empty() {
            k.to_owned()
        } else {
            format!("{}.{k}", self.path)
        }
    }
    pub(crate) fn take(&mut self, k: &str) -> Option<Value> {
        self.map.remove(k).filter(|v| !v.is_null())
    }
    /// Like `take`, but keeps an explicit `null`.
    pub(crate) fn map_entry(&mut self, k: &str) -> Option<Value> {
        self.map.remove(k)
    }
    pub(crate) fn req(&mut self, k: &str) -> Result<Value, Invalid> {
        self.take(k)
            .ok_or_else(|| invalid(&self.at(k), "missing field"))
    }
    pub(crate) fn string(&mut self, k: &str) -> Result<String, Invalid> {
        match self.req(k)? {
            Value::String(s) => Ok(s),
            _ => Err(invalid(&self.at(k), "expected a string")),
        }
    }
    pub(crate) fn opt_string(&mut self, k: &str) -> Result<Option<String>, Invalid> {
        if self.map.get(k).is_none_or(Value::is_null) {
            self.map.remove(k);
            return Ok(None);
        }
        self.string(k).map(Some)
    }
    pub(crate) fn int(&mut self, k: &str, lo: u64, hi: u64) -> Result<u64, Invalid> {
        let path = self.at(k);
        self.req(k)?
            .as_u64()
            .filter(|n| (lo..=hi).contains(n))
            .ok_or_else(|| invalid(&path, &format!("expected an integer in {lo}..={hi}")))
    }
    pub(crate) fn opt_int(&mut self, k: &str) -> Result<Option<u64>, Invalid> {
        if self.map.get(k).is_none_or(Value::is_null) {
            self.map.remove(k);
            return Ok(None);
        }
        self.int(k, 0, u64::MAX).map(Some)
    }
    pub(crate) fn opt_bool(&mut self, k: &str) -> Result<Option<bool>, Invalid> {
        match self.take(k) {
            None => Ok(None),
            Some(Value::Bool(b)) => Ok(Some(b)),
            Some(_) => Err(invalid(&self.at(k), "expected a boolean")),
        }
    }
    pub(crate) fn array(&mut self, k: &str) -> Result<Vec<Value>, Invalid> {
        match self.req(k)? {
            Value::Array(a) => Ok(a),
            _ => Err(invalid(&self.at(k), "expected an array")),
        }
    }
    pub(crate) fn finish(self) -> Result<(), Invalid> {
        match self.map.keys().next() {
            Some(k) => Err(invalid(&self.at(k), "unknown field")),
            None => Ok(()),
        }
    }
}

pub(crate) fn invalid(path: &str, message: &str) -> Invalid {
    Invalid {
        path: path.to_owned(),
        message: message.to_owned(),
    }
}

/// An order reference accepted by `cancel` and `order`.
#[derive(Clone, Debug)]
pub enum OrderRef {
    /// The client-assigned logical key.
    Client(String),
    /// The broker's identifier, kept as received.
    Broker(String),
}

/// Kill switch level.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum KillLevel {
    /// Refuse submits with an opening leg.
    HaltOpens,
    /// Refuse every submit and cancel every open order.
    HaltAll,
}

impl KillLevel {
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::HaltOpens => "halt_opens",
            Self::HaltAll => "halt_all",
        }
    }
    pub(crate) fn parse(s: &str) -> Option<Self> {
        match s {
            "halt_opens" => Some(Self::HaltOpens),
            "halt_all" => Some(Self::HaltAll),
            _ => None,
        }
    }
}

pub(crate) enum Op {
    Submit(Value),
    Cancel(OrderRef),
    Order(OrderRef),
    OpenOrders,
    Account,
    Positions,
    Activities {
        types: Vec<String>,
        after: Option<String>,
        page_token: Option<String>,
    },
    Clock,
    Calendar {
        start: String,
        end: String,
    },
    Kill {
        level: KillLevel,
        reason: String,
    },
    Unkill,
    Advance(u64),
}

pub(crate) struct Hello {
    pub(crate) client: String,
    pub(crate) token: String,
    pub(crate) resume_after: Option<u64>,
    pub(crate) cancel_on_disconnect: bool,
}

pub(crate) enum Line {
    Hello(Hello),
    Request { request_id: String, op: Op },
    Heartbeat,
}

pub(crate) enum LineError {
    /// A connection error: send it and close.
    Fatal(&'static str, String),
    /// A request error; the connection stays open.
    Invalid {
        request_id: Option<String>,
        op: String,
        invalid: Invalid,
    },
}

fn order_ref(f: &mut Fields) -> Result<OrderRef, Invalid> {
    match (
        f.opt_string("client_order_id")?,
        f.opt_string("broker_order_id")?,
    ) {
        (Some(c), None) => Ok(OrderRef::Client(c)),
        (None, Some(b)) => Ok(OrderRef::Broker(b)),
        _ => Err(invalid(
            "client_order_id",
            "exactly one of client_order_id or broker_order_id",
        )),
    }
}

pub(crate) fn valid_date(s: &str) -> bool {
    crate::safety::parse_date(s).is_some()
}

/// Parses one complete line (without its newline).
#[expect(clippy::too_many_lines, reason = "one flat table of request grammars")]
pub(crate) fn parse_line(bytes: &[u8]) -> Result<Line, LineError> {
    let value: Value =
        serde_json::from_slice(bytes).map_err(|e| LineError::Fatal("bad_json", e.to_string()))?;
    let Value::Object(mut map) = value else {
        return Err(LineError::Fatal("bad_json", "not an object".into()));
    };
    if map.remove("schema").as_ref().and_then(Value::as_str) != Some(SCHEMA) {
        return Err(LineError::Fatal(
            "schema_unsupported",
            format!("expected {SCHEMA}"),
        ));
    }
    let Some(Value::String(ty)) = map.remove("type") else {
        return Err(LineError::Fatal("bad_json", "missing type".into()));
    };
    let mut f = Fields {
        map,
        path: String::new(),
    };
    if ty == "hello" {
        let parsed = (|| {
            let hello = Hello {
                client: f.string("client")?,
                token: f.string("token")?,
                resume_after: f.opt_int("resume_after_seq")?,
                cancel_on_disconnect: f.opt_bool("cancel_on_disconnect")?.unwrap_or(false),
            };
            f.finish().map(|()| hello)
        })();
        return parsed
            .map(Line::Hello)
            .map_err(|e| LineError::Fatal("bad_json", format!("{}: {}", e.path, e.message)));
    }
    if ty == "heartbeat" {
        f.map.remove("ts");
        f.map.remove("last_seq");
        return Ok(Line::Heartbeat);
    }
    let request_id = f
        .map
        .get("request_id")
        .and_then(Value::as_str)
        .map(String::from);
    let fail = |invalid| LineError::Invalid {
        request_id: request_id.clone(),
        op: ty.clone(),
        invalid,
    };
    let rid = f.string("request_id").map_err(fail)?;
    if rid.is_empty() || rid.len() > 64 {
        return Err(fail(invalid("request_id", "1 to 64 characters")));
    }
    let op = (|| {
        let op = match ty.as_str() {
            "submit" => Op::Submit(f.req("intent")?),
            "cancel" => Op::Cancel(order_ref(&mut f)?),
            "order" => Op::Order(order_ref(&mut f)?),
            "open_orders" => Op::OpenOrders,
            "account" => Op::Account,
            "positions" => Op::Positions,
            "activities" => {
                let path = f.at("types");
                let types = f
                    .array("types")?
                    .into_iter()
                    .map(|v| v.as_str().map(String::from))
                    .collect::<Option<Vec<_>>>()
                    .ok_or_else(|| invalid(&path, "expected strings"))?;
                Op::Activities {
                    types,
                    after: f.opt_string("after")?,
                    page_token: f.opt_string("page_token")?,
                }
            }
            "clock" => Op::Clock,
            "calendar" => {
                let (start, end) = (f.string("start")?, f.string("end")?);
                if !valid_date(&start) || !valid_date(&end) {
                    return Err(invalid("start", "dates are YYYY-MM-DD"));
                }
                Op::Calendar { start, end }
            }
            "kill" => {
                let level = f.string("level")?;
                Op::Kill {
                    level: KillLevel::parse(&level)
                        .ok_or_else(|| invalid("level", "halt_opens or halt_all"))?,
                    reason: f.string("reason")?,
                }
            }
            "unkill" => {
                f.string("reason")?;
                Op::Unkill
            }
            "advance" => Op::Advance(f.int("to_ts", 0, u64::MAX)?),
            _ => return Err(invalid("type", "unknown request type")),
        };
        f.finish().map(|()| op)
    })()
    .map_err(fail)?;
    Ok(Line::Request {
        request_id: rid,
        op,
    })
}

/// The response envelope with `outcome` fields merged from a finished object.
pub(crate) fn response(request_id: &str, op: &str, outcome: &str) -> String {
    Obj::msg("response")
        .s("request_id", request_id)
        .s("op", op)
        .merge(outcome)
        .done()
}

pub(crate) fn request_error(request_id: &str, op: &str, code: &str, detail: &str) -> String {
    let o = Obj::new()
        .s("outcome", "error")
        .s("code", code)
        .raw("detail", detail)
        .done();
    response(request_id, op, &o)
}

pub(crate) fn invalid_detail(e: &Invalid) -> String {
    Obj::new()
        .s("path", &e.path)
        .s("message", &e.message)
        .done()
}

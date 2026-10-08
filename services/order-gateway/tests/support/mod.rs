//! Shared harness: disposable state roots, in-process gateways and a line client.
#![allow(dead_code)]
use order_gateway::client::Client;
use order_gateway::{ClientSpec, Config, Gateway, Profile, Role, SimBroker, Timing, start};
use serde_json::Value;
use std::path::PathBuf;
use std::time::Duration;

pub const TOKEN: &str = "abababababababababababababababababababababababababababababababab";
/// 2026-10-05 10:00 `America/New_York` (a Monday; SPY261005 expires that day).
pub const T0: u64 = 1_791_208_800_000_000_000;
/// 2026-10-05 15:29:30 `America/New_York`.
pub const CUTOFF: u64 = 1_791_228_570_000_000_000;
pub const CALL: &str = "SPY261005C00580000";
pub const PUT_A: &str = "SPY261005P00570000";
pub const PUT_B: &str = "SPY261005P00568000";

/// A fresh disposable root under the Cargo target directory.
pub fn root(name: &str) -> PathBuf {
    let p = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("gw-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&p);
    std::fs::create_dir_all(&p).unwrap();
    p
}

pub fn clients() -> Vec<ClientSpec> {
    let c = |name: &str, role, prefix: &str| ClientSpec {
        name: name.into(),
        role,
        prefix: prefix.into(),
        token: TOKEN.into(),
    };
    vec![
        c("reactor", Role::Trade, "wse-"),
        c("other", Role::Trade, "oth-"),
        c("watch", Role::Observe, ""),
        c("ops", Role::Admin, ""),
    ]
}

pub fn config(state: PathBuf) -> Config {
    let ms = Duration::from_millis;
    Config {
        account: "sim-1".into(),
        environment: "paper".into(),
        listen: "127.0.0.1:0".parse().unwrap(),
        state,
        clients: clients(),
        profile: Profile::default(),
        timing: Timing {
            resolve: vec![ms(20), ms(40), ms(80)],
            stream_grace: ms(100),
            poll: ms(50),
            disconnect_grace: ms(100),
            ..Timing::default()
        },
        stall_after_admitted: false,
    }
}

pub fn scripted(cfg: Config, script: &str) -> Gateway {
    start(cfg, SimBroker::scripted(script).unwrap()).unwrap()
}

/// A connected client that parses every line.
pub struct Conn {
    pub c: Client,
    pub lines: Vec<String>,
}

impl Conn {
    pub fn open(gw: &Gateway, name: &str, resume: Option<u64>) -> Self {
        Self::with(gw, name, resume, false)
    }
    pub fn with(gw: &Gateway, name: &str, resume: Option<u64>, cod: bool) -> Self {
        let c = Client::connect(gw.addr(), name, TOKEN, resume, cod).unwrap();
        c.set_timeout(Duration::from_secs(5)).unwrap();
        let mut conn = Self {
            c,
            lines: Vec::new(),
        };
        let w = conn.next();
        assert_eq!(w["type"], "welcome", "{w}");
        conn
    }
    pub fn next(&mut self) -> Value {
        let line = self.c.line().unwrap().expect("open connection");
        self.lines.push(line.clone());
        serde_json::from_str(&line).unwrap()
    }
    /// Reads until `pred` matches; returns every value read, the match last.
    pub fn until(&mut self, pred: impl Fn(&Value) -> bool) -> Vec<Value> {
        let mut out = Vec::new();
        loop {
            let v = self.next();
            let done = pred(&v);
            out.push(v);
            if done {
                return out;
            }
        }
    }
    /// Sends a request and returns `(response, events read before it)`.
    pub fn req(&mut self, ty: &str, rid: &str, fields: &str) -> (Value, Vec<Value>) {
        let body = format!(
            "{{\"request_id\":\"{rid}\"{}{}",
            if fields.is_empty() { "" } else { "," },
            fields
        );
        self.c.send(ty, &format!("{body}}}")).unwrap();
        let mut out = self.until(|v| v["type"] == "response" && v["request_id"] == rid);
        let r = out.pop().unwrap();
        (r, out)
    }
    pub fn submit(&mut self, rid: &str, intent: &str) -> Value {
        self.req("submit", rid, &format!("\"intent\":{intent}")).0
    }
    /// The next `order` event with the given normalized `event`.
    pub fn order_event(&mut self, cid: &str, event: &str) -> Value {
        self.until(|v| v["kind"] == "order" && v["client_order_id"] == cid && v["event"] == event)
            .pop()
            .unwrap()
    }
    pub fn event(&mut self, kind: &str, pred: impl Fn(&Value) -> bool) -> Value {
        self.until(|v| v["kind"] == kind && pred(v)).pop().unwrap()
    }
}

pub fn single(cid: &str, symbol: &str, side: &str, effect: &str, qty: u32, price: &str) -> String {
    format!(
        r#"{{"client_order_id":"{cid}","shape":"single","instrument":{{"class":"option","symbol":"{symbol}"}},"side":"{side}","effect":"{effect}","qty":{qty},"type":"limit","limit_price":"{price}","time_in_force":"day"}}"#
    )
}

pub fn buy(cid: &str) -> String {
    single(cid, CALL, "buy", "open", 1, "1.25")
}

/// `legs`: `(symbol, ratio, side, effect)`.
pub fn combo(cid: &str, legs: &[(&str, u32, &str, &str)], qty: u32, price: &str) -> String {
    let legs: Vec<String> = legs
        .iter()
        .map(|(s, r, side, eff)| {
            format!(r#"{{"instrument":{{"class":"option","symbol":"{s}"}},"ratio":{r},"side":"{side}","effect":"{eff}"}}"#)
        })
        .collect();
    format!(
        r#"{{"client_order_id":"{cid}","shape":"combo","legs":[{}],"qty":{qty},"type":"limit","limit_price":"{price}","time_in_force":"day"}}"#,
        legs.join(",")
    )
}

pub fn credit_spread(cid: &str, qty: u32) -> String {
    combo(
        cid,
        &[(PUT_A, 1, "sell", "open"), (PUT_B, 1, "buy", "open")],
        qty,
        "-0.40",
    )
}

/// Journal lines of every day file, in order.
pub fn journal(state: &std::path::Path) -> Vec<String> {
    let mut names: Vec<_> = std::fs::read_dir(state.join("journal"))
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    names.sort();
    names
        .iter()
        .flat_map(|p| {
            std::fs::read_to_string(p)
                .unwrap()
                .lines()
                .map(String::from)
                .collect::<Vec<_>>()
        })
        .collect()
}

/// The simulator's send counter from an `account` query.
pub fn sends(conn: &mut Conn, rid: &str) -> u64 {
    let (r, _) = conn.req("account", rid, "");
    r["broker_raw"]["sim"]["submits"].as_u64().unwrap()
}

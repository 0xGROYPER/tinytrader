//! Live Alpaca PAPER smoke test. Ignored: the parent runs it only with operator
//! authorization naming the account, during regular market hours:
//!
//! ```text
//! GATEWAY_ALPACA_SMOKE_ACCOUNT=alpaca-paper-N \
//! GATEWAY_ALPACA_SMOKE_CREDENTIALS=<a mode-600 file with the paper keys> \
//! GATEWAY_ALPACA_SMOKE_SYMBOL=SPY261120C00600000 \
//! //!   cargo test -p order-gateway --test alpaca_paper_smoke -- --ignored --nocapture
//! ```
//!
//! It submits ONE buy-to-open limit order for 1 contract of the named liquid,
//! not-expiring-today option at $0.01 (far below the market), queries it, cancels it,
//! confirms `canceled` and checks that positions did not change.
mod support;

use order_gateway::{AlpacaPaper, Profile, load_credentials, start};
use serde_json::Value;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use support::{Conn, config, root, single};

fn env(k: &str) -> String {
    std::env::var(k).unwrap_or_else(|_| panic!("{k} is required (see the file header)"))
}

#[test]
#[ignore = "live paper order; needs operator authorization naming the account"]
fn paper_single_far_from_market_then_cancel() {
    let account = env("GATEWAY_ALPACA_SMOKE_ACCOUNT");
    assert!(account.starts_with("alpaca-paper-"), "paper accounts only");
    let symbol = env("GATEWAY_ALPACA_SMOKE_SYMBOL");
    let creds = load_credentials(env("GATEWAY_ALPACA_SMOKE_CREDENTIALS").as_ref()).unwrap();
    let mut cfg = config(root("alpaca-paper-smoke"));
    cfg.account = account;
    cfg.timing = order_gateway::Timing::default();
    cfg.profile = Profile {
        underlyings: vec![symbol[..symbol.len() - 15].to_owned()],
        max_open: 1,
        max_notional_cents: 100,
        ..Profile::default()
    };
    let gw = start(cfg, AlpacaPaper::paper(creds)).unwrap();
    let mut c = Conn::open(&gw, "reactor", Some(0));
    c.c.set_timeout(Duration::from_mins(1)).unwrap();
    // The startup resync, then the resync after the first `listening`.
    let resyncs = std::cell::Cell::new(0);
    let seen = c.until(|v| {
        resyncs
            .set(resyncs.get() + usize::from(v["kind"] == "status" && v.get("resync").is_some()));
        resyncs.get() == 2
    });
    println!("ready: {}", seen.last().unwrap());
    let (before, _) = c.req("positions", "p0", "");
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let cid = format!("wse-smoke-{secs}");
    let sub = c.submit("s1", &single(&cid, &symbol, "buy", "open", 1, "0.01"));
    println!("submit: {sub}");
    assert_eq!(sub["outcome"], "accepted");
    let ack = c.event("order", |v| v["client_order_id"] == cid.as_str());
    println!("first stream event: {ack}");
    let (q, _) = c.req("order", "q1", &format!(r#""client_order_id":"{cid}""#));
    println!("order: {q}");
    assert_eq!(q["order"]["client_order_id"], cid.as_str());
    let (x, _) = c.req("cancel", "c1", &format!(r#""client_order_id":"{cid}""#));
    println!("cancel: {x}");
    assert_eq!(x["outcome"], "accepted");
    let done = c.event("order", |v| {
        v["client_order_id"] == cid.as_str()
            && matches!(v["status"].as_str(), Some("canceled" | "filled"))
    });
    println!("terminal: {done}");
    assert_eq!(
        done["status"], "canceled",
        "the far-from-market order must not fill"
    );
    let (q, _) = c.req("order", "q2", &format!(r#""client_order_id":"{cid}""#));
    assert_eq!(q["order"]["status"], "canceled");
    let (after, _) = c.req("positions", "p1", "");
    let list = |v: &Value| v["positions"].to_string();
    assert_eq!(list(&before), list(&after), "positions changed");
    gw.stop().unwrap();
}

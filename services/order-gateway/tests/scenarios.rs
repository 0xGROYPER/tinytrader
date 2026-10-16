//! Reactor `FakeBroker` scenario matrix through the real gateway over TCP (scripted mode).
mod support;
use serde_json::Value;
use support::*;

fn script(rules: &str) -> String {
    format!(
        r#"{{"clock_ts":{T0},"quotes":{{"{CALL}":{{"bid":"1.20","ask":"1.24"}},"{PUT_A}":{{"bid":"0.93","ask":"0.95"}},"{PUT_B}":{{"bid":"0.50","ask":"0.52"}}}},"rules":[{rules}]}}"#
    )
}

#[test]
#[expect(clippy::many_single_char_names, reason = "short response bindings")]
fn accept_reject_fill_partial_and_skew() {
    let rules = r#"{"client_order_id":"wse-r403","submit":{"reject":{"http":403,"code":40310000,"message":"insufficient options buying power"}}},
        {"client_order_id":"wse-r422","submit":{"reject":{"http":422,"code":40010001,"message":"client_order_id must be unique"}}},
        {"client_order_id":"wse-fill","submit":"fill"},
        {"client_order_id":"wse-part","submit":{"partial":1}},
        {"client_order_id":"wse-skew","submit":"fill","skew":{"SPY261005P00568000":1}}"#;
    let gw = scripted(config(root("matrix")), &script(rules));
    let mut c = Conn::open(&gw, "reactor", None);

    let r = c.submit("r1", &buy("wse-acc"));
    assert_eq!(
        (r["outcome"].as_str(), r["status"].as_str()),
        (Some("accepted"), Some("new"))
    );
    assert_eq!(r["broker_raw"]["client_order_id"], "wse-acc");
    c.order_event("wse-acc", "new");

    for (cid, http, code) in [("wse-r403", 403, "40310000"), ("wse-r422", 422, "40010001")] {
        let r = c.submit(cid, &buy(cid));
        assert_eq!(r["outcome"], "rejected");
        assert_eq!(
            (r["http_status"].as_u64(), r["code"].as_str()),
            (Some(http), Some(code))
        );
        assert_eq!(r["broker_raw"]["code"], code);
    }

    c.submit("r4", &buy("wse-fill"));
    let f = c.order_event("wse-fill", "fill");
    assert_eq!(f["fill"]["price"], "1.24");
    assert_eq!(f["fill"]["fees"], "0.02");
    assert_eq!(
        (f["filled_qty"].as_u64(), f["position_qty"].as_i64()),
        (Some(1), Some(1))
    );
    assert_eq!(
        (f["source"].as_str(), f["broker_event"].as_str()),
        (Some("sim"), Some("fill"))
    );

    c.submit("r5", &credit_spread("wse-part", 3));
    let p = c.order_event("wse-part", "partial_fill");
    assert_eq!(
        (p["status"].as_str(), p["filled_qty"].as_u64()),
        (Some("partially_filled"), Some(1))
    );
    assert_eq!(p["fill"]["price"], "-0.41"); // 0.52 - 0.93
    assert_eq!(p["legs"][0]["fill"]["price"], "0.93");
    assert_eq!(p["legs"][1]["avg_price"], "0.52");

    c.submit("r6", &credit_spread("wse-skew", 2));
    let s = c.order_event("wse-skew", "fill");
    let legs = s["legs"].as_array().unwrap();
    assert_eq!(
        (
            legs[0]["filled_qty"].as_u64(),
            legs[1]["filled_qty"].as_u64()
        ),
        (Some(2), Some(1))
    );

    let (pos, _) = c.req("positions", "r7", "");
    assert!(
        pos["positions"]
            .as_array()
            .unwrap()
            .iter()
            .any(|p| p["symbol"] == CALL && p["qty"] == 1)
    );
    gw.stop().unwrap();
}

#[test]
fn ignored_cancel_and_fill_on_query() {
    let rules = r#"{"client_order_id":"wse-ign","cancel":"ignore"},{"client_order_id":"wse-q","fill_on_query":true}"#;
    let gw = scripted(config(root("ignore")), &script(rules));
    let mut c = Conn::open(&gw, "reactor", None);
    c.submit("r1", &buy("wse-ign"));
    let (r, _) = c.req("cancel", "r2", r#""client_order_id":"wse-ign""#);
    assert_eq!(r["outcome"], "accepted");
    let (q, _) = c.req("order", "r3", r#""client_order_id":"wse-ign""#);
    assert_eq!(
        q["order"]["status"], "new",
        "an ignored cancel leaves the order working"
    );
    assert!(
        q["order"]["intent_sha256"]
            .as_str()
            .is_some_and(|s| s.len() == 64)
    );

    c.submit("r4", &buy("wse-q"));
    let (q, _) = c.req("order", "r5", r#""client_order_id":"wse-q""#);
    assert_eq!(q["order"]["status"], "filled");
    c.order_event("wse-q", "fill");
    let (q, _) = c.req("order", "r6", r#""client_order_id":"wse-none""#);
    assert!(q["order"].is_null());
    let (q, _) = c.req("cancel", "r7", r#""client_order_id":"wse-none""#);
    assert_eq!(q["code"], "not_found");
    gw.stop().unwrap();
}

#[test]
fn duplicate_and_duplicate_conflict_never_send() {
    let gw = scripted(config(root("dup")), &script(""));
    let mut c = Conn::open(&gw, "reactor", None);
    c.submit("r1", &buy("wse-d"));
    let r = c.submit("r2", &buy("wse-d"));
    assert_eq!(r["outcome"], "duplicate");
    assert_eq!(
        r["current"]["broker_order_id"],
        "00000000-0000-4000-8000-000000000001"
    );
    let r = c.submit("r3", &single("wse-d", CALL, "buy", "open", 2, "1.25"));
    assert_eq!(
        (r["outcome"].as_str(), r["reason"].as_str()),
        (Some("not_sent"), Some("duplicate_conflict"))
    );
    assert_eq!(sends(&mut c, "r4"), 1);
    gw.stop().unwrap();
}

#[test]
fn unknown_outcomes_resolve_by_lookup_without_resend() {
    let rules = r#"{"client_order_id":"wse-lost","submit":"timeout_after_accept"},{"client_order_id":"wse-err","submit":"error_before_send"}"#;
    let gw = scripted(config(root("unknown")), &script(rules));
    let mut c = Conn::open(&gw, "reactor", None);
    let r = c.submit("r1", &buy("wse-lost"));
    assert_eq!(
        (r["outcome"].as_str(), r["reason"].as_str()),
        (Some("unknown"), Some("timeout"))
    );
    let res = c.event("resolution", |v| v["client_order_id"] == "wse-lost");
    assert_eq!(
        (res["outcome"].as_str(), res["window_ms"].as_u64()),
        (Some("found"), Some(20))
    );
    let o = c.order_event("wse-lost", "new");
    assert_eq!(o["source"], "rest_reconcile");
    assert_eq!(sends(&mut c, "r2"), 1, "the lost submit was sent once");

    let r = c.submit("r3", &buy("wse-err"));
    assert_eq!(
        (r["outcome"].as_str(), r["reason"].as_str()),
        (Some("unknown"), Some("connection"))
    );
    let res = c.event("resolution", |v| v["client_order_id"] == "wse-err");
    assert_eq!(
        (res["outcome"].as_str(), res["window_ms"].as_u64()),
        (Some("absent_after"), Some(80))
    );
    assert_eq!(sends(&mut c, "r4"), 2);
    let r = c.submit("r5", &buy("wse-err"));
    assert_eq!(r["outcome"], "duplicate", "an absent id stays reserved");
    gw.stop().unwrap();
}

#[test]
fn client_side_throttle_is_not_sent_and_sheds_polls_first() {
    let state = root("throttle");
    let mut cfg = config(state.clone());
    cfg.profile.budget_per_min = 6;
    let gw = scripted(cfg, &script(""));
    let mut c = Conn::open(&gw, "reactor", None);
    let mut accepted = 0;
    let mut shed_poll = false;
    for n in 0..6 {
        let (a, _) = c.req("clock", &format!("q{n}"), "");
        shed_poll |= a["code"] == "throttled";
        let r = c.submit(&format!("r{n}"), &buy(&format!("wse-t{n}")));
        if r["outcome"] == "accepted" {
            accepted += 1;
        } else {
            assert_eq!(
                (r["outcome"].as_str(), r["reason"].as_str()),
                (Some("not_sent"), Some("throttled"))
            );
            break;
        }
    }
    assert!(shed_poll, "polls are shed before critical requests");
    assert!(accepted >= 1);
    gw.stop().unwrap();
    let sent = journal(&state)
        .iter()
        .filter(|l| l.contains(r#""phase":"sent""#))
        .count();
    assert_eq!(sent, accepted, "throttled submits never reached the broker");
}

#[test]
fn http_429_blocks_until_retry_after_and_is_never_resent() {
    let rules =
        r#"{"client_order_id":"wse-429","submit":{"http_429":{"headers":{"Retry-After":"1"}}}}"#;
    let gw = scripted(config(root("429")), &script(rules));
    let mut c = Conn::open(&gw, "reactor", None);
    let r = c.submit("r1", &buy("wse-429"));
    assert_eq!(r["reason"], "http_429_after_send");
    let r = c.submit("r2", &buy("wse-after"));
    assert_eq!(r["reason"], "throttled");
    let (q, _) = c.req("account", "r3", "");
    assert_eq!(q["code"], "throttled");
    // The lookups are refused by the budget while the 429 backoff runs, so the
    // resolution comes after it: the broker's 404 (the 429 never stored the
    // order) is what makes it absent. Look at lines already read first.
    let is_res = |v: &Value| v["kind"] == "resolution" && v["client_order_id"] == "wse-429";
    let seen = c
        .lines
        .iter()
        .map(|l| serde_json::from_str::<Value>(l).unwrap())
        .find(|v| is_res(v));
    let res = seen.unwrap_or_else(|| c.event("resolution", |v| v["client_order_id"] == "wse-429"));
    assert_eq!(
        (res["outcome"].as_str(), res["window_ms"].as_u64()),
        (Some("absent_after"), Some(80))
    );
    std::thread::sleep(std::time::Duration::from_millis(1100));
    assert_eq!(c.submit("r4", &buy("wse-after"))["outcome"], "accepted");
    assert_eq!(sends(&mut c, "r5"), 2);
    gw.stop().unwrap();
}

#[test]
fn consecutive_rejects_trip_halt_opens() {
    let mut cfg = config(root("rejects"));
    cfg.profile.max_rejects = 2;
    let rule = r#"{"submit":{"reject":{"http":403,"code":40310000,"message":"no"}}}"#;
    let text = script("").replace(r#""rules":[]"#, &format!(r#""default":{rule}"#));
    let gw = scripted(cfg, &text);
    let mut c = Conn::open(&gw, "reactor", None);
    c.submit("r1", &buy("wse-1"));
    c.submit("r2", &buy("wse-2"));
    let st = c.event("status", |v| v.get("cap").is_some());
    assert_eq!(st["kill"]["level"], "halt_opens");
    assert_eq!(st["cap"]["name"], "consecutive_rejects");
    let r = c.submit("r3", &buy("wse-3"));
    assert_eq!(r["reason"], "killed");
    let r = c.submit("r4", &single("wse-4", CALL, "sell", "close", 1, "1.00"));
    assert_eq!(
        r["outcome"], "rejected",
        "closing intents still pass halt_opens"
    );
    gw.stop().unwrap();
}

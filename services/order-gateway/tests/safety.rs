//! Safety and session rules: persistent kill switch, caps, stream drop resync,
//! `cancel_on_disconnect`, resume and slow consumers.
mod support;
use std::time::Duration;
use support::*;

fn clocked() -> String {
    format!(r#"{{"clock_ts":{T0}}}"#)
}

/// The clock, and options held long: closes are checked against the position held.
fn clocked_holding() -> String {
    format!(
        r#"{{"clock_ts":{T0},"positions":{{"{CALL}":10,"{PUT_A}":10,"{PUT_B}":10,"{FAR}":10}}}}"#
    )
}

#[test]
fn kill_levels_persist_across_restart_until_unkill() {
    let state = root("kill");
    let gw = scripted(config(state.clone()), &clocked_holding());
    let mut c = Conn::open(&gw, "reactor", None);
    c.submit("r1", &buy("wse-1"));
    let mut ops = Conn::open(&gw, "ops", None);
    let (r, _) = ops.req("kill", "k1", r#""level":"halt_opens","reason":"test""#);
    assert_eq!(r["state"], "killed");
    assert_eq!(c.submit("r2", &buy("wse-2"))["reason"], "killed");
    // The resting buy on CALL is working, so the close goes on PUT_A.
    let close = single("wse-3", PUT_A, "sell", "close", 1, "1.00");
    assert_eq!(c.submit("r3", &close)["outcome"], "accepted");
    ops.req("kill", "k2", r#""level":"halt_all","reason":"test""#);
    // halt_all cancels every open order.
    c.order_event("wse-1", "canceled");
    c.order_event("wse-3", "canceled");
    let close = single("wse-4", CALL, "sell", "close", 1, "1.00");
    assert_eq!(c.submit("r4", &close)["reason"], "killed");
    gw.stop().unwrap();

    let gw = scripted(config(state.clone()), &clocked_holding());
    let mut c = Conn::open(&gw, "reactor", None);
    assert_eq!(
        c.submit("r5", &close.replace("wse-4", "wse-5"))["reason"],
        "killed"
    );
    let mut ops = Conn::open(&gw, "ops", None);
    assert_eq!(
        ops.req("unkill", "u1", r#""reason":"done""#).0["state"],
        "ready"
    );
    assert_eq!(c.submit("r6", &buy("wse-6"))["outcome"], "accepted");
    gw.stop().unwrap();

    std::fs::write(state.join("KILL"), b"").unwrap();
    let gw = scripted(config(state.clone()), &clocked_holding());
    let mut c = Conn::open(&gw, "reactor", None);
    let st = c.event("status", |v| v.get("kill").is_some());
    assert_eq!(
        (st["kill"]["level"].as_str(), st["kill"]["reason"].as_str()),
        (Some("halt_all"), Some("sentinel"))
    );
    gw.stop().unwrap();
}

#[test]
fn each_cap_refuses_before_any_send() {
    let mut cfg = config(root("caps"));
    cfg.profile.max_open = 2;
    cfg.profile.max_legs = 2;
    cfg.profile.submits_per_min = 3;
    let gw = scripted(cfg, &clocked());
    let mut c = Conn::open(&gw, "reactor", None);
    let cap = |r: serde_json::Value| {
        (
            r["reason"].as_str().map(String::from),
            r["detail"]["cap"].as_str().map(String::from),
        )
    };
    let want = |name: &str| (Some("cap".to_string()), Some(name.to_string()));
    let three = combo(
        "wse-l",
        &[
            (PUT_A, 1, "buy", "open"),
            (PUT_B, 1, "sell", "open"),
            (CALL, 1, "buy", "open"),
        ],
        1,
        "0.10",
    );
    assert_eq!(cap(c.submit("l", &three)), want("legs"));
    assert_eq!(
        cap(c.submit("n", &single("wse-n", CALL, "buy", "open", 10, "2.51"))),
        want("notional")
    );
    // A market order's notional is its reference price's.
    let market = |s: String| {
        s.replace(
            r#""type":"limit","limit_price""#,
            r#""type":"market","ref_price""#,
        )
    };
    assert_eq!(
        cap(c.submit(
            "nm",
            &market(single("wse-nm", CALL, "buy", "open", 10, "2.51"))
        )),
        want("notional")
    );
    c.submit("a1", &buy("wse-a1"));
    c.submit("a2", &buy("wse-a2"));
    assert_eq!(cap(c.submit("o", &buy("wse-o"))), want("open_orders"));
    c.req("cancel", "c1", r#""client_order_id":"wse-a1""#);
    c.submit("a3", &buy("wse-a3"));
    c.req("cancel", "c2", r#""client_order_id":"wse-a2""#);
    assert_eq!(
        cap(c.submit("m", &buy("wse-m"))),
        want("submits_per_minute")
    );
    gw.stop().unwrap();

    let mut cfg = config(root("caps-day"));
    cfg.profile.submits_per_day = 1;
    let gw = scripted(cfg, &clocked());
    let mut c = Conn::open(&gw, "reactor", None);
    c.submit("d1", &buy("wse-d1"));
    assert_eq!(cap(c.submit("d2", &buy("wse-d2"))), want("submits_per_day"));
    gw.stop().unwrap();
}

/// The `cap` name of a refusal, or `None`.
fn cap_name(r: &serde_json::Value) -> Option<String> {
    (r["reason"] == "cap")
        .then(|| r["detail"]["cap"].as_str().map(String::from))
        .flatten()
}

fn close(cid: &str) -> String {
    close_on(cid, CALL)
}

fn close_on(cid: &str, symbol: &str) -> String {
    single(cid, symbol, "sell", "close", 1, "1.00")
}

/// A fourth option, for a close on a leg with no order of its own.
const FAR: &str = "SPY261005C00590000";

#[test]
fn a_close_is_not_counted_toward_the_daily_cap() {
    let mut cfg = config(root("caps-day-close"));
    cfg.profile.submits_per_day = 1;
    cfg.profile.max_open = 10;
    let gw = scripted(cfg, &clocked_holding());
    let mut c = Conn::open(&gw, "reactor", None);
    // Each close is on its own leg: a working close or open on a leg refuses a close there.
    assert_eq!(c.submit("c1", &close("wse-c1"))["outcome"], "accepted");
    assert_eq!(
        c.submit("c2", &close_on("wse-c2", PUT_A))["outcome"],
        "accepted"
    );
    assert_eq!(
        c.submit("o1", &buy("wse-o1"))["outcome"],
        "accepted",
        "closes used none of the day's opens"
    );
    assert_eq!(
        cap_name(&c.submit("o2", &buy("wse-o2"))),
        Some("submits_per_day".into())
    );
    assert_eq!(
        c.submit("c3", &close_on("wse-c3", FAR))["outcome"],
        "accepted",
        "a close passes the day cap the opens have used up"
    );
    gw.stop().unwrap();
}

#[test]
fn closes_per_minute_refuses_closes_and_never_opens() {
    let mut cfg = config(root("closes-per-min"));
    cfg.profile.closes_per_min = 2;
    cfg.profile.max_open = 10;
    let gw = scripted(cfg, &clocked_holding());
    let mut c = Conn::open(&gw, "reactor", None);
    assert_eq!(c.submit("c1", &close("wse-c1"))["outcome"], "accepted");
    assert_eq!(
        c.submit("c2", &close_on("wse-c2", PUT_A))["outcome"],
        "accepted"
    );
    assert_eq!(
        cap_name(&c.submit("c3", &close_on("wse-c3", PUT_B))),
        Some("closes_per_minute".into())
    );
    assert_eq!(
        c.submit("o1", &buy("wse-o1"))["outcome"],
        "accepted",
        "the closes budget is not the opens budget"
    );
    assert_eq!(sends(&mut c, "s1"), 3, "a refused close is never sent");
    gw.stop().unwrap();
}

#[test]
fn restart_counts_only_opens_toward_the_day() {
    let state = root("day-restart");
    let day_cap = |n: usize| {
        let mut cfg = config(state.clone());
        cfg.profile.submits_per_day = n;
        cfg.profile.max_open = 10;
        cfg
    };
    let gw = scripted(day_cap(1), &clocked_holding());
    let mut c = Conn::open(&gw, "reactor", None);
    assert_eq!(c.submit("c1", &close("wse-c1"))["outcome"], "accepted");
    gw.stop().unwrap();

    let gw = scripted(day_cap(1), &clocked_holding());
    let mut c = Conn::open(&gw, "reactor", None);
    assert_eq!(
        c.submit("o1", &buy("wse-o1"))["outcome"],
        "accepted",
        "the day count rebuilt from the journal has no close in it"
    );
    gw.stop().unwrap();

    let gw = scripted(day_cap(1), &clocked_holding());
    let mut c = Conn::open(&gw, "reactor", None);
    assert_eq!(
        cap_name(&c.submit("o2", &buy("wse-o2"))),
        Some("submits_per_day".into()),
        "an open journaled today still counts"
    );
    gw.stop().unwrap();
}

#[test]
fn a_429_backoff_holds_opens_but_not_closes() {
    let rule =
        r#"{"client_order_id":"wse-429","submit":{"http_429":{"headers":{"Retry-After":"1"}}}}"#;
    let script = format!(r#"{{"clock_ts":{T0},"positions":{{"{PUT_A}":1}},"rules":[{rule}]}}"#);
    let gw = scripted(config(root("backoff-close")), &script);
    let mut c = Conn::open(&gw, "reactor", None);
    let r = c.submit("r1", &buy("wse-429"));
    assert_eq!(r["reason"], "http_429_after_send");
    assert_eq!(
        c.submit("r2", &buy("wse-open"))["reason"],
        "throttled",
        "the backoff holds opens"
    );
    assert_eq!(
        c.submit("r3", &close_on("wse-close", PUT_A))["outcome"],
        "accepted",
        "and does not hold closes"
    );
    gw.stop().unwrap();
}

#[test]
fn stream_drop_degrades_then_resyncs_to_ready() {
    let script = format!(
        r#"{{"clock_ts":{T0},"stream":{{"drop_after_events":1,"down_ms":400}},"rules":[{{"nth":2,"submit":"fill"}}]}}"#
    );
    let gw = scripted(config(root("stream")), &script);
    let mut c = Conn::open(&gw, "reactor", None);
    c.submit("r1", &buy("wse-1"));
    c.order_event("wse-1", "new");
    let st = c.event("status", |v| v["state"] == "degraded");
    assert!(st["broker_stream"]["down"]["since"].is_u64());
    assert_eq!(c.submit("r2", &buy("wse-2"))["reason"], "not_ready");
    let (r, _) = c.req("cancel", "r3", r#""client_order_id":"wse-1""#);
    assert_eq!(
        r["outcome"], "accepted",
        "cancels are allowed while degraded"
    );
    // The cancel happened while the stream was down; the 1 Hz poll or resync reports it.
    let o = c.order_event("wse-1", "canceled");
    assert_eq!(o["source"], "rest_reconcile");
    let st = c.event("status", |v| v.get("resync").is_some());
    assert_eq!(st["state"], "ready");
    assert_eq!(c.submit("r4", &buy("wse-4"))["outcome"], "accepted");
    gw.stop().unwrap();
}

#[test]
fn cancel_on_disconnect_is_opt_in() {
    let gw = scripted(config(root("cod")), &clocked());
    let mut watch = Conn::open(&gw, "watch", None);
    let mut keep = Conn::open(&gw, "other", None);
    keep.submit("o1", &buy("oth-1"));
    drop(keep);
    let mut c = Conn::with(&gw, "reactor", None, true);
    c.submit("r1", &buy("wse-1"));
    drop(c);
    let ev = watch.order_event("wse-1", "canceled");
    assert_eq!(ev["status"], "canceled");
    std::thread::sleep(Duration::from_millis(200));
    let (q, _) = watch.req("order", "w1", r#""client_order_id":"oth-1""#);
    assert_eq!(
        q["order"]["status"], "new",
        "no automatic cancel without opt-in"
    );
    let cancels: Vec<_> = watch
        .lines
        .iter()
        .filter(|l| l.contains("cancel_on_disconnect"))
        .collect();
    assert_eq!(cancels.len(), 2, "sent + outcome: {cancels:?}");
    gw.stop().unwrap();
}

#[test]
fn reconnect_resumes_an_identical_suffix() {
    let gw = scripted(
        config(root("resume")),
        &format!(r#"{{"clock_ts":{T0},"default":{{"submit":"fill"}}}}"#),
    );
    let mut watch = Conn::open(&gw, "watch", Some(0));
    let mut c = Conn::open(&gw, "reactor", None);
    c.submit("r1", &buy("wse-1"));
    c.order_event("wse-1", "fill");
    let mut early = Conn::open(&gw, "ops", Some(0));
    let cut = early.order_event("wse-1", "new")["seq"].as_u64().unwrap();
    drop(early);
    c.submit("r2", &buy("wse-2"));
    c.order_event("wse-2", "fill");
    let last = watch.order_event("wse-2", "fill")["seq"].as_u64().unwrap();
    let mut back = Conn::open(&gw, "ops", Some(cut));
    let resumed = back.until(|v| v["seq"] == last);
    let reference: Vec<String> = watch
        .lines
        .iter()
        .filter(|l| {
            let v: serde_json::Value = serde_json::from_str(l).unwrap();
            v["seq"].as_u64().is_some_and(|s| s > cut && s <= last)
        })
        .cloned()
        .collect();
    assert_eq!(back.lines[1..].to_vec(), reference);
    assert_eq!(resumed.len(), reference.len());
    gw.stop().unwrap();
}

#[test]
fn slow_consumer_is_closed_and_recovers_by_resume() {
    let mut cfg = config(root("slow"));
    cfg.timing.queue_bytes = 8000;
    let gw = scripted(
        cfg,
        &format!(r#"{{"clock_ts":{T0},"default":{{"submit":"fill"}}}}"#),
    );
    let mut c = Conn::open(&gw, "reactor", None);
    for n in 0..10 {
        c.submit(&format!("r{n}"), &buy(&format!("wse-{n}")));
        c.order_event(&format!("wse-{n}"), "fill");
    }
    let mut slow =
        order_gateway::client::Client::connect(gw.addr(), "watch", TOKEN, Some(0), false).unwrap();
    let first = slow.line().unwrap().unwrap();
    assert!(first.contains("slow_consumer"), "{first}");
    assert_eq!(slow.line().unwrap(), None, "closed after slow_consumer");
    // A bounded resume from a recent seq fits the queue.
    let recent = c
        .lines
        .iter()
        .rev()
        .find_map(|l| serde_json::from_str::<serde_json::Value>(l).ok()?["seq"].as_u64());
    let mut c2 = Conn::open(&gw, "watch", recent.map(|s| s - 2));
    assert_eq!(c2.next()["seq"], recent.unwrap() - 1);
    gw.stop().unwrap();
}

#[test]
fn flatten_closes_every_position_and_halts_opens() {
    let script = format!(
        r#"{{"clock_ts":{T0},"positions":{{"{CALL}":2,"{PUT_A}":-1}},"quotes":{{"{CALL}":{{"bid":"1.20","ask":"1.30"}}}}}}"#
    );
    let gw = scripted(config(root("flatten")), &script);
    let mut c = Conn::open(&gw, "reactor", None);
    assert_eq!(c.submit("r1", &buy("wse-1"))["outcome"], "accepted");
    // Only an admin may flatten.
    assert_eq!(
        c.req("flatten", "f0", r#""reason":"test""#).0["code"],
        "forbidden"
    );
    let mut ops = Conn::open(&gw, "ops", None);
    let (r, _) = ops.req("flatten", "f1", r#""reason":"test""#);
    assert_eq!(
        (r["outcome"].as_str(), r["state"].as_str()),
        (Some("ok"), Some("killed"))
    );
    let mut closed: Vec<String> = r["positions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["symbol"].as_str().unwrap().to_owned())
        .collect();
    closed.sort();
    assert_eq!(closed, [CALL, PUT_A]);
    // The flatten events follow its response, so the next request reads past them.
    let (p, skipped) = ops.req("positions", "p1", "");
    let ev = skipped
        .iter()
        .find(|v| v["kind"] == "flatten" && v["phase"] == "outcome")
        .expect("a flatten outcome event")
        .clone();
    assert_eq!(p["positions"].as_array().unwrap().len(), 0, "{p}");
    // The resting open order was cancelled; opens stay refused, closes pass.
    c.order_event("wse-1", "canceled");
    assert_eq!(c.submit("r2", &buy("wse-2"))["reason"], "killed");
    // Closes are not stopped by halt_opens, but nothing is held now: the close is refused.
    let close = single("wse-3", CALL, "sell", "close", 1, "1.00");
    let r3 = c.submit("r3", &close);
    assert_eq!(
        (r3["reason"].as_str(), r3["detail"]["held_qty"].as_i64()),
        (Some("close_exceeds_position"), Some(0))
    );
    assert_eq!(ev["outcome"]["outcome"], "ok");
    gw.stop().unwrap();
}

#[test]
fn an_any_underlying_client_skips_only_the_allowlist() {
    let mut cfg = config(root("any-underlying"));
    cfg.profile.any_underlying_clients = vec!["other".into()];
    let gw = scripted(cfg, &clocked());
    let sap = |cid: &str, qty: u32, price: &str| {
        single(cid, "SAP261016C00250000", "buy", "open", qty, price)
    };
    // Another trading client on the same account keeps the allowlist.
    let mut c = Conn::open(&gw, "reactor", None);
    let r = c.submit("r1", &sap("wse-1", 1, "1.00"));
    assert_eq!(
        (r["reason"].as_str(), r["detail"]["underlying"].as_str()),
        (Some("unsupported"), Some("SAP"))
    );
    let mut o = Conn::open(&gw, "other", None);
    assert_eq!(
        o.submit("o1", &sap("oth-1", 1, "1.00"))["outcome"],
        "accepted"
    );
    // Caps still apply.
    assert_eq!(
        o.submit("o3", &sap("oth-3", 1, "26.00"))["detail"]["cap"],
        "notional"
    );
    gw.stop().unwrap();
}

#[test]
fn expiry_day_opens_go_to_the_broker() {
    // No gateway cutoff: the broker's own expiry-day rules decide.
    let gw = scripted(
        config(root("expiry-day")),
        &format!(r#"{{"clock_ts":{LATE}}}"#),
    );
    let mut c = Conn::open(&gw, "reactor", None);
    assert_eq!(c.submit("r1", &buy("wse-1"))["outcome"], "accepted");
    gw.stop().unwrap();
}

#[test]
fn closing_orders_pass_the_notional_and_open_order_caps() {
    let mut cfg = config(root("caps-close"));
    cfg.profile.max_open = 1;
    let gw = scripted(cfg, &clocked_holding());
    let mut c = Conn::open(&gw, "reactor", None);
    assert_eq!(c.submit("a1", &buy("wse-a1"))["outcome"], "accepted");
    // Past the notional cap ($2,500) and with the one open order working on CALL: still sent.
    let r = c.submit("x1", &single("wse-x1", PUT_A, "sell", "close", 10, "2.51"));
    assert_eq!(r["outcome"], "accepted", "{r}");
    // Opening is still held to both.
    assert_eq!(
        c.submit("o1", &buy("wse-o1"))["detail"]["cap"],
        "open_orders"
    );
    gw.stop().unwrap();
}

//! Safety and session rules: persistent kill switch, caps, stream drop resync,
//! `cancel_on_disconnect`, resume and slow consumers.
mod support;
use std::time::Duration;
use support::*;

fn clocked() -> String {
    format!(r#"{{"clock_ts":{T0}}}"#)
}

#[test]
fn kill_levels_persist_across_restart_until_unkill() {
    let state = root("kill");
    let gw = scripted(config(state.clone()), &clocked());
    let mut c = Conn::open(&gw, "reactor", None);
    c.submit("r1", &buy("wse-1"));
    let mut ops = Conn::open(&gw, "ops", None);
    let (r, _) = ops.req("kill", "k1", r#""level":"halt_opens","reason":"test""#);
    assert_eq!(r["state"], "killed");
    assert_eq!(c.submit("r2", &buy("wse-2"))["reason"], "killed");
    let close = single("wse-3", CALL, "sell", "close", 1, "1.00");
    assert_eq!(c.submit("r3", &close)["outcome"], "accepted");
    ops.req("kill", "k2", r#""level":"halt_all","reason":"test""#);
    // halt_all cancels every open order.
    c.order_event("wse-1", "canceled");
    c.order_event("wse-3", "canceled");
    let close = single("wse-4", CALL, "sell", "close", 1, "1.00");
    assert_eq!(c.submit("r4", &close)["reason"], "killed");
    gw.stop().unwrap();

    let gw = scripted(config(state.clone()), &clocked());
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
    let gw = scripted(config(state.clone()), &clocked());
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
    assert_eq!(
        cap(c.submit("q", &single("wse-q", CALL, "buy", "open", 11, "0.10"))),
        want("qty")
    );
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
    let close = single("wse-3", CALL, "sell", "close", 1, "1.00");
    assert_eq!(c.submit("r3", &close)["outcome"], "accepted");
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
    // Caps still apply: contracts and notional.
    assert_eq!(
        o.submit("o2", &sap("oth-2", 11, "0.10"))["detail"]["cap"],
        "qty"
    );
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

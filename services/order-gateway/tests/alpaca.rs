//! `AlpacaPaper` through the real gateway against a loopback fake Alpaca (REST and
//! `trade_updates`), offline.
mod fake_alpaca;
mod support;

use fake_alpaca::{Act, Fake, FakeOrder, KEY, SECRET};
use order_gateway::trading::{Credentials, Endpoints};
use order_gateway::{AlpacaOptions, AlpacaPaper, Gateway, Profile, start};
use serde_json::Value;
use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use support::{CALL, Conn, PUT_A, PUT_B, buy, config, credit_spread, journal, root, single};

struct Rig {
    fake: Fake,
    gw: Gateway,
    state: PathBuf,
}

fn rig(name: &str) -> Rig {
    rig_with(name, |_| {})
}

/// A rig whose profile is the default with `max_open` 10, then `profile` applied.
fn rig_with(name: &str, profile: impl FnOnce(&mut Profile)) -> Rig {
    let fake = Fake::start();
    let creds = Credentials::from_env_text(&format!(
        "export APCA_API_KEY_ID={KEY}\nAPCA_API_SECRET_KEY=\"{SECRET}\"\n"
    ))
    .unwrap();
    let opts = AlpacaOptions {
        timeout: Duration::from_secs(2),
        reconnect_min: Duration::from_millis(50),
        reconnect_max: Duration::from_millis(200),
    };
    let broker = AlpacaPaper::new(Endpoints::loopback(fake.addr).unwrap(), creds, opts);
    let state = root(name);
    let mut cfg = config(state.clone());
    cfg.account = "alpaca-paper-test".into();
    cfg.profile.max_open = 10;
    profile(&mut cfg.profile);
    let gw = start(cfg, broker).unwrap();
    fake.wait_listening(1);
    Rig { fake, gw, state }
}

/// A client that has seen the startup resync and the resync after the first `listening`.
fn ready(gw: &Gateway, name: &str) -> Conn {
    let mut c = Conn::open(gw, name, Some(0));
    let n = std::cell::Cell::new(0);
    c.until(|v| {
        if v["kind"] == "status" && v.get("resync").is_some() {
            n.set(n.get() + 1);
        }
        n.get() == 2
    });
    c
}

fn posts(fake: &Fake) -> Vec<String> {
    let st = fake.lock();
    st.log
        .iter()
        .filter(|(m, p, _)| m == "POST" && p == "/v2/orders")
        .map(|(_, _, b)| b.clone())
        .collect()
}

#[test]
fn exact_bodies_and_every_stream_event() {
    let r = rig("alpaca-events");
    let mut c = ready(&r.gw, "reactor");
    let resp = c.submit("r1", &single("wse-1", CALL, "buy", "open", 2, "1.25"));
    assert_eq!(resp["outcome"], "accepted", "{resp}");
    assert_eq!(resp["status"], "accepted");
    let line = c.lines.last().unwrap();
    assert!(
        line.contains(r#""x_unknown_field":{"kept":1.50}"#),
        "{line}"
    );
    let resp = c.submit("r2", &credit_spread("wse-2", 3));
    assert_eq!(resp["outcome"], "accepted", "{resp}");
    assert_eq!(
        posts(&r.fake),
        [
            r#"{"symbol":"SPY261005C00580000","qty":"2","side":"buy","type":"limit","limit_price":"1.25","time_in_force":"day","client_order_id":"wse-1","position_intent":"buy_to_open"}"#,
            r#"{"order_class":"mleg","qty":"3","type":"limit","time_in_force":"day","limit_price":"-0.40","client_order_id":"wse-2","legs":[{"symbol":"SPY261005P00570000","ratio_qty":"1","side":"sell","position_intent":"sell_to_open"},{"symbol":"SPY261005P00568000","ratio_qty":"1","side":"buy","position_intent":"buy_to_open"}]}"#,
        ]
    );
    for (event, status) in [
        ("pending_new", "pending_new"),
        ("accepted", "accepted"),
        ("new", "new"),
    ] {
        r.fake.event("wse-1", event, status);
        let v = c.order_event("wse-1", event);
        assert_eq!(v["status"], status);
        assert_eq!(v["source"], "stream");
        assert_eq!(v["ts_broker"], 1_791_208_801_123_456_789_u64);
    }
    r.fake.fill("wse-1", 1, "1.24");
    let v = c.order_event("wse-1", "partial_fill");
    assert_eq!(v["fill"]["qty"], 1);
    assert_eq!(v["fill"]["price"], "1.24");
    assert!(
        v["fill"]["execution_id"]
            .as_str()
            .unwrap()
            .starts_with("e000")
    );
    assert_eq!(
        (v["filled_qty"].as_u64(), v["position_qty"].as_i64()),
        (Some(1), Some(1))
    );
    r.fake.fill("wse-1", 1, "1.23");
    let v = c.order_event("wse-1", "fill");
    assert_eq!(
        (v["status"].as_str(), v["filled_qty"].as_u64()),
        (Some("filled"), Some(2))
    );
    assert_eq!(v["broker_raw"]["order"]["filled_avg_price"], "1.23");

    // MLEG: per-leg executions from `data.legs`, the net price at the top.
    r.fake.mleg_fill("wse-2", 1, "-0.41", &["0.93", "0.52"]);
    let v = c.order_event("wse-2", "partial_fill");
    assert_eq!(v["fill"]["price"], "-0.41");
    assert_eq!(v["fill"]["execution_id"], Value::Null);
    let legs = v["legs"].as_array().unwrap();
    assert_eq!(legs[0]["symbol"], PUT_A);
    assert_eq!(legs[0]["filled_qty"], 1);
    assert_eq!(legs[0]["fill"]["price"], "0.93");
    assert_eq!(legs[1]["symbol"], PUT_B);
    assert_eq!(legs[1]["fill"]["qty"], 1);
    assert!(legs[1]["fill"]["execution_id"].is_string());
    assert_eq!(v["broker_raw"]["position_qtys"][PUT_A], "-1");
    r.fake.mleg_fill("wse-2", 2, "-0.40", &["0.92", "0.52"]);
    let v = c.order_event("wse-2", "fill");
    assert_eq!(v["legs"][0]["filled_qty"], 3);

    let terminal = [
        ("canceled", "canceled", "canceled"),
        ("expired", "expired", "expired"),
        ("rejected", "rejected", "rejected"),
        ("replaced", "replaced", "replaced"),
        ("done_for_day", "done_for_day", "done_for_day"),
        ("order_cancel_rejected", "new", "cancel_rejected"),
        ("pending_replace", "pending_replace", "other"),
    ];
    for (n, (event, status, normalized)) in terminal.iter().enumerate() {
        let cid = format!("wse-t{n}");
        let resp = c.submit(&format!("t{n}"), &buy(&cid));
        assert_eq!(resp["outcome"], "accepted", "{resp}");
        r.fake.event(&cid, event, status);
        let v = c.order_event(&cid, normalized);
        assert_eq!(
            (v["broker_event"].as_str(), v["status"].as_str()),
            (Some(*event), Some(*status))
        );
    }
}

#[test]
fn broker_refusals_map_status_and_code() {
    let r = rig("alpaca-rejects");
    let mut c = ready(&r.gw, "reactor");
    let bp = include_str!("fixtures/alpaca/error_buying_power.json");
    r.fake.reply("POST", "/v2/orders", 403, bp);
    let v = c.submit("r1", &buy("wse-a"));
    assert_eq!(
        (v["outcome"].as_str(), v["http_status"].as_u64()),
        (Some("rejected"), Some(403))
    );
    assert_eq!(
        (v["code"].as_str(), v["message"].as_str()),
        (Some("40310000"), Some("insufficient buying power"))
    );
    assert_eq!(v["broker_raw"]["code"], 40_310_000);
    let lvl = include_str!("fixtures/alpaca/error_options_level.json");
    r.fake.reply("POST", "/v2/orders", 403, lvl);
    let v = c.submit("r2", &buy("wse-b"));
    assert!(v["message"].as_str().unwrap().contains("not eligible"));
    // An id the broker already holds (placed outside the gateway): Alpaca's 422.
    let mut st = r.fake.lock();
    let dashboard = FakeOrder {
        id: "00000000-0000-4000-8000-0000000000aa".into(),
        cid: "wse-c".into(),
        single: None,
        legs: Vec::new(),
        qty: 1,
        filled: 0,
        avg: None,
        limit: "1.00".into(),
        status: "canceled".into(),
    };
    st.orders.push(dashboard);
    drop(st);
    let v = c.submit("r3", &buy("wse-c"));
    assert_eq!(
        (v["http_status"].as_u64(), v["code"].as_str()),
        (Some(422), Some("40010001"))
    );
    // A gateway-known id is never sent again.
    let v = c.submit("r4", &buy("wse-c"));
    assert_eq!(v["outcome"], "duplicate");
    assert_eq!(posts(&r.fake).len(), 3);
    r.fake
        .reply("POST", "/v2/orders", 502, "<html>bad gateway</html>");
    let v = c.submit("r5", &buy("wse-d"));
    assert_eq!(
        (v["outcome"].as_str(), v["reason"].as_str()),
        (Some("unknown"), Some("http_5xx"))
    );
}

#[test]
fn lost_response_resolves_by_lookup_without_resend() {
    let r = rig("alpaca-lost");
    let mut c = ready(&r.gw, "reactor");
    r.fake.next("POST", "/v2/orders", Act::Lost);
    let v = c.submit("r1", &buy("wse-lost"));
    assert_eq!(
        (v["outcome"].as_str(), v["reason"].as_str()),
        (Some("unknown"), Some("connection"))
    );
    let res = c.event("resolution", |v| v["client_order_id"] == "wse-lost");
    assert_eq!(res["outcome"], "found");
    let v = c.order_event("wse-lost", "accepted");
    assert_eq!(v["source"], "rest_reconcile");
    assert_eq!(posts(&r.fake).len(), 1);
    assert!(r.fake.count("GET", "/v2/orders:by_client_order_id") >= 1);
}

#[test]
fn a_failed_lookup_leaves_the_order_unknown_not_absent() {
    let r = rig("alpaca-lookup-error");
    let mut c = ready(&r.gw, "reactor");
    // The broker is failing, which is not the same as saying the order is missing:
    // every lookup gets a 500, so the order is unresolved, not absent. The submit's
    // reply is a 502 and the broker never stores the order, so no lookup can find it.
    // That keeps the test about failed lookups and nothing else.
    for _ in 0..3 {
        r.fake.next(
            "GET",
            "/v2/orders:by_client_order_id",
            Act::Respond(500, Vec::new(), "{}".into()),
        );
    }
    r.fake.reply("POST", "/v2/orders", 502, "<html>bad gateway</html>");
    let v = c.submit("r1", &buy("wse-err"));
    assert_eq!(
        (v["outcome"].as_str(), v["reason"].as_str()),
        (Some("unknown"), Some("http_5xx"))
    );
    let res = c.event("resolution", |v| v["client_order_id"] == "wse-err");
    assert_eq!(
        (res["outcome"].as_str(), res["window_ms"].as_u64()),
        (Some("unresolved"), Some(80))
    );
    assert!(r.fake.count("GET", "/v2/orders:by_client_order_id") >= 3);
    // Still unknown and open. The order reaches the broker later, and its stream
    // event settles it.
    r.fake.appear(&format!(
        r#"{{"client_order_id":"wse-err","symbol":"{CALL}","qty":"1","side":"buy","limit_price":"1.25","position_intent":"buy_to_open"}}"#
    ));
    r.fake.event("wse-err", "new", "new");
    let o = c.order_event("wse-err", "new");
    assert_eq!(o["source"], "stream");
    assert_eq!(posts(&r.fake).len(), 1, "never resent");
}

/// A submit whose POST gets a 502 and never reaches the fake, so every lookup is a
/// 404: the order ends up absent, not unknown.
fn absent_submit(r: &Rig, c: &mut Conn, cid: &str) {
    r.fake.reply("POST", "/v2/orders", 502, "<html>bad gateway</html>");
    let v = c.submit("r-abs", &buy(cid));
    assert_eq!(v["outcome"], "unknown");
    let res = c.event("resolution", |v| v["client_order_id"] == cid);
    assert_eq!(res["outcome"], "absent_after");
}

#[test]
fn an_absent_order_still_counts_toward_max_open() {
    let r = rig_with("alpaca-absent-cap", |p| p.max_open = 1);
    let mut c = ready(&r.gw, "reactor");
    absent_submit(&r, &mut c, "wse-abs");
    let (q, _) = c.req("open_orders", "q1", "");
    assert_eq!(q["unresolved"], 1);
    let v = c.submit("r2", &buy("wse-next"));
    assert_eq!(
        (v["reason"].as_str(), v["detail"]["cap"].as_str()),
        (Some("cap"), Some("open_orders"))
    );
    // Nothing refused was sent, and a close on its leg waits for the absent order.
    assert_eq!(posts(&r.fake).len(), 1);
    let v = c.submit("r3", &single("wse-close", CALL, "sell", "close", 1, "1.00"));
    assert_eq!(v["reason"], "leg_pending");
    assert_eq!(v["detail"]["symbols"][0], CALL);
}

#[test]
fn a_stream_event_settles_an_absent_order() {
    let r = rig_with("alpaca-absent-settle", |p| p.max_open = 1);
    let mut c = ready(&r.gw, "reactor");
    absent_submit(&r, &mut c, "wse-abs");
    let v = c.submit("r2", &buy("wse-next"));
    assert_eq!(v["detail"]["cap"], "open_orders");
    // The order turns up on the stream and is cancelled: it no longer counts.
    r.fake.appear(&format!(
        r#"{{"client_order_id":"wse-abs","symbol":"{CALL}","qty":"1","side":"buy","limit_price":"1.25","position_intent":"buy_to_open"}}"#
    ));
    r.fake.event("wse-abs", "canceled", "canceled");
    c.order_event("wse-abs", "canceled");
    let (q, _) = c.req("open_orders", "q1", "");
    assert_eq!(q["unresolved"], 0);
    assert_eq!(c.submit("r3", &buy("wse-next"))["outcome"], "accepted");
}

/// Polls `open_orders` until nothing is unresolved, with no stream event sent.
fn wait_resolved(c: &mut Conn) -> Value {
    let t = Instant::now();
    let mut n = 0;
    loop {
        n += 1;
        let (q, _) = c.req("open_orders", &format!("w{n}"), "");
        if q["unresolved"] == 0 {
            return q;
        }
        assert!(
            t.elapsed() < Duration::from_secs(5),
            "still unresolved: {q}"
        );
        std::thread::sleep(Duration::from_millis(100));
    }
}

#[test]
fn a_periodic_check_settles_an_absent_order_the_broker_now_shows() {
    let r = rig_with("alpaca-absent-periodic", |p| p.max_open = 1);
    let mut c = ready(&r.gw, "reactor");
    absent_submit(&r, &mut c, "wse-pc");
    r.fake.appear(&format!(
        r#"{{"client_order_id":"wse-pc","symbol":"{CALL}","qty":"1","side":"buy","limit_price":"1.25","position_intent":"buy_to_open"}}"#
    ));
    // No stream event: the poll's lookup finds it. It is working, so it still counts.
    wait_resolved(&mut c);
    assert_eq!(c.submit("r2", &buy("wse-next"))["detail"]["cap"], "open_orders");
}

#[test]
fn a_404_never_clears_an_absent_order_however_many_checks_return_it() {
    let r = rig_with("alpaca-absent-404", |p| p.max_open = 1);
    let mut c = ready(&r.gw, "reactor");
    absent_submit(&r, &mut c, "wse-404");
    // Let the poll's lookup run several times, each one a 404 from the broker.
    let before = r.fake.count("GET", "/v2/orders:by_client_order_id");
    let t = Instant::now();
    while r.fake.count("GET", "/v2/orders:by_client_order_id") < before + 5 {
        assert!(
            t.elapsed() < Duration::from_secs(5),
            "the poll never looked again"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    // Still counted: it uses the open slot, and a close on its leg waits for it.
    let (q, _) = c.req("open_orders", "q1", "");
    assert_eq!(q["unresolved"], 1);
    let v = c.submit("r2", &buy("wse-next"));
    assert_eq!(v["detail"]["cap"], "open_orders");
    let v = c.submit(
        "r3",
        &single("wse-404-close", CALL, "sell", "close", 1, "1.00"),
    );
    assert_eq!(v["reason"], "leg_pending");
}

#[test]
fn a_close_waits_for_pending_orders_on_its_leg_and_opens_do_not() {
    let r = rig("alpaca-leg-guard");
    let mut c = ready(&r.gw, "reactor");
    absent_submit(&r, &mut c, "wse-p1");
    // The close is on the pending order's leg: refused whole, and not sent.
    let v = c.submit("r2", &single("wse-p2", CALL, "sell", "close", 1, "1.00"));
    assert_eq!(v["reason"], "leg_pending");
    assert_eq!(v["detail"]["symbols"][0], CALL);
    // A close on a leg with no pending order is not held by the guard.
    let v = c.submit("r3", &single("wse-p3", PUT_A, "sell", "close", 1, "1.00"));
    assert_eq!(v["outcome"], "accepted");
    // An open on the pending leg is not held by the guard either.
    assert_eq!(c.submit("r4", &buy("wse-p4"))["outcome"], "accepted");
    assert_eq!(posts(&r.fake).len(), 3, "the refused close was never sent");
}

#[test]
fn a_close_on_a_clear_leg_passes_the_day_cap_but_not_a_working_order_on_its_leg() {
    let r = rig_with("alpaca-leg-day", |p| p.submits_per_day = 1);
    let mut c = ready(&r.gw, "reactor");
    assert_eq!(c.submit("d1", &buy("wse-d1"))["outcome"], "accepted");
    // The day's one open is used up, but PUT_A has no pending order.
    let v = c.submit("d2", &single("wse-d2", PUT_A, "sell", "close", 1, "1.00"));
    assert_eq!(v["outcome"], "accepted");
    // The open on CALL is still working, so a close on CALL waits for it.
    let v = c.submit("d3", &single("wse-d3", CALL, "sell", "close", 1, "1.00"));
    assert_eq!(v["reason"], "leg_pending");
}

#[test]
fn rate_limits_back_off_and_never_resend() {
    let r = rig("alpaca-429");
    let mut c = ready(&r.gw, "reactor");
    let reset = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
        + 1;
    let headers = vec![
        ("Retry-After".into(), "1".into()),
        ("X-RateLimit-Limit".into(), "200".into()),
        ("X-RateLimit-Remaining".into(), "0".into()),
        ("X-RateLimit-Reset".into(), reset.to_string()),
    ];
    let body = include_str!("fixtures/alpaca/error_rate_limit.json").to_owned();
    r.fake.next(
        "GET",
        "/v2/positions",
        Act::Respond(429, headers.clone(), body.clone()),
    );
    let (v, _) = c.req("positions", "q1", "");
    assert_eq!(
        (v["code"].as_str(), v["detail"]["http_status"].as_u64()),
        (Some("broker_error"), Some(429))
    );
    let (v, _) = c.req("positions", "q2", "");
    assert_eq!(v["code"], "throttled");
    std::thread::sleep(Duration::from_millis(2100));
    r.fake
        .next("POST", "/v2/orders", Act::Respond(429, headers, body));
    let v = c.submit("r1", &buy("wse-r1"));
    assert_eq!(
        (v["outcome"].as_str(), v["reason"].as_str()),
        (Some("unknown"), Some("http_429_after_send"))
    );
    let v = c.submit("r2", &buy("wse-r2"));
    assert_eq!(
        (v["outcome"].as_str(), v["reason"].as_str()),
        (Some("not_sent"), Some("throttled"))
    );
    assert_eq!(posts(&r.fake).len(), 1);
}

#[test]
fn stream_loss_degrades_refuses_and_resyncs_on_reconnect() {
    let r = rig("alpaca-stream");
    let mut c = ready(&r.gw, "reactor");
    assert_eq!(c.submit("r1", &buy("wse-s"))["outcome"], "accepted");
    {
        let mut st = r.fake.lock();
        st.auth_ok = false;
        st.ws_drop = true;
    }
    let v = c.event("status", |v| v["state"] == "degraded");
    assert!(v["broker_stream"]["down"]["since"].is_u64(), "{v}");
    let v = c.submit("r2", &buy("wse-s2"));
    assert_eq!(
        (v["outcome"].as_str(), v["reason"].as_str()),
        (Some("not_sent"), Some("not_ready"))
    );
    let refused = r.fake.lock().ws_auth.len();
    let until = Instant::now() + Duration::from_secs(5);
    while r.fake.lock().ws_auth.len() < refused + 2 {
        assert!(Instant::now() < until, "no reconnect attempts");
        std::thread::sleep(Duration::from_millis(10));
    }
    {
        // Filled while the stream was down: only REST resync can report it.
        let mut st = r.fake.lock();
        let o = st.orders.iter_mut().find(|o| o.cid == "wse-s").unwrap();
        (o.filled, o.avg, o.status) = (1, Some("1.20".into()), "filled".into());
        st.auth_ok = true;
    }
    let v = c.order_event("wse-s", "fill");
    assert_eq!(
        (v["source"].as_str(), v["filled_qty"].as_u64()),
        (Some("rest_reconcile"), Some(1))
    );
    assert!(v.get("fill").is_none(), "no fabricated execution");
    let v = c.event("status", |v| v.get("resync").is_some());
    assert_eq!(
        (v["state"].as_str(), v["broker_stream"].as_str()),
        (Some("ready"), Some("up"))
    );
    assert_eq!(c.submit("r3", &buy("wse-s3"))["outcome"], "accepted");
}

#[test]
fn queries_cancel_and_kill_pass_through_without_credentials_in_output() {
    let r = rig("alpaca-queries");
    let mut c = ready(&r.gw, "reactor");
    let mut ops = ready(&r.gw, "ops");
    let (v, _) = c.req("account", "q1", "");
    assert_eq!(v["account"]["options_trading_level"], 3);
    assert_eq!(v["broker_raw"]["account_number"], "PA0000000000");
    assert!(v["ts_gateway"].is_u64());
    let (v, _) = c.req("positions", "q2", "");
    assert_eq!(v["positions"][0]["qty"], -2);
    let (v, _) = c.req("clock", "q3", "");
    assert_eq!(v["clock"]["ts"], support::T0);
    assert_eq!(v["clock"]["is_open"], true);
    let (v, _) = c.req(
        "calendar",
        "q4",
        r#""start":"2026-10-05","end":"2026-10-05""#,
    );
    assert_eq!(v["days"][0]["close"], "16:00");
    let (v, _) = c.req("activities", "q5", r#""types":["FILL","OPEXP"]"#);
    assert_eq!(v["items"].as_array().unwrap().len(), 2);
    assert_eq!(v["items"][1]["activity_type"], "OPEXP");
    assert_eq!(v["next_page_token"], Value::Null);
    assert!(
        r.fake
            .lock()
            .log
            .iter()
            .any(|(_, p, _)| p.contains("activity_types=FILL%2COPEXP"))
    );

    assert_eq!(c.submit("r1", &buy("wse-q1"))["outcome"], "accepted");
    let (v, _) = c.req("order", "q6", r#""client_order_id":"wse-q1""#);
    let bid = v["order"]["broker_order_id"].as_str().unwrap().to_owned();
    assert!(v["order"]["intent_sha256"].is_string());
    let (v, _) = c.req("order", "q7", &format!(r#""broker_order_id":"{bid}""#));
    assert_eq!(v["order"]["client_order_id"], "wse-q1");
    let (v, _) = c.req("order", "q8", r#""client_order_id":"wse-none""#);
    assert_eq!(v["order"], Value::Null);
    let (v, _) = c.req("open_orders", "q9", "");
    assert_eq!(v["orders"][0]["client_order_id"], "wse-q1");
    let (v, _) = c.req("cancel", "c1", r#""client_order_id":"wse-q1""#);
    assert_eq!(v["outcome"], "accepted", "{v}");
    assert!(r.fake.count("DELETE", &format!("/v2/orders/{bid}")) == 1);
    r.fake.event("wse-q1", "canceled", "canceled");
    c.order_event("wse-q1", "canceled");
    let (v, _) = c.req("cancel", "c2", r#""client_order_id":"wse-q1""#);
    assert_eq!(
        (v["outcome"].as_str(), v["http_status"].as_u64()),
        (Some("rejected"), Some(422))
    );

    assert_eq!(c.submit("r2", &buy("wse-q2"))["outcome"], "accepted");
    let (v, _) = ops.req("kill", "k1", r#""level":"halt_all","reason":"test""#);
    assert_eq!(v["state"], "killed");
    assert_eq!(r.fake.count("DELETE", "/v2/orders"), 3);
    assert_eq!(r.fake.lock().orders[1].status, "canceled");

    assert!(r.fake.lock().keys_seen.iter().all(|k| k == KEY));
    let mut text = journal(&r.state).join("\n");
    for conn in [&c, &ops] {
        text += &conn.lines.join("\n");
    }
    assert!(!text.contains(KEY) && !text.contains(SECRET));
}

#[test]
fn cli_refuses_unsafe_or_incomplete_credentials_without_printing_values() {
    use std::os::unix::fs::PermissionsExt;
    let dir = root("alpaca-cli");
    let run = |extra: &[&str]| {
        let state = dir.join("state");
        let mut args = vec!["--account", "a", "--listen", "127.0.0.1:0"];
        args.extend([
            "--state",
            state.to_str().unwrap(),
            "--broker",
            "alpaca-paper",
        ]);
        args.extend(extra);
        let out = std::process::Command::new(env!("CARGO_BIN_EXE_order-gateway"))
            .args(&args)
            .output()
            .unwrap();
        (
            out.status.code(),
            String::from_utf8_lossy(&out.stderr).into_owned(),
        )
    };
    let (code, err) = run(&[]);
    assert_eq!(code, Some(2), "{err}");
    assert!(err.contains("--credentials-file"));
    let file = dir.join("alpaca-paper-9.env");
    std::fs::write(
        &file,
        format!("ALPACA_API_KEY_ID={KEY}\nALPACA_API_SECRET_KEY={SECRET}\n"),
    )
    .unwrap();
    std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o644)).unwrap();
    let path = file.to_str().unwrap();
    let (code, err) = run(&["--credentials-file", path]);
    assert_eq!(code, Some(2));
    assert!(err.contains("mode-600") && !err.contains(SECRET), "{err}");
    std::fs::write(&file, format!("ALPACA_API_KEY_ID={KEY}\n")).unwrap();
    std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600)).unwrap();
    let (code, err) = run(&["--credentials-file", path]);
    assert_eq!(code, Some(2));
    assert!(
        err.contains("APCA_API_SECRET_KEY") && !err.contains(KEY),
        "{err}"
    );
    let (code, _) = run(&["--credentials-file", path, "--sim-script", path]);
    assert_eq!(code, Some(2));
}

#[test]
fn flatten_sends_alpacas_liquidate_all_once() {
    let r = rig("alpaca-flatten");
    let mut c = ready(&r.gw, "reactor");
    let mut ops = ready(&r.gw, "ops");
    assert_eq!(
        c.req("flatten", "f0", r#""reason":"test""#).0["code"],
        "forbidden"
    );
    let (v, _) = ops.req("flatten", "f1", r#""reason":"test""#);
    assert_eq!(
        (v["outcome"].as_str(), v["state"].as_str()),
        (Some("ok"), Some("killed"))
    );
    assert_eq!(v["positions"][0]["symbol"], "SPY261016P00560000");
    assert_eq!(v["positions"][0]["status"], 200);
    assert_eq!(v["positions"][0]["broker_raw"]["type"], "market");
    let liquidations: Vec<String> = r
        .fake
        .lock()
        .log
        .iter()
        .filter(|(m, p, _)| m == "DELETE" && p.starts_with("/v2/positions"))
        .map(|(_, p, _)| p.clone())
        .collect();
    assert_eq!(liquidations, ["/v2/positions?cancel_orders=true"]);
    // Opens are refused afterwards.
    assert_eq!(c.submit("r1", &support::buy("wse-1"))["reason"], "killed");
}

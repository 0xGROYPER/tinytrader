//! A simulator fed in process and clocked by a session (decision 0022): records pushed
//! through a `Feed`, symbols from symbol mappings, `cbbo-1m` and `bbo-1m` books, and a
//! `Clock` that advances only once the client's order messages have all been handled.
mod support;
use dbn::{
    BboMsg, CbboMsg, Cmbp1Msg, RecordHeader, RecordRef, SType, Schema, SymbolMappingMsg, rtype,
};
use order_gateway::{Clock, Feed, Gateway, SimBroker, start};
use serde_json::Value;
use std::time::Duration;
use support::*;

const S: u64 = 1_000_000_000;
const WAIT: Duration = Duration::from_secs(5);

fn px(cents: i64) -> i64 {
    cents * 10_000_000
}

/// The options by OPRA raw symbol (root padded to six), and SPY's shares, as a batch
/// file's symbol mappings give them.
fn mappings(feed: &Feed) {
    let opra = |s: &str| format!("{:<6}{}", &s[..3], &s[3..]);
    for (iid, raw) in [
        (1, opra(CALL)),
        (2, opra(PUT_A)),
        (3, opra(PUT_B)),
        (9, "SPY".into()),
    ] {
        let m = SymbolMappingMsg::new(
            iid,
            T0 - S,
            SType::Parent,
            "SPY.OPT",
            SType::RawSymbol,
            &raw,
            0,
            0,
        )
        .unwrap();
        feed.push(T0 - S, RecordRef::from(&m)).unwrap();
    }
}

/// A one-minute consolidated quote, stamped at the interval's end.
fn cbbo(feed: &Feed, iid: u32, ts: u64, bid: i64, ask: i64) {
    let mut q = CbboMsg {
        hd: RecordHeader::new::<CbboMsg>(rtype::CBBO_1M, 30, iid, u64::MAX),
        ts_recv: ts,
        ..CbboMsg::default_for_schema(Schema::Cbbo1M)
    };
    q.levels[0].bid_px = px(bid);
    q.levels[0].ask_px = px(ask);
    q.levels[0].bid_sz = 10;
    q.levels[0].ask_sz = 10;
    feed.push(ts, RecordRef::from(&q)).unwrap();
}

fn bbo(feed: &Feed, iid: u32, ts: u64, bid: i64, ask: i64) {
    let mut q = BboMsg {
        hd: RecordHeader::new::<BboMsg>(rtype::BBO_1M, 2, iid, ts),
        ts_recv: ts,
        ..BboMsg::default_for_schema(Schema::Bbo1M)
    };
    q.levels[0].bid_px = px(bid);
    q.levels[0].ask_px = px(ask);
    q.levels[0].bid_sz = 300;
    q.levels[0].ask_sz = 300;
    feed.push(ts, RecordRef::from(&q)).unwrap();
}

fn gateway(name: &str, script: &str) -> (Gateway, Feed, Clock) {
    let (sim, feed) = SimBroker::fed(script, r#"{"source":"session","run":"test"}"#).unwrap();
    let mut cfg = config(root(name));
    cfg.profile.equities = vec!["SPY".into()];
    let gw = start(cfg, sim).unwrap();
    let clock = gw.clock();
    (gw, feed, clock)
}

fn stock(cid: &str, side: &str, qty: u32, price: &str) -> String {
    format!(
        r#"{{"client_order_id":"{cid}","shape":"single","instrument":{{"class":"equity","symbol":"SPY"}},"side":"{side}","effect":"open","qty":{qty},"type":"limit","limit_price":"{price}","time_in_force":"day"}}"#
    )
}

fn fill(c: &mut Conn, cid: &str) -> Value {
    c.until(|v| v["kind"] == "order" && v["client_order_id"] == cid && v["event"] == "fill")
        .pop()
        .unwrap()
}

/// An options combo and a share order fill on the minute after they're sent, each at
/// its touch, from `cbbo-1m` and `bbo-1m` books keyed by symbol mappings.
#[test]
fn options_and_shares_fill_from_minute_quotes() {
    let (gw, feed, clock) = gateway("fed-fill", "{}");
    let mut c = Conn::open(&gw, "reactor", None);
    let welcome = serde_json::from_str::<Value>(&c.lines[0]).unwrap();
    let pins = &welcome["pins"]["broker"];
    assert_eq!(pins["model"], "tinytrader-sim/2");
    assert_eq!(pins["quotes"]["source"], "session");
    mappings(&feed);
    // At first neither fills: the spread's natural is 0.92 - 0.53 = 0.39 credit, short
    // of the 0.40 asked, and the shares' ask is above the limit.
    cbbo(&feed, 2, T0, 92, 94);
    cbbo(&feed, 3, T0, 51, 53);
    bbo(&feed, 9, T0, 58_011, 58_012);
    assert_eq!(clock.advance("reactor", 0, T0, WAIT).unwrap(), T0);
    assert_eq!(
        c.submit("r1", &credit_spread("wse-cs", 2))["outcome"],
        "accepted"
    );
    assert_eq!(
        c.submit("r2", &stock("wse-sh", "buy", 4, "580.10"))["outcome"],
        "accepted"
    );
    // The next minute the shares fill at the new ask; the spread is unchanged. A minute
    // later its natural is 0.94 - 0.52 = 0.42 and it fills there: both legs' quotes of
    // that instant apply before it is evaluated, never 0.94 against the old 0.53.
    cbbo(&feed, 2, T0 + 60 * S, 92, 94);
    cbbo(&feed, 3, T0 + 60 * S, 51, 53);
    bbo(&feed, 9, T0 + 60 * S, 58_006, 58_007);
    clock.advance("reactor", 2, T0 + 60 * S, WAIT).unwrap();
    let f = fill(&mut c, "wse-sh");
    assert_eq!(f["fill"]["price"], "580.07");
    assert_eq!(f["ts_broker"], T0 + 60 * S);
    cbbo(&feed, 2, T0 + 120 * S, 94, 96);
    cbbo(&feed, 3, T0 + 120 * S, 50, 52);
    clock.advance("reactor", 2, T0 + 120 * S, WAIT).unwrap();
    let f = fill(&mut c, "wse-cs");
    assert_eq!(
        (f["fill"]["price"].as_str(), f["filled_qty"].as_u64()),
        (Some("-0.42"), Some(2))
    );
    assert_eq!(f["ts_broker"], T0 + 120 * S);
    gw.stop().unwrap();
}

/// An order sent just before the clock moves is always handled first: the clock waits
/// for the requests the session says were sent, however the threads interleave.
#[test]
fn the_clock_waits_for_the_requests_sent() {
    let mut seen = Vec::new();
    for round in 0..10 {
        let (gw, feed, clock) = gateway(&format!("fed-race-{round}"), "{}");
        let mut c = Conn::open(&gw, "reactor", None);
        mappings(&feed);
        cbbo(&feed, 1, T0, 120, 130);
        clock.advance("reactor", 0, T0, WAIT).unwrap();
        // Sent, not awaited: the clock may well reach the gateway first.
        let body = format!(r#"{{"request_id":"r1","intent":{}}}"#, buy("wse-race"));
        c.c.send("submit", &body).unwrap();
        cbbo(&feed, 1, T0 + 60 * S, 121, 124);
        clock.advance("reactor", 1, T0 + 60 * S, WAIT).unwrap();
        let f = fill(&mut c, "wse-race");
        seen.push((f["fill"]["price"].clone(), f["ts_broker"].clone()));
        gw.stop().unwrap();
    }
    assert!(seen.iter().all(|s| *s == seen[0]), "{seen:?}");
    assert_eq!(seen[0].0, "1.24");
}

#[test]
fn a_clock_waiting_for_requests_never_sent_says_so() {
    let (gw, feed, clock) = gateway("fed-wait", "{}");
    let _c = Conn::open(&gw, "reactor", None);
    mappings(&feed);
    let e = clock
        .advance("reactor", 3, T0, Duration::from_millis(200))
        .unwrap_err();
    assert!(e.contains("waited for 3 order messages, 0 arrived"), "{e}");
    let e = clock.advance("nobody", 0, T0, WAIT).unwrap_err();
    assert!(e.contains("no client nobody"), "{e}");
    gw.stop().unwrap();
}

/// The strategy's declared latency delays activation: a quote good for one second is
/// missed by a strategy two seconds slow.
#[test]
fn strategy_latency_delays_activation() {
    let run = |name: &str, script: &str| {
        let (gw, feed, clock) = gateway(name, script);
        let mut c = Conn::open(&gw, "reactor", None);
        mappings(&feed);
        cbbo(&feed, 1, T0, 120, 130);
        clock.advance("reactor", 0, T0, WAIT).unwrap();
        c.submit("r1", &buy("wse-lat"));
        cbbo(&feed, 1, T0 + S, 120, 124);
        cbbo(&feed, 1, T0 + 2 * S, 120, 130);
        clock.advance("reactor", 1, T0 + 10 * S, WAIT).unwrap();
        let (r, _) = c.req("order", "r3", r#""client_order_id":"wse-lat""#);
        gw.stop().unwrap();
        r["order"]["status"].as_str().unwrap().to_owned()
    };
    assert_eq!(run("fed-lat-0", "{}"), "filled");
    assert_ne!(
        run("fed-lat-2s", r#"{"strategy_latency_ms":2000}"#),
        "filled"
    );
}

/// A wait that times out is dropped: it never holds up the advances behind it, and
/// never runs late when the order it waited for arrives after all.
#[test]
fn a_timed_out_clock_never_wedges_or_runs_late() {
    let (gw, feed, clock) = gateway("fed-wedge", "{}");
    let mut c = Conn::open(&gw, "reactor", None);
    mappings(&feed);
    cbbo(&feed, 1, T0, 120, 130);
    clock.advance("reactor", 0, T0, WAIT).unwrap();
    let late = T0 + 60 * S;
    cbbo(&feed, 1, late, 121, 124);
    assert!(
        clock
            .advance("reactor", 1, late, Duration::from_millis(100))
            .is_err()
    );
    // Not wedged: an advance that needs nothing more runs at once.
    assert_eq!(clock.advance("reactor", 0, T0 + S, WAIT).unwrap(), T0 + S);
    // The order the dropped wait wanted arrives now; the dropped advance to `late`
    // doesn't run, so the order rests on the book of T0 + 1 s.
    c.submit("r1", &buy("wse-late"));
    std::thread::sleep(Duration::from_millis(100));
    let (r, _) = c.req("order", "r2", r#""client_order_id":"wse-late""#);
    assert_ne!(r["order"]["status"], "filled", "{r}");
    gw.stop().unwrap();
}

/// Only order messages count: queries sent before an order don't release the clock
/// before the order is handled.
#[test]
fn queries_never_count_as_order_messages() {
    let (gw, feed, clock) = gateway("fed-queries", "{}");
    let mut c = Conn::open(&gw, "reactor", None);
    mappings(&feed);
    cbbo(&feed, 1, T0, 120, 130);
    clock.advance("reactor", 0, T0, WAIT).unwrap();
    c.c.send("open_orders", r#"{"request_id":"q1"}"#).unwrap();
    c.c.send("positions", r#"{"request_id":"q2"}"#).unwrap();
    let body = format!(r#"{{"request_id":"r1","intent":{}}}"#, buy("wse-q"));
    c.c.send("submit", &body).unwrap();
    cbbo(&feed, 1, T0 + 60 * S, 121, 124);
    clock.advance("reactor", 1, T0 + 60 * S, WAIT).unwrap();
    assert_eq!(fill(&mut c, "wse-q")["ts_broker"], T0 + 60 * S);
    gw.stop().unwrap();
}

#[test]
fn a_client_cannot_move_a_session_clock() {
    let (gw, feed, clock) = gateway("fed-own-advance", "{}");
    let mut c = Conn::open(&gw, "reactor", None);
    mappings(&feed);
    clock.advance("reactor", 0, T0, WAIT).unwrap();
    let (r, _) = c.req("advance", "a1", &format!(r#""to_ts":{}"#, T0 + S));
    assert_eq!(r["code"], "clocked_by_session", "{r}");
    gw.stop().unwrap();
}

#[test]
fn a_record_going_back_is_refused() {
    let (gw, feed, _clock) = gateway("fed-back", "{}");
    cbbo(&feed, 1, T0 + S, 120, 130);
    let q = CbboMsg {
        hd: RecordHeader::new::<CbboMsg>(rtype::CBBO_1M, 30, 1, u64::MAX),
        ts_recv: T0,
        ..CbboMsg::default_for_schema(Schema::Cbbo1M)
    };
    let e = feed.push(T0, RecordRef::from(&q)).unwrap_err();
    assert!(e.contains("after one at"), "{e}");
    gw.stop().unwrap();
}

/// Three updates in one instant, two of them for one contract: the order sees only
/// the instant's last quote of each, never the replaced one.
#[test]
fn an_instant_is_one_book_with_the_last_quote_of_each() {
    let (gw, feed, clock) = gateway("fed-instant", "{}");
    let mut c = Conn::open(&gw, "reactor", None);
    mappings(&feed);
    cbbo(&feed, 2, T0, 92, 94);
    cbbo(&feed, 3, T0, 51, 53);
    clock.advance("reactor", 0, T0, WAIT).unwrap();
    c.submit("r1", &credit_spread("wse-cs", 1));
    // A good enough with the old B, then B, then A replaced by a worse quote.
    cbbo(&feed, 2, T0 + 60 * S, 99, 101);
    cbbo(&feed, 3, T0 + 60 * S, 51, 53);
    cbbo(&feed, 2, T0 + 60 * S, 92, 94);
    clock.advance("reactor", 1, T0 + 60 * S, WAIT).unwrap();
    let (r, _) = c.req("order", "r2", r#""client_order_id":"wse-cs""#);
    assert_ne!(r["order"]["status"], "filled", "{r}");
    gw.stop().unwrap();
}

/// A tcbbo quote (the one before a trade) doesn't replace a book that quote records
/// give; alone, it is the book.
#[test]
fn tcbbo_is_a_book_only_where_nothing_else_is() {
    let tcbbo = |feed: &Feed, iid: u32, ts: u64, bid: i64, ask: i64| {
        let mut q = Cmbp1Msg {
            hd: RecordHeader::new::<Cmbp1Msg>(rtype::TCBBO, 30, iid, ts),
            ts_recv: ts,
            ..Cmbp1Msg::default_for_schema(Schema::Tcbbo)
        };
        q.levels[0].bid_px = px(bid);
        q.levels[0].ask_px = px(ask);
        q.levels[0].bid_sz = 10;
        q.levels[0].ask_sz = 10;
        feed.push(ts, RecordRef::from(&q)).unwrap();
    };
    let (gw, feed, clock) = gateway("fed-tcbbo", "{}");
    let mut c = Conn::open(&gw, "reactor", None);
    mappings(&feed);
    cbbo(&feed, 1, T0, 120, 130);
    tcbbo(&feed, 2, T0, 50, 52);
    clock.advance("reactor", 0, T0, WAIT).unwrap();
    c.submit("r1", &buy("wse-q"));
    c.submit("r2", &single("wse-t", PUT_A, "buy", "open", 1, "0.51"));
    tcbbo(&feed, 1, T0 + S, 119, 121);
    tcbbo(&feed, 2, T0 + S, 49, 51);
    clock.advance("reactor", 2, T0 + 2 * S, WAIT).unwrap();
    assert_eq!(fill(&mut c, "wse-t")["fill"]["price"], "0.51");
    let (r, _) = c.req("order", "r3", r#""client_order_id":"wse-q""#);
    assert_ne!(r["order"]["status"], "filled", "{r}");
    gw.stop().unwrap();
}

//! Pure Alpaca trading DTOs: endpoints, credentials, bodies and parsing.
use order_gateway::trading::{
    Credentials, Endpoints, LegSpec, StreamMessage, compact, elements, member, order_body,
    parse_account, parse_clock, parse_error, parse_order, parse_positions, parse_stream,
    parse_time,
};

#[test]
fn only_paper_or_loopback_endpoints_exist() {
    let p = Endpoints::paper();
    assert_eq!(p.rest(), "https://paper-api.alpaca.markets");
    assert_eq!(p.stream(), "wss://paper-api.alpaca.markets/stream");
    assert_eq!(p.environment(), "paper");
    assert!(Endpoints::loopback("10.0.0.1:80".parse().unwrap()).is_none());
    let l = Endpoints::loopback("127.0.0.1:9".parse().unwrap()).unwrap();
    assert_eq!(l.stream(), "ws://127.0.0.1:9/stream");
    // No live host is compiled into the trading module or the gateway driver.
    let dir = env!("CARGO_MANIFEST_DIR");
    for name in ["trading.rs", "mod.rs", "stream.rs", "http.rs"] {
        let src = format!("{dir}/src/alpaca/{name}");
        let text = std::fs::read_to_string(&src).unwrap();
        let live = text.replace("paper-api.alpaca.markets", "");
        assert!(!live.contains("api.alpaca.markets"), "{src}");
    }
}

#[test]
fn credentials_parse_either_prefix_and_never_show() {
    let c = Credentials::from_env_text(
        "# paper\nexport APCA_API_KEY_ID='PKX'\nAPCA_API_SECRET_KEY=s3\n",
    )
    .unwrap();
    assert_eq!(format!("{c:?}"), "Credentials(<redacted>)");
    assert_eq!(
        c.headers(),
        [("APCA-API-KEY-ID", "PKX"), ("APCA-API-SECRET-KEY", "s3")]
    );
    assert_eq!(
        c.stream_auth(),
        r#"{"action":"auth","key":"PKX","secret":"s3"}"#
    );
    let c = Credentials::from_env_text("ALPACA_API_KEY_ID=K\nALPACA_API_SECRET_KEY=S").unwrap();
    assert_eq!(c.headers()[1].1, "S");
    let e = Credentials::from_env_text("APCA_API_KEY_ID=hidden-key\n").unwrap_err();
    assert!(
        e.contains("APCA_API_SECRET_KEY") && !e.contains("hidden"),
        "{e}"
    );
    let e = Credentials::from_env_text("APCA_API_KEY_ID=a b\nAPCA_API_SECRET_KEY=x").unwrap_err();
    assert!(!e.contains("a b"));
}

#[test]
fn bodies_follow_reactor_field_order() {
    let leg = |symbol, buy, open| LegSpec {
        symbol,
        ratio: 1,
        buy,
        open,
        equity: false,
    };
    let single = order_body(
        "wse-1",
        false,
        &[leg("SPY261005C00580000", false, false)],
        2,
        "1.25",
        false,
    );
    assert_eq!(
        single,
        r#"{"symbol":"SPY261005C00580000","qty":"2","side":"sell","type":"limit","limit_price":"1.25","time_in_force":"day","client_order_id":"wse-1","position_intent":"sell_to_close"}"#
    );
    let legs = [
        LegSpec {
            ratio: 2,
            ..leg("SPY261005C00580000", true, true)
        },
        leg("SPY261005C00590000", false, true),
    ];
    assert_eq!(
        order_body("wse-2", true, &legs, 1, "-0.40", false),
        r#"{"order_class":"mleg","qty":"1","type":"limit","time_in_force":"day","limit_price":"-0.40","client_order_id":"wse-2","legs":[{"symbol":"SPY261005C00580000","ratio_qty":"2","side":"buy","position_intent":"buy_to_open"},{"symbol":"SPY261005C00590000","ratio_qty":"1","side":"sell","position_intent":"sell_to_open"}]}"#
    );
}

#[test]
fn stock_orders_have_no_position_intent_and_may_be_on_close() {
    let aapl = |buy, open| LegSpec {
        symbol: "AAPL",
        ratio: 1,
        buy,
        open,
        equity: true,
    };
    assert_eq!(
        order_body("pead-1", false, &[aapl(true, true)], 9, "222.50", true),
        r#"{"symbol":"AAPL","qty":"9","side":"buy","type":"limit","limit_price":"222.50","time_in_force":"cls","client_order_id":"pead-1"}"#
    );
    assert_eq!(
        order_body("pead-2", false, &[aapl(false, true)], 9, "210.00", false),
        r#"{"symbol":"AAPL","qty":"9","side":"sell","type":"limit","limit_price":"210.00","time_in_force":"day","client_order_id":"pead-2"}"#
    );
}

#[test]
fn raw_scanning_keeps_order_and_lexemes() {
    let raw = compact("[ {\"b\": 1.50, \"a\": \"x, \\\"y]\"},\n {\"c\": [1, {\"d\": null}]} ]");
    assert_eq!(raw, r#"[{"b":1.50,"a":"x, \"y]"},{"c":[1,{"d":null}]}]"#);
    let items = elements(&raw).unwrap();
    assert_eq!(
        items,
        [r#"{"b":1.50,"a":"x, \"y]"}"#, r#"{"c":[1,{"d":null}]}"#]
    );
    assert_eq!(member(items[0], "a"), Some(r#""x, \"y]""#));
    assert_eq!(member(items[1], "c"), Some(r#"[1,{"d":null}]"#));
    assert_eq!(member(items[1], "z"), None);
    assert_eq!(elements("[]").unwrap().len(), 0);
    assert!(elements("[1}").is_none());
}

#[test]
fn times_with_offsets() {
    let utc = parse_time("2026-10-05T14:00:00Z").unwrap();
    assert_eq!(parse_time("2026-10-05T10:00:00-04:00"), Some(utc));
    assert_eq!(
        parse_time("2026-10-05T15:00:00.5+01:00"),
        Some(utc + 500_000_000)
    );
    assert_eq!(parse_time("2026-10-05"), None);
}

/// The Alpaca docs' multi-leg fill example, trimmed to the fields that matter.
const MLEG_FILL: &str = r#"{"stream":"trade_updates","data":{"at":"2025-01-21T07:32:40.70095Z",
 "event_id":"01JJ3WE73W5PG672TC4XACXH5R","event":"fill","timestamp":"2025-01-21T07:32:40.695569506Z",
 "order":{"id":"31cd620f-3bd5-41b7-8bb2-6834524679d0","client_order_id":"fe999618","updated_at":"2025-01-21T07:32:40.699359002Z",
  "qty":"1","filled_qty":"1","filled_avg_price":"1.62","order_class":"mleg","status":"filled","legs":[
  {"id":"3cbe69ef","symbol":"AAPL250321P00200000","qty":"1","filled_qty":"1","filled_avg_price":"1.6","ratio_qty":"1"},
  {"id":"ec694de5","symbol":"AAPL250321C00380000","qty":"1","filled_qty":"1","filled_avg_price":"0.02","ratio_qty":"1"}]},
 "price":"1.62","qty":"1","position_qtys":{"AAPL250321P00200000":"1","AAPL250321C00380000":"1"},
 "legs":[{"execution_id":"69a70e98","qty":"1","price":"1.6","order_id":"3cbe69ef","symbol":"AAPL250321P00200000","timestamp":"2025-01-21T07:32:40.695564076Z"},
  {"execution_id":"fb878d87","qty":"1","price":"0.02","order_id":"ec694de5","symbol":"AAPL250321C00380000","timestamp":"2025-01-21T07:32:40.695569506Z"}]}}"#;

#[test]
fn stream_messages() {
    let StreamMessage::Update(u) = parse_stream(MLEG_FILL) else {
        panic!("update");
    };
    assert_eq!(
        (u.event.as_str(), u.qty, u.price.as_deref()),
        ("fill", Some(1), Some("1.62"))
    );
    assert_eq!(u.execution_id, None);
    assert_eq!(u.timestamp_ns, parse_time("2025-01-21T07:32:40.695569506Z"));
    assert!(u.order.mleg && u.order.legs[1].filled_avg_price.as_deref() == Some("0.02"));
    assert_eq!(u.legs[0].execution_id.as_deref(), Some("69a70e98"));
    assert!(
        u.raw
            .starts_with(r#"{"at":"2025-01-21T07:32:40.70095Z","event_id""#)
    );
    assert_eq!(member(&u.raw, "order"), Some(u.order.raw.as_str()));
    let auth = |s| {
        format!(r#"{{"stream":"authorization","data":{{"status":"{s}","action":"authenticate"}}}}"#)
    };
    assert_eq!(parse_stream(&auth("authorized")), StreamMessage::Authorized);
    assert_eq!(
        parse_stream(&auth("unauthorized")),
        StreamMessage::Unauthorized
    );
    let listening = r#"{"stream":"listening","data":{"streams":["trade_updates"]}}"#;
    assert_eq!(parse_stream(listening), StreamMessage::Listening);
    let error = r#"{"action":"error","data":{"error_message":"internal server error"}}"#;
    assert_eq!(
        parse_stream(error),
        StreamMessage::Error("internal server error".into())
    );
    assert_eq!(parse_stream("not json"), StreamMessage::Other);
}

#[test]
fn rest_bodies() {
    let o = parse_order(r#"{"id":"b1","client_order_id":"wse-1","status":"new","filled_qty":"0","filled_avg_price":null,"order_class":"simple","legs":null,"created_at":"2026-10-05T10:00:00-04:00"}"#).unwrap();
    assert_eq!(
        (o.filled_qty, o.mleg, o.updated_ns),
        (0, false, parse_time("2026-10-05T14:00:00Z").unwrap())
    );
    assert!(parse_order(r#"{"id":"b1"}"#).is_none());
    let a = parse_account(r#"{"status":"ACTIVE","cash":"1","buying_power":"2","options_approved_level":2,"account_blocked":true}"#).unwrap();
    assert_eq!((a.options_trading_level, a.blocked), (2, true));
    let p = parse_positions(r#"[{"symbol":"X","qty":"3","side":"short","avg_entry_price":"1.0"},{"symbol":"Y","qty":"-1","side":"short"}]"#).unwrap();
    assert_eq!((p[0].1, p[1].1), (-3, -1));
    let c = parse_clock(r#"{"timestamp":"2026-10-05T10:00:00.5-04:00","is_open":false,"next_open":"2026-10-06T09:30:00-04:00","next_close":"2026-10-06T16:00:00-04:00"}"#).unwrap();
    assert_eq!(c.0, parse_time("2026-10-05T14:00:00.5Z").unwrap());
    assert_eq!(
        parse_error(
            422,
            r#"{"code":40010001,"message":"client_order_id must be unique"}"#
        ),
        ("40010001".into(), "client_order_id must be unique".into())
    );
    assert_eq!(
        parse_error(502, "<html>"),
        ("http_502".into(), "<html>".into())
    );
}

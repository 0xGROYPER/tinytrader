//! Stock orders: the equity allowlist, short sales, limit-on-close, the share cap, the
//! ×1 notional, and the shapes the intent refuses.
mod support;
use support::*;

fn stock(
    cid: &str,
    symbol: &str,
    side: &str,
    effect: &str,
    qty: u32,
    price: &str,
    tif: &str,
) -> String {
    format!(
        r#"{{"client_order_id":"{cid}","shape":"single","instrument":{{"class":"equity","symbol":"{symbol}"}},"side":"{side}","effect":"{effect}","qty":{qty},"type":"limit","limit_price":"{price}","time_in_force":"{tif}"}}"#
    )
}

fn cap_of(r: &serde_json::Value) -> (Option<&str>, Option<&str>) {
    (r["reason"].as_str(), r["detail"]["cap"].as_str())
}

#[test]
fn stock_orders_follow_the_profile() {
    let mut cfg = config(root("equities"));
    cfg.profile.equities = vec!["AAPL".into(), "BRK.B".into(), "KO".into()];
    cfg.profile.max_open = 10;
    let gw = scripted(cfg, &format!(r#"{{"clock_ts":{T0}}}"#));
    let mut c = Conn::open(&gw, "reactor", None);

    // Allowlisted, on the close: 10 × $222.50 = $2,225, under the $2,500 cap at ×1.
    let r = c.submit(
        "r1",
        &stock("wse-1", "AAPL", "buy", "open", 10, "222.50", "cls"),
    );
    assert_eq!(r["outcome"], "accepted");
    assert_eq!(
        c.submit(
            "r2",
            &stock("wse-2", "BRK.B", "buy", "open", 1, "480.00", "day")
        )["outcome"],
        "accepted"
    );

    let r = c.submit(
        "r3",
        &stock("wse-3", "MSFT", "buy", "open", 1, "400.00", "day"),
    );
    assert_eq!(
        (r["reason"].as_str(), r["detail"]["equity"].as_str()),
        (Some("unsupported"), Some("MSFT"))
    );

    // Selling to open is a short sale: refused without --allow-short; selling to close isn't.
    // The close is on KO, where no order of its own is working (AAPL's buy is).
    let r = c.submit(
        "r4",
        &stock("wse-4", "AAPL", "sell", "open", 1, "220.00", "day"),
    );
    assert_eq!(r["detail"]["short_sale"], "AAPL");
    assert_eq!(
        c.submit(
            "r5",
            &stock("wse-5", "KO", "sell", "close", 1, "220.00", "cls")
        )["outcome"],
        "accepted"
    );

    // No share cap: size is bounded by dollars, at ×1.
    assert_eq!(
        cap_of(&c.submit(
            "r7",
            &stock("wse-7", "AAPL", "buy", "open", 12, "222.50", "day")
        )),
        (Some("cap"), Some("notional"))
    );
    gw.stop().unwrap();
}

#[test]
fn short_sales_pass_when_allowed() {
    let mut cfg = config(root("equities-short"));
    cfg.profile.equities = vec!["INTC".into()];
    cfg.profile.allow_short = true;
    let gw = scripted(cfg, &format!(r#"{{"clock_ts":{T0}}}"#));
    let mut c = Conn::open(&gw, "reactor", None);
    assert_eq!(
        c.submit(
            "r1",
            &stock("wse-1", "INTC", "sell", "open", 70, "27.30", "cls")
        )["outcome"],
        "accepted"
    );
    gw.stop().unwrap();
}

#[test]
fn the_intent_refuses_mixed_or_misplaced_stock_shapes() {
    let mut cfg = config(root("equities-shapes"));
    cfg.profile.equities = vec!["AAPL".into()];
    let gw = scripted(cfg, &format!(r#"{{"clock_ts":{T0}}}"#));
    let mut c = Conn::open(&gw, "reactor", None);
    let path = |r: serde_json::Value| {
        (
            r["reason"].as_str().map(String::from),
            r["detail"]["path"].as_str().map(String::from),
        )
    };
    let invalid = |p: &str| (Some("validation".to_string()), Some(p.to_string()));

    // Limit-on-close is for stock singles only.
    let option_on_close =
        single("wse-1", CALL, "buy", "open", 1, "1.25").replace(r#""day""#, r#""cls""#);
    assert_eq!(
        path(c.submit("r1", &option_on_close)),
        invalid("intent.time_in_force")
    );
    // Not a ticker.
    assert_eq!(
        path(c.submit(
            "r2",
            &stock("wse-2", "aapl", "buy", "open", 1, "1.00", "day")
        )),
        invalid("intent.instrument.symbol")
    );
    // Combos are options only.
    let mixed = combo(
        "wse-3",
        &[(CALL, 1, "buy", "open"), (PUT_A, 1, "sell", "open")],
        1,
        "0.50",
    )
    .replacen(
        r#""class":"option","symbol":"SPY261005P00570000""#,
        r#""class":"equity","symbol":"AAPL""#,
        1,
    );
    assert_eq!(path(c.submit("r3", &mixed)), invalid("intent.legs[1]"));
    gw.stop().unwrap();
}

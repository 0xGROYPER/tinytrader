//! A minimal strategy client: market data from a relay (`tinytrader.relay/1`),
//! orders through an order gateway (`tinytrader.gateway/1`). Explained in
//! `docs/tutorials/strategy-client.md`.
//!
//! ```text
//! RELAY=127.0.0.1:7610 GATEWAY=127.0.0.1:7601 CLIENT=my-strategy TOKEN_FILE=my-strategy.token \
//!   START=2026-10-12T13:30:00Z cargo run -p order-gateway --example strategy_client
//! ```
//!
//! It replays the relay from `START`, counts quotes per instrument until the
//! relay says the replay is complete, then asks the gateway for the account
//! and its positions. With `SUBMIT=SYMBOL:PRICE` it also sends one 1-share
//! buy limit order for the day (use a sim or paper gateway).

use std::collections::BTreeMap;
use std::io::{BufReader, Write as _};
use std::net::TcpStream;

use dbn::decode::{DbnMetadata as _, DecodeRecordRef as _};
use dbn::{RecordRefEnum, SystemCode};
use order_gateway::client::Client;
use serde_json::{Value, json};

fn env(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.is_empty())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    if let Some(relay) = env("RELAY") {
        market_data(&relay, env("START"))?;
    }
    if let Some(gateway) = env("GATEWAY") {
        let client = env("CLIENT").unwrap_or_else(|| "my-strategy".into());
        let token = std::fs::read_to_string(env("TOKEN_FILE").ok_or("set TOKEN_FILE")?)?;
        orders(&gateway, &client, token.trim(), env("SUBMIT"))?;
    }
    Ok(())
}

/// Join the relay, replay from `start` (or join live), and count quotes per
/// instrument until `ReplayCompleted`.
fn market_data(addr: &str, start: Option<String>) -> Result<(), Box<dyn std::error::Error>> {
    let mut stream = TcpStream::connect(addr)?;
    let mut join = json!({ "protocol": "tinytrader.relay/1", "client": "my-strategy" });
    if let Some(start) = start {
        join["start"] = json!(start);
    }
    stream.write_all(format!("{join}\n").as_bytes())?;

    let mut decoder = dbn::decode::dbn::Decoder::new(BufReader::new(stream))?;
    println!(
        "relay: dataset {}, schema {:?}",
        decoder.metadata().dataset,
        decoder.metadata().schema
    );
    let mut names: BTreeMap<u32, String> = BTreeMap::new();
    let mut quotes: BTreeMap<u32, u64> = BTreeMap::new();
    while let Some(rec) = decoder.decode_record_ref()? {
        let Ok(rec) = rec.as_enum() else { continue }; // record kinds this build doesn't know
        match rec {
            RecordRefEnum::InstrumentDef(d) => {
                names.insert(d.hd.instrument_id, d.raw_symbol()?.replace(' ', ""));
            }
            RecordRefEnum::Mbp1(m) => *quotes.entry(m.hd.instrument_id).or_default() += 1,
            RecordRefEnum::Cmbp1(m) => *quotes.entry(m.hd.instrument_id).or_default() += 1,
            RecordRefEnum::System(m) if matches!(m.code(), Ok(SystemCode::ReplayCompleted)) => {
                break;
            }
            RecordRefEnum::System(m) => println!("relay: {}", m.msg()?), // upstream-down, gaps, ...
            RecordRefEnum::Error(m) => return Err(format!("relay refused: {}", m.err()?).into()),
            _ => {}
        }
    }
    for (id, n) in &quotes {
        let name = names.get(id).map_or("?", String::as_str);
        println!("relay: {name} ({id}): {n} quotes before live");
    }
    Ok(())
}

/// One gateway connection: authenticate, query, and maybe submit one order.
fn orders(
    addr: &str,
    client: &str,
    token: &str,
    submit: Option<String>,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut gw = Client::connect(addr.parse()?, client, token, None, false)?;
    let welcome = next(&mut gw, |m| m["type"] == "welcome" || m["type"] == "error")?;
    if welcome["type"] == "error" {
        return Err(format!("gateway refused: {}", welcome["code"]).into());
    }
    println!(
        "gateway: account {} on {} ({}), state {}",
        welcome["account"], welcome["broker"], welcome["environment"], welcome["state"]
    );

    let account = request(&mut gw, "r1", "account", json!({}))?;
    println!("gateway: account {}", account["account"]);
    let positions = request(&mut gw, "r2", "positions", json!({}))?;
    println!("gateway: positions {}", positions["positions"]);

    if let Some(spec) = submit {
        let (symbol, price) = spec.split_once(':').ok_or("SUBMIT=SYMBOL:PRICE")?;
        // The id must start with this client's prefix, and is never reused.
        let id = format!("strat-{}", order_gateway::wall_ns());
        let intent = json!({
            "client_order_id": id, "shape": "single",
            "instrument": { "class": "equity", "symbol": symbol },
            "side": "buy", "effect": "open", "qty": 1,
            "type": "limit", "limit_price": price, "time_in_force": "day",
        });
        let r = request(&mut gw, "r3", "submit", json!({ "intent": intent }))?;
        // accepted (not a fill), rejected, not_sent, duplicate, or unknown:
        // never resend an unknown; the gateway resolves it and says so.
        println!("gateway: submit {} -> {}", id, r["outcome"]);
    }
    Ok(())
}

/// Send a request and return its response; events and heartbeats in between
/// are skipped here (a real strategy keeps them: fills arrive as events).
fn request(
    gw: &mut Client,
    id: &str,
    kind: &str,
    fields: Value,
) -> Result<Value, Box<dyn std::error::Error>> {
    let mut body = fields;
    body["request_id"] = json!(id);
    gw.send(kind, &body.to_string())?;
    let r = next(gw, |m| m["type"] == "error" || m["request_id"] == id)?;
    if r["type"] == "error" {
        return Err(format!("gateway closed: {}", r["code"]).into());
    }
    Ok(r)
}

fn next(
    gw: &mut Client,
    want: impl Fn(&Value) -> bool,
) -> Result<Value, Box<dyn std::error::Error>> {
    while let Some(line) = gw.line()? {
        let m: Value = serde_json::from_str(&line)?;
        if want(&m) {
            return Ok(m);
        }
    }
    Err("the gateway closed the connection".into())
}

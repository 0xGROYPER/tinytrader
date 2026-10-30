//! A strategy that only watches: joins a session (its control address as the
//! argument, or `TT_SESSION`), reads every step's records and answers `ready`, and
//! prints what it saw. The smallest client of the session protocol.
use dbn::decode::{DbnDecoder, DecodeRecordRef};
use order_gateway::client::Client;
use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Write};
use std::net::TcpStream;
use std::time::Instant;

fn main() -> Result<(), String> {
    let addr = std::env::args()
        .nth(1)
        .or_else(|| std::env::var("TT_SESSION").ok())
        .ok_or("usage: watch CONTROL_ADDR (or TT_SESSION)")?;
    let e = |e: std::io::Error| e.to_string();
    let mut ctl = TcpStream::connect(&addr).map_err(e)?;
    let mut cr = BufReader::new(ctl.try_clone().map_err(e)?);
    let mut line = String::new();
    let mut recv = |cr: &mut BufReader<TcpStream>| -> Result<Value, String> {
        line.clear();
        cr.read_line(&mut line).map_err(|e| e.to_string())?;
        serde_json::from_str(&line).map_err(|e| e.to_string())
    };
    let send = |ctl: &mut TcpStream, v: Value| {
        ctl.write_all(format!("{v}\n").as_bytes())
            .map_err(|e| e.to_string())
    };
    send(
        &mut ctl,
        json!({"type": "hello", "protocol": "tinytrader.session/1", "strategy": "watch", "revision": "example"}),
    )?;
    let w = recv(&mut cr)?;
    let mut data = TcpStream::connect(w["data"].as_str().ok_or("no data address")?).map_err(e)?;
    data.write_all(b"{\"protocol\":\"tinytrader.relay/1\",\"client\":\"watch\"}\n")
        .map_err(e)?;
    let mut dec = DbnDecoder::new(BufReader::new(data)).map_err(|e| e.to_string())?;
    let mut gateways = Vec::new();
    for (name, o) in w["orders"].as_object().into_iter().flatten() {
        let mut c = Client::connect(
            o["addr"]
                .as_str()
                .unwrap_or_default()
                .parse()
                .map_err(|e| format!("{e}"))?,
            o["client"].as_str().unwrap_or_default(),
            o["token"].as_str().unwrap_or_default(),
            None,
            false,
        )
        .map_err(e)?;
        c.line().map_err(e)?;
        gateways.push((name.clone(), c));
    }
    send(&mut ctl, json!({"type": "ready", "as_of": 0, "sent": {}}))?;
    let t = Instant::now();
    let mut records = 0_u64;
    loop {
        let step = recv(&mut cr)?;
        if step["type"] == "end" {
            let secs = t.elapsed().as_secs_f64();
            println!("{step}");
            println!(
                "{records} records in {secs:.1}s ({:.0}/s)",
                records as f64 / secs
            );
            return Ok(());
        }
        for _ in 0..step["records"].as_u64().unwrap_or(0) {
            dec.decode_record_ref().map_err(|e| e.to_string())?;
            records += 1;
        }
        send(
            &mut ctl,
            json!({"type": "ready", "as_of": step["as_of"], "sent": {}}),
        )?;
    }
}

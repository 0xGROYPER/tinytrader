//! Live accounts: order events from Alpaca's `trade_updates` stream, and
//! position prices from tinytrader's own relays.
//!
//! Each account keeps one `trade_updates` websocket; every order event asks
//! for that account to be read again. Positions are valued from relay prices
//! (the bid/ask midpoint, or the last trade when a side is missing): stocks
//! from the IEX relay, options from the OPRA relay for their root. The
//! service connects only to relays covering what is held.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tinytrader_observe::lock;

use dbn::decode::{AsyncDbnDecoder, DbnMetadata as _};
use dbn::{RecordRefEnum, UNDEF_PRICE};
use futures_util::{SinkExt as _, StreamExt as _};
use serde_json::{Value, json};
use tokio::io::AsyncWriteExt as _;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio_tungstenite::tungstenite::Message;

/// Latest prices by symbol, as relays name instruments.
#[derive(Default)]
pub struct Prices {
    marks: Mutex<HashMap<String, f64>>,
}

impl Prices {
    pub fn get(&self, symbol: &str) -> Option<f64> {
        lock(&self.marks).get(symbol).copied()
    }
    fn set(&self, symbol: &str, price: f64) {
        lock(&self.marks).insert(symbol.to_owned(), price);
    }
}

/// Which relay prices which positions.
#[derive(Clone, Debug, Default)]
pub struct Relays {
    /// Address of the relay for stocks.
    pub stocks: Option<String>,
    /// Address of the relay for each option root.
    pub options: HashMap<String, String>,
}

impl Relays {
    /// `stocks=relay-iex:7610;SPY=relay-opra-spy:7600;SPXW=relay-opra-index:7603`.
    pub fn parse(spec: &str) -> Self {
        let mut r = Self::default();
        for entry in spec.split(';').map(str::trim).filter(|e| !e.is_empty()) {
            let Some((k, addr)) = entry.split_once('=') else {
                continue;
            };
            let (k, addr) = (k.trim(), addr.trim().to_owned());
            if k.eq_ignore_ascii_case("stocks") {
                r.stocks = Some(addr);
            } else {
                r.options.insert(k.to_uppercase(), addr);
            }
        }
        r
    }

    /// The relay and relay symbol for a position, if any relay prices it.
    pub fn price_key(&self, symbol: &str, asset_class: &str) -> Option<(String, String)> {
        if asset_class == "us_option" {
            let (root, rest) = occ(symbol)?;
            let addr = self.options.get(&root)?;
            // OPRA names options in OCC form: the root padded to six characters.
            Some((addr.clone(), format!("{root:<6}{rest}")))
        } else {
            Some((self.stocks.clone()?, symbol.to_owned()))
        }
    }
}

/// Split an Alpaca option symbol (`XSP261005C00776000`) into its root and
/// the 15 characters of expiry, right and strike.
fn occ(symbol: &str) -> Option<(String, String)> {
    let cut = symbol.len().checked_sub(15)?;
    let (root, rest) = symbol.split_at(cut);
    let ok = !root.is_empty()
        && root.bytes().all(|b| b.is_ascii_alphanumeric())
        && rest[..6].bytes().all(|b| b.is_ascii_digit())
        && matches!(rest.as_bytes()[6], b'C' | b'P')
        && rest[7..].bytes().all(|b| b.is_ascii_digit());
    ok.then(|| (root.to_owned(), rest.to_owned()))
}

/// Connections to the relays that price current positions.
pub struct Feeds {
    prices: Arc<Prices>,
    running: Mutex<HashMap<String, JoinHandle<()>>>,
}

impl Feeds {
    pub fn new(prices: Arc<Prices>) -> Self {
        Self {
            prices,
            running: Mutex::new(HashMap::new()),
        }
    }

    /// Connect to exactly these relays, dropping any others.
    pub fn want(&self, addrs: &HashSet<String>) {
        let mut running = lock(&self.running);
        running.retain(|addr, task| {
            let keep = addrs.contains(addr);
            if !keep {
                task.abort();
            }
            keep
        });
        for addr in addrs {
            if !running.contains_key(addr) {
                let (addr, prices) = (addr.clone(), self.prices.clone());
                running.insert(addr.clone(), tokio::spawn(follow(addr, prices)));
            }
        }
    }
}

/// Follow one relay, keeping prices current, reconnecting on failure.
async fn follow(addr: String, prices: Arc<Prices>) {
    loop {
        if let Err(e) = read_relay(&addr, &prices).await {
            eprintln!("alpaca-account: relay {addr}: {e}");
        }
        tokio::time::sleep(Duration::from_secs(5)).await;
    }
}

async fn read_relay(addr: &str, prices: &Prices) -> Result<(), String> {
    let mut stream = tokio::net::TcpStream::connect(addr)
        .await
        .map_err(|e| e.to_string())?;
    stream
        .write_all(b"{\"protocol\":\"tinytrader.relay/1\"}\n")
        .await
        .map_err(|e| e.to_string())?;
    let mut dec = AsyncDbnDecoder::new(stream)
        .await
        .map_err(|e| e.to_string())?;
    let _ = dec.metadata();
    let mut names: HashMap<u32, String> = HashMap::new();
    let px = |p: i64| (p != UNDEF_PRICE).then(|| p as f64 / 1e9);
    loop {
        let Some(rec) = dec.decode_record_ref().await.map_err(|e| e.to_string())? else {
            return Ok(());
        };
        let Ok(rec) = rec.as_enum() else { continue };
        let (id, bid, ask, trade) = match rec {
            RecordRefEnum::InstrumentDef(d) => {
                if let Ok(s) = d.raw_symbol() {
                    names.insert(d.hd.instrument_id, s.to_owned());
                }
                continue;
            }
            RecordRefEnum::Mbp1(m) => {
                let l = &m.levels[0];
                (
                    m.hd.instrument_id,
                    px(l.bid_px),
                    px(l.ask_px),
                    (m.action as u8 == b'T').then(|| px(m.price)).flatten(),
                )
            }
            RecordRefEnum::Cmbp1(m) => {
                let l = &m.levels[0];
                (
                    m.hd.instrument_id,
                    px(l.bid_px),
                    px(l.ask_px),
                    (m.action as u8 == b'T').then(|| px(m.price)).flatten(),
                )
            }
            _ => continue,
        };
        let Some(symbol) = names.get(&id) else {
            continue;
        };
        let mark = match (bid, ask) {
            (Some(b), Some(a)) if a >= b => Some((a + b) / 2.0),
            _ => trade,
        };
        if let Some(m) = mark {
            prices.set(symbol, m);
        }
    }
}

/// Value an account's positions from relay prices where available, and its
/// equity as cash plus their market value. Positions priced by a relay are
/// marked `"priced_by": "relay"`, the rest `"alpaca"`.
pub fn revalue(doc: &mut Value, relays: &Relays, prices: &Prices) {
    let Some(positions) = doc["positions"].as_array_mut() else {
        return;
    };
    let (mut long, mut short) = (0.0, 0.0);
    for p in positions.iter_mut() {
        let symbol = p["symbol"].as_str().unwrap_or_default().to_owned();
        let class = p["asset_class"].as_str().unwrap_or_default().to_owned();
        let qty = p["qty"].as_f64().unwrap_or(0.0);
        let mark = relays
            .price_key(&symbol, &class)
            .and_then(|(_, key)| prices.get(&key));
        if let Some(price) = mark {
            let multiplier = if class == "us_option" { 100.0 } else { 1.0 };
            let value = qty * price * multiplier;
            let cost = p["cost_basis"].as_f64().unwrap_or(0.0);
            p["current_price"] = json!(price);
            p["market_value"] = json!(value);
            p["unrealized_pl"] = json!(value - cost);
            if cost != 0.0 {
                p["unrealized_plpc"] = json!((value - cost) / cost.abs());
            }
            if let Some(prev) = p["lastday_price"].as_f64().filter(|v| *v > 0.0) {
                p["change_today"] = json!(price / prev - 1.0);
            }
            p["priced_by"] = json!("relay");
        } else {
            p["priced_by"] = json!("alpaca");
        }
        let v = p["market_value"].as_f64().unwrap_or(0.0);
        if v >= 0.0 { long += v } else { short += v }
    }
    let any_relay = positions.iter().any(|p| p["priced_by"] == "relay");
    if any_relay && let Some(cash) = doc["account"]["cash"].as_f64() {
        doc["account"]["long_market_value"] = json!(long);
        doc["account"]["short_market_value"] = json!(short);
        doc["account"]["equity"] = json!(cash + long + short);
    }
}

/// Relays needed for the positions in these account documents.
pub fn wanted(docs: &[Value], relays: &Relays) -> HashSet<String> {
    docs.iter()
        .flat_map(|d| d["positions"].as_array().cloned().unwrap_or_default())
        .filter_map(|p| {
            relays
                .price_key(
                    p["symbol"].as_str()?,
                    p["asset_class"].as_str().unwrap_or_default(),
                )
                .map(|(a, _)| a)
        })
        .collect()
}

/// Listen to one account's order events, sending its index on every event
/// that can change positions or cash. Reconnects until aborted.
pub async fn trade_updates(
    url: String,
    key: String,
    secret: String,
    index: usize,
    events: mpsc::Sender<usize>,
) {
    loop {
        if let Err(e) = listen(&url, &key, &secret, index, &events).await {
            eprintln!("alpaca-account: trade updates: {e}");
        }
        tokio::time::sleep(Duration::from_secs(5)).await;
    }
}

async fn listen(
    url: &str,
    key: &str,
    secret: &str,
    index: usize,
    events: &mpsc::Sender<usize>,
) -> Result<(), String> {
    let (mut ws, _) = tokio_tungstenite::connect_async(url)
        .await
        .map_err(|e| e.to_string())?;
    let auth = json!({ "action": "auth", "key": key, "secret": secret });
    ws.send(Message::text(auth.to_string()))
        .await
        .map_err(|e| e.to_string())?;
    while let Some(msg) = ws.next().await {
        let text = match msg.map_err(|e| e.to_string())? {
            Message::Text(t) => t.to_string(),
            Message::Binary(b) => String::from_utf8_lossy(&b).into_owned(),
            Message::Close(_) => return Ok(()),
            _ => continue,
        };
        let v: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
        match v["stream"].as_str() {
            Some("authorization") if v["data"]["status"] == "authorized" => {
                let listen =
                    json!({ "action": "listen", "data": { "streams": ["trade_updates"] } });
                ws.send(Message::text(listen.to_string()))
                    .await
                    .map_err(|e| e.to_string())?;
            }
            Some("authorization") => return Err("not authorized".into()),
            Some("listening") => {
                eprintln!("alpaca-account: account {index} listening for order events")
            }
            Some("trade_updates") => {
                let _ = events.send(index).await;
            }
            _ => {}
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_alpaca_options_to_opra_symbols() {
        let r = Relays::parse(
            "stocks=relay-iex:7610; XSP=relay-opra-xsp:7602; SPXW=relay-opra-index:7603",
        );
        assert_eq!(
            r.price_key("XSP261005C00776000", "us_option"),
            Some(("relay-opra-xsp:7602".into(), "XSP   261005C00776000".into()))
        );
        assert_eq!(
            r.price_key("SPXW261016P05800000", "us_option"),
            Some((
                "relay-opra-index:7603".into(),
                "SPXW  261016P05800000".into()
            ))
        );
        assert_eq!(
            r.price_key("SPY", "us_equity"),
            Some(("relay-iex:7610".into(), "SPY".into()))
        );
        assert_eq!(r.price_key("QQQ261016C00500000", "us_option"), None); // no QQQ relay listed
    }

    #[test]
    fn values_positions_from_relay_prices() {
        let r = Relays::parse("XSP=relay:1");
        let prices = Prices::default();
        prices.set("XSP   261005C00776000", 1.5);
        let mut doc = json!({
            "account": { "cash": 1000.0, "equity": 900.0 },
            "positions": [
                { "symbol": "XSP261005C00776000", "asset_class": "us_option", "qty": -2.0, "cost_basis": -400.0, "lastday_price": 2.0 },
                { "symbol": "AAPL", "asset_class": "us_equity", "qty": 1.0, "market_value": 200.0 }
            ]
        });
        revalue(&mut doc, &r, &prices);
        let p = &doc["positions"][0];
        assert_eq!(p["market_value"], json!(-300.0));
        assert_eq!(p["unrealized_pl"], json!(100.0));
        assert_eq!(p["priced_by"], json!("relay"));
        assert_eq!(doc["positions"][1]["priced_by"], json!("alpaca"));
        assert_eq!(doc["account"]["equity"], json!(1000.0 + 200.0 - 300.0));
    }
}

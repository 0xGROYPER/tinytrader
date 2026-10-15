//! The order gateways the console may act through: the Accounts widget's
//! Flatten and Resume buttons as the operator's admin client `ops`
//! (`CONSOLE_GATEWAYS`, `CONSOLE_GATEWAY_TOKEN`), and the Spread builder's
//! orders as the trade client `console`, whose order ids start `tt-`
//! (`CONSOLE_TRADE_TOKEN`).
//!
//! Each request opens one connection to the account's gateway
//! (tinytrader.gateway/1: NDJSON over TCP), sends `hello`, then the one
//! request, and returns the gateway's response. An account without a
//! gateway (a live account) gets none of this. Tokens are never logged or
//! returned.

use std::collections::BTreeMap;
use std::time::Duration;

use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt as _, AsyncWriteExt as _, BufReader};
use tokio::net::TcpStream;

const SCHEMA: &str = "tinytrader.gateway/1";
/// The console's trade client and its order id prefix.
pub const TRADER: &str = "console";
pub const PREFIX: &str = "tt-";

/// Which client a request goes as.
#[derive(Clone, Copy)]
pub enum As {
    /// The operator's admin client (flatten, resume).
    Ops,
    /// The console's trade client (the Spread builder's orders).
    Trade,
}

pub struct Gateways {
    /// Account name -> `host:port`.
    pub addrs: BTreeMap<String, String>,
    token: Option<String>,
    trade_token: Option<String>,
}

impl Gateways {
    /// `paper-1=host.docker.internal:7701;paper-3=...`, and the admin token
    /// from `CONSOLE_GATEWAY_TOKEN` or `CONSOLE_GATEWAY_TOKEN_FILE`.
    #[must_use]
    pub fn from_env() -> Self {
        let addrs = parse(&std::env::var("CONSOLE_GATEWAYS").unwrap_or_default());
        let token = tinytrader_observe::secret("CONSOLE_GATEWAY_TOKEN")
            .ok()
            .filter(|t| !t.is_empty());
        let trade_token = tinytrader_observe::secret("CONSOLE_TRADE_TOKEN")
            .ok()
            .filter(|t| !t.is_empty());
        Self {
            addrs,
            token,
            trade_token,
        }
    }

    /// Whether the console can send orders (its trade token is configured).
    #[must_use]
    pub fn can_trade(&self) -> bool {
        self.trade_token.is_some()
    }

    /// As the admin client: connect and authenticate; then, with a request,
    /// send it and read its response. Returns (welcome, response).
    pub async fn call(
        &self,
        account: &str,
        request: Option<Value>,
        wait: Duration,
    ) -> Result<(Value, Option<Value>), String> {
        self.call_as(As::Ops, account, request, wait).await
    }

    /// `call` as either client.
    pub async fn call_as(
        &self,
        who: As,
        account: &str,
        request: Option<Value>,
        wait: Duration,
    ) -> Result<(Value, Option<Value>), String> {
        let addr = self
            .addrs
            .get(account)
            .ok_or("no gateway for this account")?;
        let (client, token) = match who {
            As::Ops => (
                "ops",
                self.token
                    .as_deref()
                    .ok_or("no gateway token configured (CONSOLE_GATEWAY_TOKEN)")?,
            ),
            As::Trade => (
                TRADER,
                self.trade_token
                    .as_deref()
                    .ok_or("no trade token configured (CONSOLE_TRADE_TOKEN)")?,
            ),
        };
        let run = async {
            let stream = TcpStream::connect(addr)
                .await
                .map_err(|e| format!("gateway {account}: {e}"))?;
            let (read, mut write) = stream.into_split();
            let mut lines = BufReader::new(read).lines();
            let mut send = async |v: Value| {
                let mut line = v.to_string();
                line.push('\n');
                write
                    .write_all(line.as_bytes())
                    .await
                    .map_err(|e| format!("gateway {account}: {e}"))
            };
            send(json!({ "schema": SCHEMA, "type": "hello", "client": client, "token": token, "resume_after_seq": null })).await?;
            let next = async |lines: &mut tokio::io::Lines<
                BufReader<tokio::net::tcp::OwnedReadHalf>,
            >|
                   -> Result<Value, String> {
                let line = lines
                    .next_line()
                    .await
                    .map_err(|e| format!("gateway {account}: {e}"))?
                    .ok_or_else(|| format!("gateway {account} closed the connection"))?;
                serde_json::from_str::<Value>(&line).map_err(|e| format!("gateway {account}: {e}"))
            };
            let welcome = loop {
                let v = next(&mut lines).await?;
                match v["type"].as_str() {
                    Some("welcome") => break v,
                    Some("error") => {
                        return Err(format!(
                            "gateway {account}: {}",
                            v["code"].as_str().unwrap_or("error")
                        ));
                    }
                    _ => {}
                }
            };
            let Some(mut req) = request else {
                return Ok((welcome, None));
            };
            let rid = format!(
                "console-{}",
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_or(0, |d| d.as_nanos())
            );
            req["schema"] = json!(SCHEMA);
            req["request_id"] = json!(rid);
            send(req).await?;
            loop {
                let v = next(&mut lines).await?;
                if v["type"] == "response" && v["request_id"] == rid.as_str() {
                    return Ok((welcome, Some(v)));
                }
                if v["type"] == "error" {
                    return Err(format!(
                        "gateway {account}: {}",
                        v["code"].as_str().unwrap_or("error")
                    ));
                }
            }
        };
        tokio::time::timeout(wait, run)
            .await
            .map_err(|_| format!("gateway {account}: no answer in {} s", wait.as_secs()))?
    }
}

/// A gateway order intent (tinytrader.gateway/1, section 6) from the
/// console's order tickets: `{legs: [{symbol, side, ratio?}], qty,
/// limit_price, effect?}`, with `limit_price` the net per unit, + debit and
/// − credit (a single leg's is its positive price); or with `type: "market"`,
/// `ref_price` in its place, the natural when sent, for the gateway's caps.
/// Options only; the id is given. The gateway checks everything again.
pub fn intent(order: &Value, id: &str) -> Result<Value, String> {
    let legs = order["legs"].as_array().ok_or("legs: a list")?;
    if !(1..=4).contains(&legs.len()) {
        return Err("1 to 4 legs".into());
    }
    let effect = order["effect"].as_str().unwrap_or("open");
    if !matches!(effect, "open" | "close") {
        return Err("effect: open or close".into());
    }
    let qty = order["qty"]
        .as_u64()
        .filter(|q| (1..=100).contains(q))
        .ok_or("qty: 1 to 100")?;
    let market = match order["type"].as_str().unwrap_or("limit") {
        "limit" => false,
        "market" => true,
        _ => return Err("type: limit or market".into()),
    };
    let key = if market { "ref_price" } else { "limit_price" };
    let price = order[key]
        .as_f64()
        .filter(|p| p.is_finite())
        .ok_or(format!("{key}: a number"))?;
    let cents = (price * 100.0).round();
    if cents == 0.0 || (price * 100.0 - cents).abs() > 1e-6 {
        return Err(format!("{key}: non-zero, whole cents"));
    }
    let mut out = Vec::new();
    for l in legs {
        let symbol = l["symbol"]
            .as_str()
            .filter(|s| occ_ok(s))
            .ok_or("leg symbol: an OCC option symbol")?;
        let side = l["side"]
            .as_str()
            .filter(|s| matches!(*s, "buy" | "sell"))
            .ok_or("leg side: buy or sell")?;
        let ratio = l["ratio"].as_u64().unwrap_or(1);
        if !(1..=10).contains(&ratio) {
            return Err("leg ratio: 1 to 10".into());
        }
        if out
            .iter()
            .any(|o: &Value| o["instrument"]["symbol"] == symbol)
        {
            return Err("legs must be distinct contracts".into());
        }
        out.push(json!({ "instrument": { "class": "option", "symbol": symbol }, "ratio": ratio, "side": side, "effect": effect }));
    }
    let price = format!("{:.2}", cents / 100.0);
    let mut i = if out.len() == 1 {
        if cents < 0.0 {
            return Err(format!("a single leg's {key} is positive"));
        }
        let l = &out[0];
        json!({ "client_order_id": id, "shape": "single", "instrument": l["instrument"], "side": l["side"], "effect": effect, "qty": qty })
    } else {
        json!({ "client_order_id": id, "shape": "combo", "legs": out, "qty": qty })
    };
    i["type"] = json!(if market { "market" } else { "limit" });
    i[key] = json!(price);
    i["time_in_force"] = json!("day");
    Ok(i)
}

/// `^[A-Z]{1,6}\d{6}[CP]\d{8}$`
fn occ_ok(s: &str) -> bool {
    let b = s.as_bytes();
    let n = b.len();
    (16..=21).contains(&n)
        && b[..n - 15].iter().all(u8::is_ascii_uppercase)
        && b[n - 15..n - 9].iter().all(u8::is_ascii_digit)
        && matches!(b[n - 9], b'C' | b'P')
        && b[n - 8..].iter().all(u8::is_ascii_digit)
}

/// `account=host:port` entries separated by `;`; malformed ones are skipped.
fn parse(text: &str) -> BTreeMap<String, String> {
    text.split(';')
        .filter_map(|e| {
            let (a, addr) = e.trim().split_once('=')?;
            let (a, addr) = (a.trim(), addr.trim());
            (!a.is_empty() && !addr.is_empty()).then(|| (a.to_owned(), addr.to_owned()))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accounts_parse_and_malformed_entries_are_skipped() {
        let g = parse("paper-1=h:7701; paper-3=h:7700;bad;=x");
        assert_eq!(g.keys().collect::<Vec<_>>(), ["paper-1", "paper-3"]);
        assert_eq!(g["paper-3"], "h:7700");
    }

    #[test]
    fn builds_intents() {
        let condor = json!({ "qty": 2, "limit_price": -1.25, "legs": [
            { "symbol": "SAP261120C00300000", "side": "sell" }, { "symbol": "SAP261120C00310000", "side": "buy" },
            { "symbol": "SAP261120P00260000", "side": "sell" }, { "symbol": "SAP261120P00250000", "side": "buy" },
        ] });
        let i = intent(&condor, "tt-1").unwrap();
        assert_eq!(i["shape"], "combo");
        assert_eq!(i["limit_price"], "-1.25");
        assert_eq!(
            i["legs"][0],
            json!({ "instrument": { "class": "option", "symbol": "SAP261120C00300000" }, "ratio": 1, "side": "sell", "effect": "open" })
        );
        let one = json!({ "qty": 1, "limit_price": 2.1, "legs": [{ "symbol": "XSP261016C00780000", "side": "buy" }] });
        let i = intent(&one, "tt-2").unwrap();
        assert_eq!(
            (
                i["shape"].as_str(),
                i["limit_price"].as_str(),
                i["side"].as_str()
            ),
            (Some("single"), Some("2.10"), Some("buy"))
        );
        // A market order: its reference price instead of a limit.
        let m = json!({ "type": "market", "qty": 1, "ref_price": -2.13, "effect": "close", "legs": [
            { "symbol": "SPY261015C00765000", "side": "buy" }, { "symbol": "SPY261015C00769000", "side": "sell" },
        ] });
        let i = intent(&m, "tt-4").unwrap();
        assert_eq!(
            (i["type"].as_str(), i["ref_price"].as_str()),
            (Some("market"), Some("-2.13"))
        );
        assert!(i.get("limit_price").is_none());
        for bad in [
            json!({ "type": "stop", "qty": 1, "limit_price": 1.0, "legs": [{ "symbol": "SPY261016C00780000", "side": "buy" }] }),
            json!({ "type": "market", "qty": 1, "limit_price": 1.0, "legs": [{ "symbol": "SPY261016C00780000", "side": "buy" }] }),
            json!({ "qty": 1, "limit_price": 1.234, "legs": [{ "symbol": "SPY261016C00780000", "side": "buy" }] }),
            json!({ "qty": 1, "limit_price": -1.0, "legs": [{ "symbol": "SPY261016C00780000", "side": "buy" }] }),
            json!({ "qty": 0, "limit_price": 1.0, "legs": [{ "symbol": "SPY261016C00780000", "side": "buy" }] }),
            json!({ "qty": 1, "limit_price": 1.0, "legs": [{ "symbol": "SPY", "side": "buy" }] }),
            json!({ "qty": 1, "limit_price": 1.0, "legs": [{ "symbol": "SPY261016C00780000", "side": "buy" }, { "symbol": "SPY261016C00780000", "side": "sell" }] }),
        ] {
            assert!(intent(&bad, "tt-3").is_err(), "{bad}");
        }
    }
}

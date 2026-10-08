//! The order gateways the console may act through, for the Accounts widget's
//! Flatten and Resume buttons (`CONSOLE_GATEWAYS`, `CONSOLE_GATEWAY_TOKEN`).
//!
//! Each request opens one connection to the account's gateway as the
//! operator's admin client `ops` (tinytrader.gateway/1: NDJSON over TCP),
//! sends `hello`, then the one request, and returns the gateway's response.
//! An account without a gateway (a live account) gets none of this. The
//! token is never logged or returned.

use std::collections::BTreeMap;
use std::time::Duration;

use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt as _, AsyncWriteExt as _, BufReader};
use tokio::net::TcpStream;

const SCHEMA: &str = "tinytrader.gateway/1";
const CLIENT: &str = "ops";

pub struct Gateways {
    /// Account name -> `host:port`.
    pub addrs: BTreeMap<String, String>,
    token: Option<String>,
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
        Self { addrs, token }
    }

    /// Connect and authenticate; then, with a request, send it and read its
    /// response. Returns (welcome, response).
    pub async fn call(
        &self,
        account: &str,
        request: Option<Value>,
        wait: Duration,
    ) -> Result<(Value, Option<Value>), String> {
        let addr = self
            .addrs
            .get(account)
            .ok_or("no gateway for this account")?;
        let token = self
            .token
            .as_deref()
            .ok_or("no gateway token configured (CONSOLE_GATEWAY_TOKEN)")?;
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
            send(json!({ "schema": SCHEMA, "type": "hello", "client": CLIENT, "token": token, "resume_after_seq": null })).await?;
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
}

//! Alpaca IEX source: one websocket stream of quotes and trades, encoded as
//! MBP-1 (with trade conditions for clients that ask), and catch-up from
//! Alpaca's historical API.
//!
//! Alpaca allows one market data stream per account, so this relay must be
//! the only process streaming with its key.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use futures_util::{SinkExt as _, StreamExt as _};
use glacier_dbn::TradeConditions;
use glacier_dbn::dbn::{FlagSet, Mbp1Msg, RecordRef};
use glacier_observe::{Observer, State};
use relay::iex::{self, Encoder, RawQuote, RawTrade, Row};
use serde::Deserialize;
use serde_json::json;
use serde_json::value::RawValue;
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message;

use crate::hub::Hub;
use crate::protocol;

const LIVE_URL: &str = "wss://stream.data.alpaca.markets/v2/iex";
const TEST_URL: &str = "wss://stream.data.alpaca.markets/v2/test";
const HISTORY_URL: &str = "https://data.alpaca.markets/v2/stocks";
/// History is sent to clients in chunks of about this many bytes.
const CHUNK: usize = 64 * 1024;

pub struct Alpaca {
    pub key: String,
    pub secret: String,
    pub symbols: Vec<String>,
    /// Use Alpaca's test stream (symbol `FAKEPACA`, any time of day) and
    /// accept its non-IEX venue codes.
    pub test: bool,
    pub http: reqwest::Client,
}

/// One element of a stream frame: its type and symbol.
#[derive(Deserialize)]
struct Tag<'a> {
    #[serde(rename = "T")]
    kind: &'a str,
    #[serde(rename = "S", default)]
    symbol: Option<&'a str>,
    #[serde(default)]
    msg: Option<&'a str>,
    #[serde(default)]
    code: Option<i64>,
    /// Venue codes: `x` for trades, `bx`/`ax` for quotes.
    #[serde(default)]
    x: Option<&'a str>,
    #[serde(default)]
    bx: Option<&'a str>,
    #[serde(default)]
    ax: Option<&'a str>,
}

/// Encoded records of a frame or a stretch of history, in order, each trade
/// followed by its conditions if any.
struct Encoded {
    records: Vec<(Mbp1Msg, Option<TradeConditions>)>,
}

impl Encoded {
    /// `F_LAST` on every record unless the next is the same instrument at the
    /// same time.
    fn mark_last(&mut self) {
        let n = self.records.len();
        for i in 0..n {
            let (cur, next) = (&self.records[i].0, self.records.get(i + 1).map(|r| &r.0));
            let same = next.is_some_and(|nx| {
                nx.hd.instrument_id == cur.hd.instrument_id && nx.hd.ts_event == cur.hd.ts_event
            });
            if !same {
                self.records[i].0.flags = FlagSet::empty().set_last();
            }
        }
    }

    fn bytes(&self, conditions: bool) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.records.len() * 96);
        for (rec, cond) in &self.records {
            out.extend_from_slice(RecordRef::from(rec).as_ref());
            if conditions && let Some(c) = cond.and_then(|c| c.encode().ok()) {
                out.extend_from_slice(&c);
            }
        }
        out
    }
}

impl Alpaca {
    /// One definition record per symbol.
    fn definitions(&self) -> Vec<u8> {
        let mut out = Vec::new();
        for s in &self.symbols {
            let d = Encoder::new(s).definition(s);
            out.extend_from_slice(RecordRef::from(&d).as_ref());
        }
        out
    }

    /// Keep the stream up, publishing its records to the hub.
    pub async fn run(self: Arc<Self>, hub: Arc<Hub>, observer: Observer) {
        hub.publish(self.definitions(), self.symbols.len() as u64);
        let mut encoders: HashMap<String, Encoder> = self
            .symbols
            .iter()
            .map(|s| (s.clone(), Encoder::new(s)))
            .collect();
        let mut backoff = Duration::from_secs(1);
        loop {
            let began = Instant::now();
            let outcome = self.stream(&hub, &observer, &mut encoders).await;
            hub.upstream_down();
            let why = match outcome {
                Ok(()) => "Alpaca closed the stream".to_owned(),
                Err(e) => e,
            };
            eprintln!("relay: {why}; reconnecting in {}s", backoff.as_secs());
            observer.record_error(&why);
            observer.set_state(State::Degraded, &format!("{why}; reconnecting"));
            if began.elapsed() > Duration::from_secs(60) {
                backoff = Duration::from_secs(1);
            }
            tokio::time::sleep(backoff).await;
            // Alpaca refuses a second connection for a while after a drop.
            backoff = (backoff * 2).min(Duration::from_secs(60));
        }
    }

    async fn stream(
        &self,
        hub: &Hub,
        observer: &Observer,
        encoders: &mut HashMap<String, Encoder>,
    ) -> Result<(), String> {
        let url = if self.test { TEST_URL } else { LIVE_URL };
        let (mut ws, _) = tokio_tungstenite::connect_async(url)
            .await
            .map_err(|e| format!("cannot connect to Alpaca: {e}"))?;
        while let Some(msg) = ws.next().await {
            let text = match msg.map_err(|e| format!("Alpaca stream failed: {e}"))? {
                Message::Text(t) => t,
                Message::Ping(p) => {
                    let _ = ws.send(Message::Pong(p)).await;
                    continue;
                }
                Message::Close(_) => return Ok(()),
                _ => continue,
            };
            let ts_recv = protocol::now_ns();
            let items: Vec<&RawValue> =
                serde_json::from_str(&text).map_err(|e| format!("unreadable stream frame: {e}"))?;
            let mut frame = Encoded {
                records: Vec::new(),
            };
            let mut refused = 0u64;
            for item in items {
                let tag: Tag = serde_json::from_str(item.get())
                    .map_err(|e| format!("unreadable stream message: {e}"))?;
                match tag.kind {
                    "success" if tag.msg == Some("connected") => {
                        let auth =
                            json!({ "action": "auth", "key": self.key, "secret": self.secret });
                        ws.send(Message::text(auth.to_string()))
                            .await
                            .map_err(|e| format!("cannot authenticate: {e}"))?;
                    }
                    "success" if tag.msg == Some("authenticated") => {
                        let sub = json!({ "action": "subscribe", "trades": self.symbols, "quotes": self.symbols });
                        ws.send(Message::text(sub.to_string()))
                            .await
                            .map_err(|e| format!("cannot subscribe: {e}"))?;
                    }
                    "subscription" => {
                        hub.upstream_up();
                        observer.set_state(State::Ready, "live");
                        eprintln!(
                            "relay: Alpaca {} stream live for {}",
                            if self.test { "test" } else { "IEX" },
                            self.symbols.join(",")
                        );
                    }
                    "error" => {
                        return Err(format!(
                            "Alpaca refused: {} {}",
                            tag.code.unwrap_or(0),
                            tag.msg.unwrap_or("")
                        ));
                    }
                    "q" | "t" => {
                        let Some(enc) = tag.symbol.and_then(|s| encoders.get_mut(s)) else {
                            refused += 1;
                            continue;
                        };
                        let iex =
                            |code: Option<&str>| self.test || matches!(code, None | Some("V"));
                        if tag.kind == "q" {
                            let row: RawQuote =
                                serde_json::from_str(item.get()).map_err(|e| e.to_string())?;
                            match row.parse() {
                                Ok(q) if iex(tag.bx) && iex(tag.ax) => {
                                    frame.records.push((enc.quote(&q, ts_recv), None))
                                }
                                _ => refused += 1,
                            }
                        } else {
                            let row: RawTrade =
                                serde_json::from_str(item.get()).map_err(|e| e.to_string())?;
                            match row.parse() {
                                Ok(t) if iex(tag.x) => {
                                    let (rec, cond) = enc.trade(&t, ts_recv);
                                    frame.records.push((rec, cond.ok()));
                                }
                                _ => refused += 1,
                            }
                        }
                    }
                    _ => {}
                }
            }
            if refused > 0 {
                hub.refused_rows
                    .fetch_add(refused, std::sync::atomic::Ordering::Relaxed);
            }
            if !frame.records.is_empty() {
                frame.mark_last();
                let n = frame.records.len() as u64;
                hub.publish_pair(frame.bytes(false).into(), frame.bytes(true).into(), n);
            }
        }
        Ok(())
    }

    /// Fetch history for every symbol from `start` to `cut` (both in ns since
    /// the epoch) and send it as chunks of records, as the relay encodes it.
    pub fn catch_up(
        self: &Arc<Self>,
        start: u64,
        cut: u64,
        conditions: bool,
    ) -> mpsc::Receiver<Result<Vec<u8>, String>> {
        let (tx, rx) = mpsc::channel(4);
        let me = self.clone();
        tokio::spawn(async move {
            match me.history(start, cut).await {
                Ok(encoded) => {
                    let bytes = encoded.bytes(conditions);
                    for chunk in bytes.chunks(CHUNK) {
                        if tx.send(Ok(chunk.to_vec())).await.is_err() {
                            return;
                        }
                    }
                }
                Err(e) => {
                    let _ = tx
                        .send(Err(format!("cannot catch up from Alpaca: {e}")))
                        .await;
                }
            }
        });
        rx
    }

    async fn history(&self, start: u64, cut: u64) -> Result<Encoded, String> {
        let mut per_symbol: Vec<Encoded> = Vec::new();
        for symbol in &self.symbols {
            let trades = self.pages(symbol, "trades", start, cut).await?;
            let quotes = self.pages(symbol, "quotes", start, cut).await?;
            let mut t = Vec::new();
            for page in &trades {
                let p: Page = serde_json::from_str(page).map_err(|e| format!("trade page: {e}"))?;
                for row in p
                    .trades
                    .and_then(|mut m| m.remove(symbol.as_str()))
                    .unwrap_or_default()
                {
                    t.push(row.parse()?);
                }
            }
            let mut q = Vec::new();
            for page in &quotes {
                let p: Page = serde_json::from_str(page).map_err(|e| format!("quote page: {e}"))?;
                for row in p
                    .quotes
                    .and_then(|mut m| m.remove(symbol.as_str()))
                    .unwrap_or_default()
                {
                    q.push(row.parse()?);
                }
            }
            let rows: Vec<Row> = iex::merge(t, q)?;
            let (records, conds) = iex::encode_day(symbol, &rows)?;
            let mut conds = conds.into_iter();
            let records = records
                .into_iter()
                .map(|r| {
                    let c = (r.action as u8 == b'T').then(|| conds.next()).flatten();
                    (r, c)
                })
                .collect();
            per_symbol.push(Encoded { records });
        }
        // Interleave the symbols by time, keeping each symbol's own order.
        let mut all: Vec<(u64, usize, usize)> = Vec::new();
        for (s, e) in per_symbol.iter().enumerate() {
            all.extend(
                e.records
                    .iter()
                    .enumerate()
                    .map(|(i, r)| (r.0.hd.ts_event, s, i)),
            );
        }
        all.sort_by_key(|&(ts, s, i)| (ts, s, i));
        Ok(Encoded {
            records: all
                .into_iter()
                .map(|(_, s, i)| per_symbol[s].records[i].clone())
                .collect(),
        })
    }

    /// Every page of one kind of history for a symbol, oldest first.
    async fn pages(
        &self,
        symbol: &str,
        kind: &str,
        start: u64,
        end: u64,
    ) -> Result<Vec<String>, String> {
        let (start, end) = (rfc3339(start), rfc3339(end));
        let mut pages = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let mut req = self
                .http
                .get(format!("{HISTORY_URL}/{kind}"))
                .header("APCA-API-KEY-ID", &self.key)
                .header("APCA-API-SECRET-KEY", &self.secret)
                .query(&[
                    ("symbols", symbol),
                    ("feed", "iex"),
                    ("start", &start),
                    ("end", &end),
                    ("limit", "10000"),
                    ("sort", "asc"),
                ]);
            if let Some(t) = &token {
                req = req.query(&[("page_token", t)]);
            }
            let r = req.send().await.map_err(|e| format!("{kind}: {e}"))?;
            if !r.status().is_success() {
                return Err(format!("{kind}: Alpaca answered {}", r.status()));
            }
            let text = r.text().await.map_err(|e| format!("{kind}: {e}"))?;
            #[derive(Deserialize)]
            struct Next {
                next_page_token: Option<String>,
            }
            token = serde_json::from_str::<Next>(&text)
                .ok()
                .and_then(|n| n.next_page_token);
            pages.push(text);
            if token.is_none() {
                return Ok(pages);
            }
        }
    }
}

#[derive(Deserialize)]
struct Page<'a> {
    #[serde(borrow, default)]
    quotes: Option<std::collections::BTreeMap<&'a str, Vec<RawQuote<'a>>>>,
    #[serde(borrow, default)]
    trades: Option<std::collections::BTreeMap<&'a str, Vec<RawTrade<'a>>>>,
}

/// Nanoseconds since the epoch as RFC 3339 with nanoseconds.
fn rfc3339(ns: u64) -> String {
    let t = time::OffsetDateTime::from_unix_timestamp_nanos(i128::from(ns))
        .unwrap_or(time::OffsetDateTime::UNIX_EPOCH);
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.{:09}Z",
        t.year(),
        u8::from(t.month()),
        t.day(),
        t.hour(),
        t.minute(),
        t.second(),
        t.nanosecond()
    )
}

//! `tinytrader.session/1`, the control connection: newline-delimited JSON.
//!
//! | Direction | Message |
//! |---|---|
//! | strategy → session | `hello{protocol, strategy, revision}`, once, first |
//! | session → strategy | `welcome{session, mode, data, orders: {venue: {addr, client, token, prefix}}, days, strategy_latency_ms}` |
//! | strategy → session | `ready{as_of: 0}`, once joined to the data stream and every venue |
//! | session → strategy | `step{step, as_of, records, timers: [id], reports: {venue: last_seq}}` |
//! | strategy → session | `ready{as_of, sent: {venue: order messages sent}, timers: [{at, id}]}` |
//! | session → strategy | `end{status, reason, audit, steps, records}` |
//!
//! A step's records follow it on the data stream, written as the strategy reads
//! them; its reports
//! name each venue's last event, which the strategy reads from its gateway before
//! acting. `sent` counts the submits and cancels the strategy has sent to each venue
//! since it first connected (order-gateway.md §10). Timers are set in `ready`, for a
//! time after the step's.
use serde_json::Value;
use std::io::{BufRead, Read, Write};

/// The protocol's name.
pub const PROTOCOL: &str = "tinytrader.session/1";
/// The longest line read.
const MAX_LINE: usize = 1 << 20;

/// A message from the strategy.
#[derive(Debug, PartialEq, Eq)]
pub enum FromStrategy {
    /// Who it is.
    Hello {
        /// Its name.
        strategy: String,
        /// Its code's revision.
        revision: String,
    },
    /// Done with a step.
    Ready {
        /// The step's time.
        as_of: u64,
        /// Order messages sent to each venue so far.
        sent: Vec<(String, u64)>,
        /// Timers to set: `(at, id)`.
        timers: Vec<(u64, String)>,
    },
}

/// Writes one message.
///
/// # Errors
/// The connection failed.
pub fn send(w: &mut impl Write, v: &Value) -> Result<(), String> {
    let mut line = v.to_string();
    line.push('\n');
    w.write_all(line.as_bytes())
        .and_then(|()| w.flush())
        .map_err(|e| format!("control: {e}"))
}

/// Reads one message.
///
/// # Errors
/// A closed or silent connection, or a message that isn't one.
pub fn read(r: &mut impl BufRead) -> Result<FromStrategy, String> {
    let mut line = String::new();
    let mut limited = Read::take(&mut *r, MAX_LINE as u64);
    let n = limited
        .read_line(&mut line)
        .map_err(|e| format!("control: {e}"))?;
    if n == 0 {
        return Err("the strategy closed the control connection".into());
    }
    parse(&line)
}

fn parse(line: &str) -> Result<FromStrategy, String> {
    let v: Value = serde_json::from_str(line).map_err(|e| format!("control: {e}"))?;
    let text = |k: &str| v[k].as_str().map(str::to_owned);
    match v["type"].as_str() {
        Some("hello") => {
            if v["protocol"] != PROTOCOL {
                return Err(format!("hello must name {PROTOCOL}"));
            }
            Ok(FromStrategy::Hello {
                strategy: text("strategy").ok_or("hello needs strategy")?,
                revision: text("revision").unwrap_or_default(),
            })
        }
        Some("ready") => {
            let as_of = v["as_of"].as_u64().ok_or("ready needs as_of")?;
            let mut sent = Vec::new();
            if let Some(o) = v["sent"].as_object() {
                for (k, n) in o {
                    sent.push((k.clone(), n.as_u64().ok_or("sent counts are integers")?));
                }
            }
            let mut timers = Vec::new();
            for t in v["timers"].as_array().into_iter().flatten() {
                let at = t["at"].as_u64().ok_or("a timer needs at")?;
                let id = t["id"].as_str().ok_or("a timer needs id")?.to_owned();
                timers.push((at, id));
            }
            Ok(FromStrategy::Ready {
                as_of,
                sent,
                timers,
            })
        }
        other => Err(format!("control: unknown message {other:?}")),
    }
}

#[cfg(test)]
mod tests {
    use super::{FromStrategy, parse};

    #[test]
    fn messages_parse() {
        assert_eq!(
            parse(
                r#"{"type":"hello","protocol":"tinytrader.session/1","strategy":"toy","revision":"abc"}"#
            ),
            Ok(FromStrategy::Hello {
                strategy: "toy".into(),
                revision: "abc".into()
            })
        );
        assert_eq!(
            parse(
                r#"{"type":"ready","as_of":5,"sent":{"options":2},"timers":[{"at":9,"id":"close"}]}"#
            ),
            Ok(FromStrategy::Ready {
                as_of: 5,
                sent: vec![("options".into(), 2)],
                timers: vec![(9, "close".into())]
            })
        );
        assert!(parse(r#"{"type":"hello","protocol":"x","strategy":"toy"}"#).is_err());
        assert!(parse(r#"{"type":"ready"}"#).is_err());
        assert!(parse("not json").is_err());
    }
}

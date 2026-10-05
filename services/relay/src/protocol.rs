//! The `glacier.relay/1` wire protocol: the client's join request, and the
//! DBN records the relay writes besides the provider's own.

use std::time::{SystemTime, UNIX_EPOCH};

use databento::dbn::encode::dbn::AsyncMetadataEncoder;
use databento::dbn::{ErrorCode, ErrorMsg, Metadata, RecordRef, SystemCode, SystemMsg};
use serde::Deserialize;
use time::OffsetDateTime;

pub const PROTOCOL: &str = "glacier.relay/1";

/// The one line of JSON a client sends after connecting.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Join {
    pub protocol: String,
    /// Replay from this time, then continue live. Absent means live from now.
    #[serde(default, with = "time::serde::rfc3339::option")]
    pub start: Option<OffsetDateTime>,
    /// Interleave trade condition records (sources that have them).
    #[serde(default)]
    pub conditions: bool,
}

/// Parse a join line, refusing other protocol versions.
pub fn parse_join(line: &str) -> Result<Join, String> {
    let join: Join =
        serde_json::from_str(line.trim()).map_err(|e| format!("unreadable join request: {e}"))?;
    if join.protocol != PROTOCOL {
        return Err(format!(
            "unsupported protocol {:?}; this relay serves {PROTOCOL}",
            join.protocol
        ));
    }
    Ok(join)
}

pub fn now_ns() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_nanos()).unwrap_or(u64::MAX))
}

fn bytes_of<R: databento::dbn::HasRType>(record: &R) -> Vec<u8> {
    RecordRef::from(record).as_ref().to_vec()
}

/// A `SystemMsg` with a standard code, such as `Heartbeat`.
pub fn system_code(code: SystemCode) -> Vec<u8> {
    match SystemMsg::new(now_ns(), Some(code), "") {
        Ok(m) => bytes_of(&m),
        Err(_) => bytes_of(&SystemMsg::heartbeat(now_ns())),
    }
}

/// A `SystemMsg` carrying relay status text, such as `upstream-down since=…`.
pub fn system_text(text: &str) -> Vec<u8> {
    match SystemMsg::new(now_ns(), None, text) {
        Ok(m) => bytes_of(&m),
        Err(_) => bytes_of(&SystemMsg::heartbeat(now_ns())),
    }
}

/// An `ErrorMsg`; the relay closes the connection after sending one.
pub fn error(code: ErrorCode, text: &str) -> Vec<u8> {
    bytes_of(&ErrorMsg::new(now_ns(), Some(code), text, true))
}

/// A DBN stream header.
pub async fn header(metadata: &Metadata) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    AsyncMetadataEncoder::new(&mut out)
        .encode(metadata)
        .await
        .map_err(|e| format!("cannot encode metadata: {e}"))?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use databento::dbn::rtype;

    #[test]
    fn parses_join_requests() {
        let j = parse_join(r#"{"protocol":"glacier.relay/1"}"#).unwrap();
        assert!(j.start.is_none() && !j.conditions);
        let j = parse_join(
            r#"{"protocol":"glacier.relay/1","start":"2026-10-05T13:30:00Z","conditions":true}"#,
        )
        .unwrap();
        assert_eq!(j.start.unwrap().unix_timestamp(), 1_791_207_000);
        assert!(j.conditions);
        assert!(parse_join(r#"{"protocol":"hyphae"}"#).is_err());
        assert!(parse_join(r#"{"protocol":"glacier.relay/1","extra":1}"#).is_err());
        assert!(parse_join("not json").is_err());
    }

    #[test]
    fn builds_standard_records() {
        let hb = system_code(SystemCode::Heartbeat);
        assert_eq!(hb.len(), std::mem::size_of::<SystemMsg>());
        assert_eq!(hb[1], rtype::SYSTEM);
        let e = error(ErrorCode::InvalidSubscription, "bad");
        assert_eq!(e.len(), std::mem::size_of::<ErrorMsg>());
        assert_eq!(e[1], rtype::ERROR);
    }
}

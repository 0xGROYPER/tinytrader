//! The hash-chained JSONL order journal (section 11): one file per UTC day, every line
//! an event plus `prev`, each line fsync'd before the effect or delivery it announces.
use crate::intent::hex;
use crate::protocol::Obj;
use crate::safety::date_string;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

pub(crate) const FORMAT: &str = "tinytrader.gateway-journal/1";
const ZERO: &str = "0000000000000000000000000000000000000000000000000000000000000000";
/// `,"prev":"<64 hex>"}` appended to the wire event.
const PREV_LEN: usize = 75;

pub(crate) struct Journal {
    dir: PathBuf,
    account: String,
    file: Option<(String, File)>,
    last_hash: String,
    pub(crate) last_seq: u64,
    pub(crate) last_ts: u64,
    _lock: File,
}

fn sync_dir(dir: &Path) -> Result<(), String> {
    File::open(dir)
        .and_then(|d| d.sync_all())
        .map_err(|e| format!("journal directory fsync: {e}"))
}

fn day_files(dir: &Path) -> Result<Vec<String>, String> {
    let mut names: Vec<String> = fs::read_dir(dir)
        .map_err(|e| format!("journal read: {e}"))?
        .filter_map(Result::ok)
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|n| n.len() == 16 && n.get(10..) == Some(".jsonl"))
        .collect();
    names.sort();
    Ok(names)
}

impl Journal {
    /// Takes the lock, truncates only a torn final line, verifies the chain and `seq`.
    pub(crate) fn open(root: &Path, account: &str) -> Result<(Self, Vec<Value>), String> {
        let dir = root.join("journal");
        fs::create_dir_all(&dir).map_err(|e| format!("state root: {e}"))?;
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(root.join("lock"))
            .map_err(|e| format!("lock file: {e}"))?;
        lock.try_lock()
            .map_err(|_| "another gateway process holds the state root lock".to_string())?;
        let names = day_files(&dir)?;
        let mut events = Vec::new();
        let (mut last_hash, mut last_seq, mut last_ts) = (ZERO.to_string(), 0, 0);
        for (i, name) in names.iter().enumerate() {
            let path = dir.join(name);
            let mut bytes = fs::read(&path).map_err(|e| format!("{name}: {e}"))?;
            if i + 1 == names.len() && !bytes.is_empty() && bytes.last() != Some(&b'\n') {
                let keep = bytes.iter().rposition(|b| *b == b'\n').map_or(0, |p| p + 1);
                bytes.truncate(keep);
                let f = OpenOptions::new()
                    .write(true)
                    .open(&path)
                    .map_err(|e| e.to_string())?;
                f.set_len(keep as u64)
                    .and_then(|()| f.sync_all())
                    .map_err(|e| e.to_string())?;
            }
            for (n, raw) in bytes.split_inclusive(|b| *b == b'\n').enumerate() {
                let bad = |why: &str| format!("journal {name} line {}: {why}", n + 1);
                let text = std::str::from_utf8(raw).map_err(|_| bad("not UTF-8"))?;
                let body = text.strip_suffix('\n').ok_or_else(|| bad("torn line"))?;
                let split = body
                    .len()
                    .checked_sub(PREV_LEN)
                    .ok_or_else(|| bad("short"))?;
                let (event, prev) = body.split_at(split);
                if prev != format!(",\"prev\":\"{last_hash}\"}}") {
                    return Err(bad("hash chain mismatch"));
                }
                let line = format!("{event}}}");
                let value: Value = serde_json::from_str(&line).map_err(|_| bad("bad JSON"))?;
                let seq = value["seq"].as_u64().ok_or_else(|| bad("no seq"))?;
                if seq != last_seq + 1 {
                    return Err(bad("seq discontinuity"));
                }
                last_seq = seq;
                last_ts = value["ts_gateway"].as_u64().unwrap_or(last_ts);
                last_hash = hex(&Sha256::digest(raw));
                events.push(value);
            }
        }
        let file = match names.last() {
            Some(name) => {
                let f = OpenOptions::new()
                    .append(true)
                    .open(dir.join(name))
                    .map_err(|e| format!("{name}: {e}"))?;
                Some((name[..10].to_string(), f))
            }
            None => None,
        };
        let journal = Self {
            dir,
            account: account.to_owned(),
            file,
            last_hash,
            last_seq,
            last_ts,
            _lock: lock,
        };
        Ok((journal, events))
    }

    /// Appends one event and returns `(seq, wire line)`. Starts a new day file (with its
    /// `day` event) when `ts` falls on a later UTC day.
    pub(crate) fn append(
        &mut self,
        ts: u64,
        kind: &str,
        body: &str,
    ) -> Result<(u64, String), String> {
        let day = date_string(i64::try_from(ts / 86_400_000_000_000).unwrap_or(0));
        if self.file.as_ref().is_none_or(|(d, _)| *d < day) {
            let previous = self.file.take().map(|(d, _)| format!("{d}.jsonl"));
            let path = self.dir.join(format!("{day}.jsonl"));
            let f = OpenOptions::new()
                .create_new(true)
                .append(true)
                .open(&path)
                .map_err(|e| format!("journal create {}: {e}", path.display()))?;
            sync_dir(&self.dir)?;
            self.file = Some((day, f));
            let fields = Obj::new()
                .s("format", FORMAT)
                .opt_s("previous_file", previous.as_deref())
                .s("previous_last_hash", &self.last_hash.clone())
                .done();
            self.write(ts, "day", &fields)?;
        }
        self.write(ts, kind, body)
    }

    fn write(&mut self, ts: u64, kind: &str, body: &str) -> Result<(u64, String), String> {
        let seq = self.last_seq + 1;
        let wire = Obj::msg("event")
            .n("seq", seq)
            .n("ts_gateway", ts)
            .s("account", &self.account)
            .s("kind", kind)
            .merge(body)
            .done();
        let line = format!(
            "{},\"prev\":\"{}\"}}\n",
            &wire[..wire.len() - 1],
            self.last_hash
        );
        let (_, f) = self.file.as_mut().ok_or("journal file")?;
        f.write_all(line.as_bytes())
            .and_then(|()| f.sync_all())
            .map_err(|e| format!("journal append: {e}"))?;
        self.last_hash = hex(&Sha256::digest(line.as_bytes()));
        self.last_seq = seq;
        self.last_ts = ts;
        Ok((seq, wire))
    }

    /// The resume window (current and previous file): `(first_available_seq, lines)`.
    pub(crate) fn window(&self) -> Result<(u64, Vec<String>), String> {
        let names = day_files(&self.dir)?;
        let mut lines = Vec::new();
        for name in names.iter().skip(names.len().saturating_sub(2)) {
            let text = fs::read_to_string(self.dir.join(name)).map_err(|e| e.to_string())?;
            for l in text.lines() {
                lines.push(format!("{}}}", &l[..l.len().saturating_sub(PREV_LEN)]));
            }
        }
        let first = lines
            .first()
            .and_then(|l| serde_json::from_str::<Value>(l).ok())
            .and_then(|v| v["seq"].as_u64())
            .unwrap_or(self.last_seq + 1);
        Ok((first, lines))
    }
}

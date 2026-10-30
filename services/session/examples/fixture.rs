//! Writes a small session fixture for client tests in other languages:
//! `fixture DIR` makes `DIR/job` (a databento-jobs cache job of one day, one call
//! quoted each minute from 10:00 New York on 2026-10-27, its ask 1.30 until 10:02,
//! then 1.24) and `DIR/session.toml` (one source, one venue). Synthetic data, made
//! here.
use dbn::encode::EncodeRecord;
use dbn::encode::dbn::Encoder;
use dbn::{CbboMsg, MappingInterval, Metadata, RecordHeader, SType, Schema, SymbolMapping, rtype};
use sha2::{Digest, Sha256};
use std::path::PathBuf;

const D27: u64 = 1_793_059_200_000_000_000;
const T0: u64 = D27 + 14 * 3_600_000_000_000;

fn main() -> Result<(), String> {
    let dir = PathBuf::from(std::env::args().nth(1).ok_or("usage: fixture DIR")?);
    let job = dir.join("job");
    std::fs::create_dir_all(&job).map_err(|e| e.to_string())?;
    let date = time::Date::from_calendar_date(2026, time::Month::October, 27).map_err(|e| e.to_string())?;
    let meta = Metadata::builder()
        .dataset("OPRA.PILLAR")
        .schema(Some(Schema::Cbbo1M))
        .start(D27)
        .stype_in(Some(SType::Parent))
        .stype_out(SType::InstrumentId)
        .symbols(vec!["SPY.OPT".into()])
        .mappings(vec![SymbolMapping {
            raw_symbol: "SPY   261027C00580000".into(),
            intervals: vec![MappingInterval {
                start_date: date,
                end_date: date.next_day().ok_or("no next day")?,
                symbol: "11".into(),
            }],
        }])
        .build();
    let mut out = Vec::new();
    {
        let mut e = Encoder::with_zstd(&mut out, &meta).map_err(|e| e.to_string())?;
        for m in 0..6 {
            let mut q = CbboMsg {
                hd: RecordHeader::new::<CbboMsg>(rtype::CBBO_1M, 30, 11, u64::MAX),
                ts_recv: T0 + m * 60_000_000_000,
                ..CbboMsg::default_for_schema(Schema::Cbbo1M)
            };
            q.levels[0].bid_px = 1_200_000_000;
            q.levels[0].ask_px = if m < 2 { 1_300_000_000 } else { 1_240_000_000 };
            q.levels[0].bid_sz = 10;
            q.levels[0].ask_sz = 10;
            e.encode_record(&q).map_err(|e| e.to_string())?;
        }
    }
    let name = "opra-pillar-20261027.cbbo-1m.dbn.zst";
    let sha: String = Sha256::digest(&out).iter().map(|b| format!("{b:02x}")).collect();
    let write = |p: PathBuf, b: &[u8]| std::fs::write(&p, b).map_err(|e| format!("{}: {e}", p.display()));
    write(job.join(name), &out)?;
    write(
        job.join("metadata.json"),
        br#"{"query":{"dataset":"OPRA.PILLAR","schema":"cbbo-1m","symbols":["SPY.OPT"],"stype_in":"parent"}}"#,
    )?;
    write(
        job.join("manifest.json"),
        format!(r#"{{"files":[{{"filename":"{name}","hash":"sha256:{sha}"}}]}}"#).as_bytes(),
    )?;
    write(job.join("COMPLETE"), b"")?;
    let config = format!(
        "days = [\"2026-10-27\"]\nready_timeout_s = 20\n\n[[sources]]\nname = \"opra\"\nholdings = \"job:{}\"\nprovider = \"databento\"\ndataset = \"OPRA.PILLAR\"\nsymbol = \"SPY.OPT\"\nschema = \"cbbo-1m\"\n\n[[venues]]\nname = \"options\"\nquotes = [\"opra\"]\nunderlyings = [\"SPY\"]\n",
        job.display()
    );
    write(dir.join("session.toml"), config.as_bytes())?;
    Ok(())
}

//! Reads days through the producer and prints each day's records and pins.
//!
//! `count job DIR DAY…`, `count folder ROOT PROVIDER DATASET SYMBOL SCHEMA DAY…`,
//! or `count store URL PROVIDER DATASET SYMBOL SCHEMA DAY…`.
use dbn::decode::DecodeRecordRef;
use std::path::Path;
use std::time::Instant;
use tinytrader_producer::{Folder, Holdings, Query, StoreApi, StoreProducer};

fn main() -> Result<(), String> {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let (holdings, query, days): (Box<dyn Holdings>, Query, &[String]) = match a.first().map(String::as_str) {
        Some("job") if a.len() > 2 => {
            let meta: serde_json::Value = serde_json::from_str(
                &std::fs::read_to_string(Path::new(&a[1]).join("metadata.json")).map_err(|e| e.to_string())?,
            )
            .map_err(|e| e.to_string())?;
            let q = &meta["query"];
            let query = Query {
                provider: "databento".into(),
                dataset: q["dataset"].as_str().unwrap_or_default().into(),
                symbol: q["symbols"][0].as_str().unwrap_or_default().into(),
                schema: q["schema"].as_str().unwrap_or_default().into(),
            };
            (Box::new(Folder::job(Path::new(&a[1]))?), query, &a[2..])
        }
        Some(kind @ ("folder" | "store")) if a.len() > 6 => {
            let query = Query {
                provider: a[2].clone(),
                dataset: a[3].clone(),
                symbol: a[4].clone(),
                schema: a[5].clone(),
            };
            let h: Box<dyn Holdings> = if kind == "folder" {
                Box::new(Folder::layout(Path::new(&a[1])))
            } else {
                Box::new(StoreApi::new(&a[1]))
            };
            (h, query, &a[6..])
        }
        _ => return Err("usage: count job DIR DAY… | count folder|store WHERE PROVIDER DATASET SYMBOL SCHEMA DAY…".into()),
    };
    let t = Instant::now();
    let mut p = StoreProducer::new(holdings, query, days)?;
    let mut n = 0_u64;
    while p.decode_record_ref().map_err(|e| e.to_string())?.is_some() {
        n += 1;
    }
    let secs = t.elapsed().as_secs_f64();
    println!(
        "{n} records in {secs:.1}s ({:.0}/s) from {}",
        n as f64 / secs,
        p.describe()
    );
    for d in p.pins() {
        for f in &d.files {
            println!(
                "{} {} {} bytes sha256 {} verified={}",
                d.day, f.name, f.bytes, f.sha256, f.verified
            );
        }
        if let Some(m) = &d.missing {
            println!("{} missing: {m}", d.day);
        }
    }
    Ok(())
}

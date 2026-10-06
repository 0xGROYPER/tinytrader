//! Encode one day of Alpaca IEX history the way the relay encodes live data:
//! reads the day's quote and trade pages (tar files of Alpaca historical API
//! responses) and writes DBN files of MBP-1 records and of trade conditions.
//!
//!     iex-day --symbol SPY --day 2026-10-01 \
//!         --quotes quotes/2026-10-01.tar --trades trades/2026-10-01.tar \
//!         --out mbp-1.dbn --conditions conditions.dbn
//!
//! Used to prove the relay's encoding matches stored history, and to
//! re-encode history if the mapping ever changes.

use std::collections::BTreeMap;
use std::fs::File;
use std::io::{BufWriter, Read as _, Write as _};

use clap::Parser;
use relay::iex::{self, RawQuote, RawTrade};
use serde::Deserialize;
use tinytrader_dbn::dbn::encode::dbn::MetadataEncoder;
use tinytrader_dbn::dbn::{Metadata, RecordRef, SType, Schema};

/// Encode a stored day of Alpaca IEX pages as DBN.
#[derive(Parser, Debug)]
struct Args {
    #[arg(long)]
    symbol: String,
    /// The UTC day, `YYYY-MM-DD`.
    #[arg(long)]
    day: String,
    /// Tar of the day's quote pages.
    #[arg(long)]
    quotes: String,
    /// Tar of the day's trade pages.
    #[arg(long)]
    trades: String,
    /// Where to write the MBP-1 records.
    #[arg(long)]
    out: String,
    /// Where to write the trade conditions records.
    #[arg(long)]
    conditions: String,
}

#[derive(Deserialize)]
struct Page<'a> {
    #[serde(borrow, default)]
    quotes: Option<BTreeMap<&'a str, Vec<RawQuote<'a>>>>,
    #[serde(borrow, default)]
    trades: Option<BTreeMap<&'a str, Vec<RawTrade<'a>>>>,
}

/// The JSON pages in a tar (`page-NNNNNN.json.gz`, gzipped API responses;
/// the `.json` beside each is its receipt), in page order.
fn pages(path: &str) -> Result<Vec<String>, String> {
    let mut archive = tar::Archive::new(File::open(path).map_err(|e| format!("{path}: {e}"))?);
    let mut out = BTreeMap::new();
    for entry in archive.entries().map_err(|e| format!("{path}: {e}"))? {
        let mut entry = entry.map_err(|e| format!("{path}: {e}"))?;
        let name = entry
            .path()
            .map_err(|e| e.to_string())?
            .to_string_lossy()
            .into_owned();
        if name.ends_with(".json.gz") {
            let mut text = String::new();
            flate2::read::GzDecoder::new(&mut entry)
                .read_to_string(&mut text)
                .map_err(|e| format!("{name}: {e}"))?;
            out.insert(name, text);
        }
    }
    Ok(out.into_values().collect())
}

fn write(
    path: &str,
    metadata: &Metadata,
    records: impl Iterator<Item = Vec<u8>>,
) -> Result<(), String> {
    let mut w = BufWriter::new(File::create(path).map_err(|e| format!("{path}: {e}"))?);
    MetadataEncoder::new(&mut w)
        .encode(metadata)
        .map_err(|e| e.to_string())?;
    for r in records {
        w.write_all(&r).map_err(|e| e.to_string())?;
    }
    w.flush().map_err(|e| e.to_string())
}

fn run(a: &Args) -> Result<(), String> {
    let day_start = iex::timestamp(&format!("{}T00:00:00Z", a.day))?;
    let (quote_pages, trade_pages) = (pages(&a.quotes)?, pages(&a.trades)?);
    let mut quotes = Vec::new();
    for text in &quote_pages {
        let page: Page = serde_json::from_str(text).map_err(|e| format!("quote page: {e}"))?;
        for row in page
            .quotes
            .and_then(|mut m| m.remove(a.symbol.as_str()))
            .unwrap_or_default()
        {
            quotes.push(row.parse()?);
        }
    }
    let mut trades = Vec::new();
    for text in &trade_pages {
        let page: Page = serde_json::from_str(text).map_err(|e| format!("trade page: {e}"))?;
        for row in page
            .trades
            .and_then(|mut m| m.remove(a.symbol.as_str()))
            .unwrap_or_default()
        {
            trades.push(row.parse()?);
        }
    }
    eprintln!(
        "iex-day: {} quote pages, {} trade pages: {} quotes, {} trades",
        quote_pages.len(),
        trade_pages.len(),
        quotes.len(),
        trades.len()
    );
    let rows = iex::merge(trades, quotes)?;
    let (records, conditions) = iex::encode_day(&a.symbol, &rows)?;
    let meta = |schema| {
        Metadata::builder()
            .dataset(iex::DATASET.to_owned())
            .schema(schema)
            .start(day_start)
            .stype_in(Some(SType::RawSymbol))
            .stype_out(SType::InstrumentId)
            .build()
    };
    write(
        &a.out,
        &meta(Some(Schema::Mbp1)),
        records.iter().map(|r| RecordRef::from(r).as_ref().to_vec()),
    )?;
    write(
        &a.conditions,
        &meta(None),
        conditions
            .iter()
            .map(|c| c.encode().map(|b| b.to_vec()).unwrap_or_default()),
    )?;
    eprintln!(
        "iex-day: wrote {} records and {} conditions",
        records.len(),
        conditions.len()
    );
    Ok(())
}

fn main() {
    if let Err(e) = run(&Args::parse()) {
        eprintln!("iex-day: {e}");
        std::process::exit(1);
    }
}

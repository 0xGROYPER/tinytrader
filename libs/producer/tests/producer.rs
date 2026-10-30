//! The store producer over fixtures written here: a cache job, a folder in the
//! store's layout, and a fake store service. Nothing reads real data.
use dbn::decode::{DbnMetadata, DecodeRecordRef};
use dbn::encode::EncodeRecord;
use dbn::encode::dbn::Encoder;
use dbn::{
    CbboMsg, MappingInterval, Metadata, RecordHeader, RecordRef, SType, Schema, SymbolMapping,
    SymbolMappingMsg, SystemMsg, rtype,
};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;
use tinytrader_producer::{Day, Folder, Holdings, Query, Retry, StoreApi, StoreProducer};

const DAY: u64 = 86_400_000_000_000;
const D27: u64 = 1_793_059_200_000_000_000; // 2026-10-27T00:00Z

fn query() -> Query {
    Query {
        provider: "databento".into(),
        dataset: "OPRA.PILLAR".into(),
        symbol: "SPY.OPT".into(),
        schema: "cbbo-1m".into(),
    }
}

fn date(ns: u64) -> time::Date {
    time::Date::from_julian_day(i32::try_from(ns / DAY).unwrap() + 2_440_588).unwrap()
}

/// One day's file: two contracts, a quote each at `minutes` past the day's
/// 14:00Z, parent symbology, zstd when `zst`.
fn day_file(day0: u64, minutes: &[u64], zst: bool) -> Vec<u8> {
    let maps = [
        ("SPY   261027C00580000", 11_u32),
        ("SPY   261027P00570000", 12),
    ]
    .iter()
    .map(|(raw, iid)| SymbolMapping {
        raw_symbol: (*raw).into(),
        intervals: vec![MappingInterval {
            start_date: date(day0),
            end_date: date(day0 + DAY),
            symbol: iid.to_string(),
        }],
    })
    .collect();
    let meta = Metadata::builder()
        .dataset("OPRA.PILLAR")
        .schema(Some(Schema::Cbbo1M))
        .start(day0)
        .stype_in(Some(SType::Parent))
        .stype_out(SType::InstrumentId)
        .symbols(vec!["SPY.OPT".into()])
        .mappings(maps)
        .build();
    let mut out = Vec::new();
    {
        let write = |enc: &mut dyn FnMut(&CbboMsg)| {
            for m in minutes {
                for iid in [11, 12] {
                    let ts = day0 + 14 * 3_600_000_000_000 + m * 60_000_000_000;
                    enc(&CbboMsg {
                        hd: RecordHeader::new::<CbboMsg>(rtype::CBBO_1M, 30, iid, u64::MAX),
                        ts_recv: ts,
                        ..CbboMsg::default_for_schema(Schema::Cbbo1M)
                    });
                }
            }
        };
        if zst {
            let mut e = Encoder::with_zstd(&mut out, &meta).unwrap();
            write(&mut |r| e.encode_record(r).unwrap());
        } else {
            let mut e = Encoder::new(&mut out, &meta).unwrap();
            write(&mut |r| e.encode_record(r).unwrap());
        }
    }
    out
}

fn sha(b: &[u8]) -> String {
    Sha256::digest(b)
        .iter()
        .map(|x| format!("{x:02x}"))
        .collect()
}

fn tmp(name: &str) -> PathBuf {
    let p = std::env::temp_dir().join(format!("tt-producer-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&p);
    std::fs::create_dir_all(&p).unwrap();
    p
}

/// A cache job holding 10-27 and 10-29, not 10-28.
fn job(dir: &Path, complete: bool, bad_hash: bool) {
    let a = day_file(D27, &[0, 1], true);
    let c = day_file(D27 + 2 * DAY, &[0], true);
    std::fs::write(dir.join("opra-pillar-20261027.cbbo-1m.dbn.zst"), &a).unwrap();
    std::fs::write(dir.join("opra-pillar-20261029.cbbo-1m.dbn.zst"), &c).unwrap();
    std::fs::write(
        dir.join("metadata.json"),
        r#"{"query":{"dataset":"OPRA.PILLAR","schema":"cbbo-1m","symbols":["SPY.OPT"],"stype_in":"parent"}}"#,
    )
    .unwrap();
    let h = if bad_hash { "00".repeat(32) } else { sha(&a) };
    std::fs::write(
        dir.join("manifest.json"),
        format!(
            r#"{{"files":[{{"filename":"opra-pillar-20261027.cbbo-1m.dbn.zst","hash":"sha256:{h}"}},
                         {{"filename":"opra-pillar-20261029.cbbo-1m.dbn.zst","hash":"sha256:{}"}}]}}"#,
            sha(&c)
        ),
    )
    .unwrap();
    if complete {
        std::fs::write(dir.join("COMPLETE"), "").unwrap();
    }
}

fn days(d: &[&str]) -> Vec<String> {
    d.iter().map(|s| (*s).to_owned()).collect()
}

#[derive(Debug, PartialEq)]
enum Out {
    Map(u32, String, String),
    Quote(u64, u32),
    Sys(String),
}

fn describe(r: &RecordRef<'_>) -> Out {
    if let Some(m) = r.get::<SymbolMappingMsg>() {
        Out::Map(
            m.hd.instrument_id,
            m.stype_in_symbol().unwrap().to_owned(),
            m.stype_out_symbol().unwrap().to_owned(),
        )
    } else if let Some(q) = r.get::<CbboMsg>() {
        Out::Quote(q.ts_recv, q.hd.instrument_id)
    } else if let Some(s) = r.get::<SystemMsg>() {
        Out::Sys(s.msg().unwrap().to_owned())
    } else {
        panic!("unexpected record {r:?}")
    }
}

fn drain(p: &mut StoreProducer) -> Result<Vec<Out>, String> {
    let mut v = Vec::new();
    while let Some(r) = p.decode_record_ref().map_err(|e| e.to_string())? {
        v.push(describe(&r));
    }
    Ok(v)
}

fn q(day0: u64, minute: u64, iid: u32) -> Out {
    Out::Quote(day0 + 14 * 3_600_000_000_000 + minute * 60_000_000_000, iid)
}

fn maps() -> [Out; 2] {
    [
        Out::Map(11, "SPY.OPT".into(), "SPY   261027C00580000".into()),
        Out::Map(12, "SPY.OPT".into(), "SPY   261027P00570000".into()),
    ]
}

#[test]
fn a_job_streams_its_days_in_order_with_holes_as_gaps() {
    let dir = tmp("job");
    job(&dir, true, false);
    let holdings = Folder::job(&dir).unwrap();
    let mut p = StoreProducer::new(
        Box::new(holdings),
        query(),
        &days(&["2026-10-27", "2026-10-28", "2026-10-29"]),
    )
    .unwrap();
    assert_eq!(p.metadata().schema, Some(Schema::Cbbo1M));
    let got = drain(&mut p).unwrap();
    let [m1, m2] = maps();
    let [n1, n2] = maps();
    let want = vec![
        m1,
        m2,
        q(D27, 0, 11),
        q(D27, 0, 12),
        q(D27, 1, 11),
        q(D27, 1, 12),
        Out::Sys(format!(
            "store-gap day=2026-10-28 reason=not in job {}",
            dir.file_name().unwrap().to_string_lossy()
        )),
        n1,
        n2,
        q(D27 + 2 * DAY, 0, 11),
        q(D27 + 2 * DAY, 0, 12),
    ];
    assert_eq!(got, want);
    let pins = p.pins();
    assert_eq!(pins.len(), 3);
    assert!(pins[0].files[0].verified);
    assert!(pins[1].files.is_empty() && pins[1].missing.is_some());
    assert_eq!(
        pins[2].files[0].sha256,
        sha(&std::fs::read(dir.join("opra-pillar-20261029.cbbo-1m.dbn.zst")).unwrap())
    );
}

#[test]
fn an_unfinished_job_is_refused() {
    let dir = tmp("unfinished");
    job(&dir, false, false);
    let e = Folder::job(&dir).err().unwrap();
    assert!(e.contains("COMPLETE"), "{e}");
}

#[test]
fn a_job_of_another_symbol_is_refused() {
    let dir = tmp("other");
    job(&dir, true, false);
    let mut other = query();
    other.symbol = "QQQ.OPT".into();
    let e = StoreProducer::new(
        Box::new(Folder::job(&dir).unwrap()),
        other,
        &days(&["2026-10-27"]),
    )
    .err()
    .unwrap();
    assert!(e.contains("not OPRA.PILLAR cbbo-1m QQQ.OPT"), "{e}");
}

#[test]
fn a_hash_that_doesnt_match_fails_the_day() {
    let dir = tmp("badhash");
    job(&dir, true, true);
    let mut p = StoreProducer::new(
        Box::new(Folder::job(&dir).unwrap()),
        query(),
        &days(&["2026-10-27"]),
    )
    .unwrap();
    let e = drain(&mut p).err().unwrap();
    assert!(e.contains("SHA-256"), "{e}");
}

#[test]
fn days_out_of_order_are_refused() {
    let dir = tmp("order");
    job(&dir, true, false);
    let e = StoreProducer::new(
        Box::new(Folder::job(&dir).unwrap()),
        query(),
        &days(&["2026-10-29", "2026-10-27"]),
    )
    .err()
    .unwrap();
    assert!(e.contains("in order"), "{e}");
}

/// The store's layout: a day file, and a day folder of two files (merged on
/// one clock); an empty file is padding, so its day is a gap.
#[test]
fn a_layout_folder_merges_a_day_held_in_several_files() {
    let root = tmp("layout");
    let dir = root.join("Databento/OPRA.PILLAR/SPY.OPT/cbbo-1m");
    std::fs::create_dir_all(dir.join("2026-10-28")).unwrap();
    std::fs::write(dir.join("2026-10-27.dbn"), day_file(D27, &[0], false)).unwrap();
    let d28 = D27 + DAY;
    std::fs::write(
        dir.join("2026-10-28/a.dbn.zst"),
        day_file(d28, &[0, 2], true),
    )
    .unwrap();
    std::fs::write(dir.join("2026-10-28/b.dbn"), day_file(d28, &[1], false)).unwrap();
    std::fs::write(dir.join("2026-10-29.dbn.zst"), b"").unwrap();
    let mut p = StoreProducer::new(
        Box::new(Folder::layout(&root)),
        query(),
        &days(&["2026-10-27", "2026-10-28", "2026-10-29"]),
    )
    .unwrap();
    let got = drain(&mut p).unwrap();
    let [m1, m2] = maps();
    let [n1, n2] = maps();
    assert_eq!(
        got,
        vec![
            m1,
            m2,
            q(D27, 0, 11),
            q(D27, 0, 12),
            n1,
            n2,
            q(d28, 0, 11),
            q(d28, 0, 12),
            q(d28, 1, 11),
            q(d28, 1, 12),
            q(d28, 2, 11),
            q(d28, 2, 12),
            Out::Sys("store-gap day=2026-10-29 reason=not in the folder".into()),
        ]
    );
    assert_eq!(p.pins()[1].files.len(), 2);
    assert!(!p.pins()[1].files[0].verified);
}

/// The fake store's state: the two day files and `/coverage`, the `/files`
/// calls so far, and how many answer 503.
type Fake = (
    Arc<(Vec<u8>, Vec<u8>, serde_json::Value)>,
    Arc<AtomicU32>,
    u32,
);

/// A fake store: `/coverage` holds 10-27 (one file, with its hash) and 10-29
/// (partial), with a volume missing; `/files` answers 503 `busy` times first.
fn fake_store(busy: u32) -> (String, Arc<AtomicU32>) {
    use axum::extract::{Path as P, Query as Q, State};
    use axum::http::StatusCode;
    use axum::routing::get;
    let a = day_file(D27, &[0], true);
    let c = day_file(D27 + 2 * DAY, &[0], true);
    let coverage = serde_json::json!({
        "days": [
            {"day": "2026-10-27", "kind": "complete", "sha256": sha(&a),
             "files": ["vol/Databento/OPRA.PILLAR/SPY.OPT/cbbo-1m/2026-10-27.dbn.zst"]},
            {"day": "2026-10-29", "kind": "partial",
             "files": ["vol/Databento/OPRA.PILLAR/SPY.OPT/cbbo-1m/2026-10-29.dbn.zst"]}
        ],
        "missing_volumes": ["data"]
    });
    let calls = Arc::new(AtomicU32::new(0));
    let state = (Arc::new((a, c, coverage)), calls.clone(), busy);
    let app = axum::Router::new()
        .route(
            "/coverage",
            get(
                |Q(p): Q<std::collections::HashMap<String, String>>,
                 State(s): State<Fake>| async move {
                    assert_eq!(p["symbol"], "SPY.OPT");
                    assert_eq!(p["start"], "2026-10-27");
                    axum::Json(s.0.2.clone())
                },
            ),
        )
        .route(
            "/files/{*name}",
            get(
                |P(name): P<String>,
                 State(s): State<Fake>| async move {
                    if s.1.fetch_add(1, Ordering::SeqCst) < s.2 {
                        return Err((
                            StatusCode::SERVICE_UNAVAILABLE,
                            "volume vol is being scanned",
                        ));
                    }
                    Ok(if name.ends_with("27.dbn.zst") {
                        s.0.0.clone()
                    } else {
                        s.0.1.clone()
                    })
                },
            ),
        )
        .with_state(state);
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async move {
            let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            tx.send(l.local_addr().unwrap()).unwrap();
            axum::serve(l, app).await.unwrap();
        });
    });
    (format!("http://{}", rx.recv().unwrap()), calls)
}

fn quick(n: usize) -> Retry {
    Retry {
        waits: vec![Duration::from_millis(5); n],
    }
}

#[test]
fn the_store_api_plans_retries_and_streams() {
    let (base, calls) = fake_store(2);
    let store = StoreApi::with_retry(&base, quick(3));
    let plan = store
        .plan(&query(), &days(&["2026-10-27", "2026-10-28", "2026-10-29"]))
        .unwrap();
    assert!(matches!(&plan[0], Day::Held { files, partial: false } if files[0].sha256.is_some()));
    assert_eq!(
        plan[1],
        Day::Missing {
            reason: "not held while volumes are missing: data".into()
        }
    );
    assert!(matches!(&plan[2], Day::Held { partial: true, .. }));
    let mut p = StoreProducer::new(
        Box::new(store),
        query(),
        &days(&["2026-10-27", "2026-10-28", "2026-10-29"]),
    )
    .unwrap();
    let got = drain(&mut p).unwrap();
    assert!(got.contains(&Out::Sys(
        "store-gap day=2026-10-28 reason=not held while volumes are missing: data".into()
    )));
    assert!(got.contains(&Out::Sys("store-partial day=2026-10-29".into())));
    assert_eq!(
        got.iter().filter(|o| matches!(o, Out::Quote(..))).count(),
        4
    );
    assert!(p.pins()[0].files[0].verified);
    assert_eq!(calls.load(Ordering::SeqCst), 4); // two 503s, then both files
}

#[test]
fn the_store_api_gives_up_saying_so() {
    let (base, _) = fake_store(100);
    let store = StoreApi::with_retry(&base, quick(2));
    let mut p = StoreProducer::new(Box::new(store), query(), &days(&["2026-10-27"])).unwrap();
    let e = drain(&mut p).err().unwrap();
    assert!(e.contains("gave up after 3 attempts"), "{e}");
}

/// Two producers merge on one clock like any DBN source.
#[test]
fn producers_merge_with_each_other() {
    let dir = tmp("merge");
    job(&dir, true, false);
    let root = tmp("merge-layout");
    let sdir = root.join("Databento/OPRA.PILLAR/SPY.OPT/cbbo-1m");
    std::fs::create_dir_all(&sdir).unwrap();
    std::fs::write(sdir.join("2026-10-27.dbn"), day_file(D27, &[0], false)).unwrap();
    let one = |h: Box<dyn Holdings>| {
        tinytrader_dbn::DbnSource::new(
            StoreProducer::new(h, query(), &days(&["2026-10-27"])).unwrap(),
            0,
        )
    };
    let mut m = tinytrader_dbn::Merge::new(vec![
        one(Box::new(Folder::job(&dir).unwrap())),
        one(Box::new(Folder::layout(&root))),
    ]);
    let mut ranks = Vec::new();
    while let Some(d) = m.next_delivery().unwrap() {
        if d.record().get::<CbboMsg>().is_some() {
            ranks.push((d.key.ts, d.key.rank));
        }
    }
    let t0 = D27 + 14 * 3_600_000_000_000;
    let t1 = t0 + 60_000_000_000;
    assert_eq!(
        ranks,
        [(t0, 0), (t0, 0), (t0, 1), (t0, 1), (t1, 0), (t1, 0)]
    );
}

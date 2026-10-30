//! The merge's order on fixtures generated here: interleaving, ties, the
//! `F_BAD_TS_RECV` offset, untimed and out-of-order records, empty sources.
use std::io::Cursor;
use tinytrader_dbn::dbn::decode::DbnDecoder;
use tinytrader_dbn::dbn::encode::{DbnMetadataEncoder, DbnRecordEncoder, EncodeRecord};
use tinytrader_dbn::dbn::flags::BAD_TS_RECV;
use tinytrader_dbn::dbn::{
    Cmbp1Msg, FlagSet, Mbp1Msg, Metadata, Record, RecordHeader, SType, Schema, SystemMsg,
    UNDEF_TIMESTAMP, rtype,
};
use tinytrader_dbn::{Class, DbnSource, Merge, MergeError};

type Src = DbnSource<DbnDecoder<Cursor<Vec<u8>>>>;

fn meta() -> Metadata {
    Metadata::builder()
        .dataset("TEST.FIXTURE")
        .schema(None)
        .start(0)
        .stype_in(Some(SType::RawSymbol))
        .stype_out(SType::InstrumentId)
        .build()
}

/// A consolidated quote received at `ts_recv`, tagged by `iid`.
fn cmbp(iid: u32, ts_recv: u64) -> Cmbp1Msg {
    Cmbp1Msg {
        hd: RecordHeader::new::<Cmbp1Msg>(rtype::CMBP_1, 1, iid, ts_recv.saturating_sub(5)),
        ts_recv,
        ..Cmbp1Msg::default_for_schema(Schema::Cmbp1)
    }
}

/// An IEX-style quote: an event time and, when `bad`, a receive time that
/// isn't one.
fn mbp(iid: u32, ts_event: u64, ts_recv: u64, bad: bool) -> Mbp1Msg {
    Mbp1Msg {
        hd: RecordHeader::new::<Mbp1Msg>(rtype::MBP_1, 2, iid, ts_event),
        ts_recv,
        flags: FlagSet::new(if bad { BAD_TS_RECV } else { 0 }),
        ..Mbp1Msg::default()
    }
}

#[expect(clippy::large_enum_variant, reason = "a handful of test records")]
enum R {
    C(Cmbp1Msg),
    M(Mbp1Msg),
    S(SystemMsg),
}

fn source(records: &[R], offset_ns: u64) -> Src {
    let mut out = Vec::new();
    DbnMetadataEncoder::new(&mut out).encode(&meta()).unwrap();
    {
        let mut enc = DbnRecordEncoder::new(&mut out);
        for r in records {
            match r {
                R::C(r) => enc.encode_record(r).unwrap(),
                R::M(r) => enc.encode_record(r).unwrap(),
                R::S(r) => enc.encode_record(r).unwrap(),
            }
        }
    }
    DbnSource::new(DbnDecoder::new(Cursor::new(out)).unwrap(), offset_ns)
}

/// Every delivery as (ts, rank, pos, instrument id).
fn drain(
    mut m: Merge<DbnDecoder<Cursor<Vec<u8>>>>,
) -> Result<Vec<(u64, u32, u64, u32)>, MergeError> {
    let mut v = Vec::new();
    while let Some(d) = m.next_delivery()? {
        assert_eq!(d.key.class, Class::Market);
        v.push((
            d.key.ts,
            d.key.rank,
            d.key.pos,
            d.record().header().instrument_id,
        ));
    }
    Ok(v)
}

#[test]
fn interleaves_by_receive_time() {
    let a = source(
        &[R::C(cmbp(1, 10)), R::C(cmbp(2, 30)), R::C(cmbp(3, 50))],
        0,
    );
    let b = source(&[R::C(cmbp(4, 20)), R::C(cmbp(5, 40))], 0);
    let got = drain(Merge::new(vec![a, b])).unwrap();
    let ids: Vec<u32> = got.iter().map(|g| g.3).collect();
    assert_eq!(ids, [1, 4, 2, 5, 3]);
    assert_eq!(got[1], (20, 1, 0, 4));
    assert_eq!(got[4], (50, 0, 2, 3));
}

#[test]
fn ties_keep_rank_then_file_order() {
    // Three records at t=10 in source 1, two in source 0, one in source 2.
    let a = source(&[R::C(cmbp(1, 10)), R::C(cmbp(2, 10))], 0);
    let b = source(
        &[R::C(cmbp(3, 10)), R::C(cmbp(4, 10)), R::C(cmbp(5, 10))],
        0,
    );
    let c = source(&[R::C(cmbp(6, 5)), R::C(cmbp(7, 10))], 0);
    let ids: Vec<u32> = drain(Merge::new(vec![a, b, c]))
        .unwrap()
        .iter()
        .map(|g| g.3)
        .collect();
    assert_eq!(ids, [6, 1, 2, 3, 4, 5, 7]);
}

#[test]
fn bad_receive_time_takes_event_time_plus_offset() {
    // IEX's ts_recv here is garbage (1); with a 7 ns offset its records sort
    // at ts_event + 7. A record without the flag keeps its ts_recv.
    let iex = source(
        &[
            R::M(mbp(1, 10, 1, true)),
            R::M(mbp(2, 20, 21, false)),
            R::M(mbp(3, 30, 1, true)),
        ],
        7,
    );
    let opra = source(
        &[R::C(cmbp(4, 17)), R::C(cmbp(5, 18)), R::C(cmbp(6, 37))],
        0,
    );
    let got = drain(Merge::new(vec![opra, iex])).unwrap();
    let order: Vec<(u64, u32)> = got.iter().map(|g| (g.0, g.3)).collect();
    // 17 ties: OPRA is rank 0, so its record goes first.
    assert_eq!(
        order,
        [(17, 4), (17, 1), (18, 5), (21, 2), (37, 6), (37, 3)]
    );
}

#[test]
fn system_messages_take_event_time() {
    let gap = SystemMsg::new(25, None, "store-gap").unwrap();
    let a = source(&[R::C(cmbp(1, 20)), R::S(gap), R::C(cmbp(2, 30))], 0);
    let b = source(&[R::C(cmbp(3, 26))], 0);
    let got = drain(Merge::new(vec![a, b])).unwrap();
    assert_eq!(
        got.iter().map(|g| g.0).collect::<Vec<_>>(),
        [20, 25, 26, 30]
    );
}

#[test]
fn empty_and_single_record_sources() {
    let empty = source(&[], 0);
    let one = source(&[R::C(cmbp(9, 3))], 0);
    let got = drain(Merge::new(vec![empty, one])).unwrap();
    assert_eq!(got, [(3, 1, 0, 9)]);
    assert!(drain(Merge::new(vec![source(&[], 0)])).unwrap().is_empty());
    assert!(drain(Merge::new(Vec::new())).unwrap().is_empty());
}

#[test]
fn a_source_going_back_in_time_fails() {
    let a = source(
        &[R::C(cmbp(1, 10)), R::C(cmbp(2, 30)), R::C(cmbp(3, 20))],
        0,
    );
    match drain(Merge::new(vec![a])) {
        Err(MergeError::OutOfOrder {
            rank,
            pos,
            ts,
            last,
        }) => {
            assert_eq!((rank, pos, ts, last), (0, 2, 20, 30));
        }
        other => panic!("expected OutOfOrder, got {:?}", other.map(|v| v.len())),
    }
}

#[test]
fn an_untimed_record_fails() {
    let a = source(&[R::C(cmbp(1, 10)), R::C(cmbp(2, UNDEF_TIMESTAMP))], 0);
    assert!(matches!(
        drain(Merge::new(vec![a])),
        Err(MergeError::Untimed { rank: 0, pos: 1 })
    ));
}

#[test]
fn peek_key_matches_the_next_delivery() {
    let a = source(&[R::C(cmbp(1, 10)), R::C(cmbp(2, 30))], 0);
    let b = source(&[R::C(cmbp(3, 20))], 0);
    let mut m = Merge::new(vec![a, b]);
    while let Some(k) = m.peek_key().unwrap() {
        let d = m.next_delivery().unwrap().unwrap();
        assert_eq!(k, d.key);
        assert_eq!(d.record().raw_index_ts(), k.ts);
    }
    assert!(m.next_delivery().unwrap().is_none());
}

/// Many generated sources: the output is in key order, every record comes out
/// once, each source's records keep their order, and a second merge of the
/// same inputs gives the same sequence.
#[test]
fn generated_sources_merge_in_key_order() {
    let mut seed: u64 = 0x5eed;
    let mut rand = move |n: u64| {
        seed = seed.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
        (seed >> 33) % n
    };
    let mut inputs = Vec::new();
    let mut total = 0;
    for s in 0..7_u32 {
        let (mut t, mut recs) = (0, Vec::new());
        for i in 0..rand(200) {
            t += rand(4); // steps of 0 give ties within a source
            recs.push((s * 1000 + u32::try_from(i).unwrap(), t));
        }
        total += recs.len();
        inputs.push(recs);
    }
    let build = || {
        Merge::new(
            inputs
                .iter()
                .map(|r| {
                    source(
                        &r.iter()
                            .map(|(id, t)| R::C(cmbp(*id, *t)))
                            .collect::<Vec<_>>(),
                        0,
                    )
                })
                .collect(),
        )
    };
    let got = drain(build()).unwrap();
    assert_eq!(got.len(), total);
    for w in got.windows(2) {
        assert!((w[0].0, w[0].1, w[0].2) < (w[1].0, w[1].1, w[1].2), "{w:?}");
    }
    for (rank, recs) in inputs.iter().enumerate() {
        let mine: Vec<u32> = got
            .iter()
            .filter(|g| g.1 as usize == rank)
            .map(|g| g.3)
            .collect();
        assert_eq!(mine, recs.iter().map(|r| r.0).collect::<Vec<_>>());
    }
    assert_eq!(got, drain(build()).unwrap());
}

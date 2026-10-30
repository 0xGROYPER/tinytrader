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
use tinytrader_dbn::{Class, DbnSource, Merge, MergeError, TradeConditions, TradeUpdate};

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

/// A source of `before`, then raw record bytes, then `after`.
fn source_with(before: &[R], raw: &[Vec<u8>], after: &[R], offset_ns: u64) -> Src {
    let mut out = Vec::new();
    DbnMetadataEncoder::new(&mut out).encode(&meta()).unwrap();
    {
        let mut enc = DbnRecordEncoder::new(&mut out);
        for r in before {
            match r {
                R::C(r) => enc.encode_record(r).unwrap(),
                R::M(r) => enc.encode_record(r).unwrap(),
                R::S(r) => enc.encode_record(r).unwrap(),
            }
        }
    }
    for b in raw {
        out.extend(b);
    }
    {
        let mut enc = DbnRecordEncoder::new(&mut out);
        for r in after {
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
fn an_untimed_first_record_fails() {
    let a = source(&[R::C(cmbp(2, UNDEF_TIMESTAMP)), R::C(cmbp(1, 10))], 0);
    assert!(matches!(
        drain(Merge::new(vec![a])),
        Err(MergeError::Untimed { rank: 0, pos: 0 })
    ));
}

/// An untimed record and an extension record (trade conditions, an rtype `dbn`
/// doesn't know, whose `ts_event` is its trade's, earlier than the trade's
/// `ts_recv`) take the time of the record before them and follow it directly,
/// though another source has a record at that same time with a lower rank.
#[test]
fn companion_records_stay_with_their_record() {
    let cond = TradeConditions {
        publisher_id: 2,
        instrument_id: 7,
        ts_event: 15,
        price: 1_000_000_000,
        trade_id: 1,
        size: 100,
        codes: [b'@', 0, 0, 0],
        count: 1,
        tape: b'C',
        update: TradeUpdate::None,
        trade_seq: 1,
    };
    let b = source_with(
        &[R::C(cmbp(7, 20))],
        &[cond.encode().unwrap().to_vec()],
        &[R::C(cmbp(8, UNDEF_TIMESTAMP)), R::C(cmbp(9, 40))],
        0,
    );
    let a = source(
        &[R::C(cmbp(1, 10)), R::C(cmbp(2, 20)), R::C(cmbp(3, 30))],
        0,
    );
    let mut m = Merge::new(vec![a, b]);
    let mut got = Vec::new();
    while let Some(d) = m.next_delivery().unwrap() {
        got.push((d.key.ts, d.key.rank, d.record().header().rtype));
    }
    let c = tinytrader_dbn::trade_conditions::RTYPE;
    let q = rtype::CMBP_1;
    assert_eq!(
        got,
        [
            (10, 0, q),
            (20, 0, q),
            (20, 1, q),
            (20, 1, c),
            (20, 1, q),
            (30, 0, q),
            (40, 1, q)
        ]
    );
}

/// A flagged source whose `ts_event` steps back (reordered packets) keeps its
/// file order, clamped to the time before, and counts it.
#[test]
fn a_flagged_record_stepping_back_is_clamped() {
    let iex = source(
        &[
            R::M(mbp(1, 30, 1, true)),
            R::M(mbp(2, 25, 1, true)),
            R::M(mbp(3, 40, 1, true)),
        ],
        0,
    );
    let mut m = Merge::new(vec![iex]);
    let mut got = Vec::new();
    while let Some(d) = m.next_delivery().unwrap() {
        got.push((d.key.ts, d.record().header().instrument_id));
    }
    assert_eq!(got, [(30, 1), (30, 2), (40, 3)]);
    assert_eq!(m.sources()[0].clamped(), 1);
}

#[test]
fn an_offset_that_overflows_is_its_own_error() {
    let iex = source(&[R::M(mbp(1, u64::MAX - 3, 1, true))], 10);
    assert!(matches!(
        drain(Merge::new(vec![iex])),
        Err(MergeError::BadRecord {
            rank: 0,
            pos: 0,
            ..
        })
    ));
}

/// A read-ahead that fails mid-merge delivers the record already taken, then
/// returns the error on every later call, never carrying on without the source.
#[test]
fn errors_mid_merge_are_sticky_and_lose_nothing() {
    let a = source(
        &[R::C(cmbp(1, 10)), R::C(cmbp(2, 30)), R::C(cmbp(3, 20))],
        0,
    );
    let b = source(&[R::C(cmbp(4, 15)), R::C(cmbp(5, 35))], 0);
    let mut m = Merge::new(vec![a, b]);
    let mut ids = Vec::new();
    let err = loop {
        match m.next_delivery() {
            Ok(Some(d)) => ids.push(d.record().header().instrument_id),
            Ok(None) => panic!("ended without the error"),
            Err(e) => break e,
        }
    };
    // 1 at 10, 4 at 15, then 2 at 30 is taken and its read-ahead fails on 3.
    assert_eq!(ids, [1, 4, 2]);
    assert!(matches!(
        err,
        MergeError::OutOfOrder {
            rank: 0,
            pos: 2,
            ..
        }
    ));
    for _ in 0..3 {
        assert_eq!(m.next_delivery().err(), Some(err.clone()));
        assert_eq!(m.peek_key().err(), Some(err.clone()));
    }
}

#[test]
fn errors_while_priming_are_sticky() {
    let a = source(&[R::C(cmbp(1, 10))], 0);
    let b = source(&[R::C(cmbp(2, UNDEF_TIMESTAMP))], 0);
    let mut m = Merge::new(vec![a, b]);
    let e = m.peek_key().unwrap_err();
    assert!(matches!(e, MergeError::Untimed { rank: 1, pos: 0 }));
    for _ in 0..3 {
        assert_eq!(m.next_delivery().err(), Some(e.clone()));
    }
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

//! The purchase paths (decision 0019) against a fake Databento (job list,
//! job details, cost, submit, with delays and failures) and a fake store
//! (`/catalog`, `/coverage`, `/files`) on loopback. Nothing here reaches the
//! network or a real disk outside a temporary folder.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::atomic::AtomicUsize;

use axum::extract::RawQuery;
use databento::historical::batch::{BatchFileDesc, BatchJobShort, ListJobsParams};
use databento::historical::metadata::{
    DatasetConditionDetail, DatasetRange, GetDatasetConditionParams,
};

use super::*;
use crate::api::Fut;

const DS: &str = "OPRA.PILLAR";
const SCHEMA: &str = "cmbp-1";
const DAY: &str = "2026-10-02";
const C1: &str = "SPY   261002C00728000";
const C2: &str = "SPY   261002P00728000";
const HOUR: i64 = 3_600_000_000_000;

fn date(s: &str) -> time::Date {
    time::Date::parse(s, time::macros::format_description!("[year]-[month]-[day]")).unwrap()
}

fn iso(ns: i64) -> String {
    let t = had::at(ns);
    format!(
        "{}T{:02}:{:02}:{:02}.{:09}Z",
        t.date(),
        t.hour(),
        t.minute(),
        t.second(),
        t.nanosecond()
    )
}

/// A job as Databento's job details describe it.
fn job(id: &str, symbols: Value, stype_in: &str, w: had::Window, state: &str) -> BatchJob {
    let now = had::ns(time::OffsetDateTime::now_utc());
    serde_json::from_value(json!({
        "id": id, "user_id": null, "cost_usd": 0.0, "dataset": DS, "symbols": symbols,
        "stype_in": stype_in, "stype_out": "instrument_id", "schema": SCHEMA,
        "start": iso(w.0), "end": iso(w.1), "limit": null, "encoding": "dbn", "compression": "zstd",
        "pretty_px": false, "pretty_ts": false, "map_symbols": false, "split_symbols": false,
        "split_duration": "day", "split_size": null, "delivery": "download", "record_count": null,
        "billed_size": null, "actual_size": null, "package_size": null, "state": state,
        "ts_received": iso(now), "ts_queued": null, "ts_process_start": null,
        "ts_process_done": null, "ts_expiration": null, "progress": null,
    }))
    .unwrap()
}

#[derive(Default)]
struct FakeState {
    jobs: Vec<BatchJob>,
    /// Jobs Databento made but doesn't list yet.
    hidden: HashSet<String>,
    unreadable: HashSet<String>,
    list_fails: bool,
    price: f64,
    /// Each price waits (up to 300 ms) until this many have been asked
    /// for: two purchases checked at once both get this far.
    cost_gate: usize,
    costs_asked: usize,
    /// Databento makes the job but the answer is lost.
    submit_lost: bool,
    /// Databento doesn't answer a submission at all.
    submit_hangs: bool,
    costed: Vec<(Vec<String>, had::Window)>,
    submitted: Vec<SubmitJobParams>,
    listed_with: Vec<ListJobsParams>,
}

#[derive(Default)]
struct Fake {
    s: Mutex<FakeState>,
    next: AtomicUsize,
}

impl Fake {
    fn add(&self, j: BatchJob) {
        lock(&self.s).jobs.push(j);
    }
    fn submits(&self) -> usize {
        lock(&self.s).submitted.len()
    }
}

fn not_faked<'a, T: Send + 'a>() -> Fut<'a, T> {
    Box::pin(async { Err("not faked".to_owned()) })
}

impl api::Databento for Fake {
    fn list_jobs<'a>(&'a self, p: &'a ListJobsParams) -> Fut<'a, Vec<BatchJobShort>> {
        Box::pin(async move {
            let mut s = lock(&self.s);
            s.listed_with.push(p.clone());
            if s.list_fails {
                return Err("503 Service Unavailable".into());
            }
            // As Databento: without states, expired jobs aren't listed.
            let states = p
                .states
                .clone()
                .unwrap_or_else(|| vec![JobState::Queued, JobState::Processing, JobState::Done]);
            Ok(s.jobs
                .iter()
                .filter(|j| !s.hidden.contains(&j.id) && states.contains(&j.state))
                .map(|j| BatchJobShort {
                    id: j.id.clone(),
                    state: j.state,
                    ts_received: j.ts_received,
                })
                .collect())
        })
    }
    fn job<'a>(&'a self, id: &'a str) -> Fut<'a, BatchJob> {
        Box::pin(async move {
            let s = lock(&self.s);
            if s.unreadable.contains(id) {
                return Err("timed out".into());
            }
            s.jobs
                .iter()
                .find(|j| j.id == id)
                .cloned()
                .ok_or_else(|| "no such job".into())
        })
    }
    fn files<'a>(&'a self, _: &'a str) -> Fut<'a, Vec<BatchFileDesc>> {
        not_faked()
    }
    fn submit<'a>(&'a self, p: &'a SubmitJobParams) -> Fut<'a, BatchJob> {
        Box::pin(async move {
            if lock(&self.s).submit_hangs {
                tokio::time::sleep(Duration::from_secs(3600)).await;
            }
            let id = format!("OPRA-NEW{}", self.next.fetch_add(1, Ordering::Relaxed));
            let mut j = job(
                &id,
                json!(p.symbols.to_api_string()),
                p.stype_in.as_str(),
                (
                    had::ns(p.date_time_range.start),
                    had::ns(p.date_time_range.end),
                ),
                "queued",
            );
            j.dataset.clone_from(&p.dataset);
            let mut s = lock(&self.s);
            s.submitted.push(p.clone());
            s.jobs.push(j.clone());
            if s.submit_lost {
                s.hidden.insert(id);
                return Err("connection reset".into());
            }
            Ok(j)
        })
    }
    fn cost<'a>(&'a self, p: &'a GetCostParams) -> Fut<'a, (f64, u64)> {
        Box::pin(async move {
            let gate = {
                let mut s = lock(&self.s);
                s.costs_asked += 1;
                s.cost_gate
            };
            for _ in 0..60 {
                if lock(&self.s).costs_asked >= gate {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
            let mut s = lock(&self.s);
            let w = (
                had::ns(p.date_time_range.start),
                had::ns(p.date_time_range.end),
            );
            let symbols = p
                .symbols
                .to_api_string()
                .split(',')
                .map(str::to_owned)
                .collect();
            s.costed.push((symbols, w));
            Ok((s.price, 1000))
        })
    }
    fn datasets(&self) -> Fut<'_, Vec<String>> {
        Box::pin(async { Ok(vec![DS.to_owned(), "XNAS.ITCH".to_owned()]) })
    }
    fn schemas<'a>(&'a self, _: &'a str) -> Fut<'a, Vec<Schema>> {
        not_faked()
    }
    fn range<'a>(&'a self, _: &'a str) -> Fut<'a, DatasetRange> {
        not_faked()
    }
    fn conditions<'a>(
        &'a self,
        _: &'a GetDatasetConditionParams,
    ) -> Fut<'a, Vec<DatasetConditionDetail>> {
        not_faked()
    }
}

/// What the fake store answers.
struct StoreState {
    catalog: (u16, Value),
    coverage: (u16, Value),
    coverage_asked: Vec<String>,
    files_asked: Vec<String>,
    definitions: Vec<u8>,
}

/// A definition day (zstd DBN) listing these contracts, all expiring on `DAY`.
fn definitions(symbols: &[&str]) -> Vec<u8> {
    use dbn::encode::EncodeRecord as _;
    let meta = dbn::Metadata::builder()
        .dataset(DS)
        .schema(Some(dbn::Schema::Definition))
        .start(0)
        .stype_in(Some(dbn::SType::RawSymbol))
        .stype_out(dbn::SType::InstrumentId)
        .build();
    let expiration = u64::try_from(whole_day().0 + 20 * HOUR).unwrap();
    let mut out = Vec::new();
    {
        let mut e = dbn::encode::dbn::Encoder::with_zstd(&mut out, &meta).unwrap();
        for (i, s) in symbols.iter().enumerate() {
            e.encode_record(&dbn::InstrumentDefMsg {
                hd: dbn::RecordHeader::new::<dbn::InstrumentDefMsg>(
                    dbn::rtype::INSTRUMENT_DEF,
                    1,
                    u32::try_from(i).unwrap() + 1,
                    0,
                ),
                raw_symbol: dbn::record::str_to_c_chars(s).unwrap(),
                expiration,
                strike_price: 728_000_000_000,
                ..Default::default()
            })
            .unwrap();
        }
    }
    out
}

fn ready_store() -> StoreState {
    StoreState {
        catalog: (200, json!({ "roots": [], "missing_volumes": [] })),
        coverage: (200, json!({ "days": [], "missing_volumes": [] })),
        coverage_asked: Vec::new(),
        files_asked: Vec::new(),
        definitions: definitions(&[C1, C2]),
    }
}

type StoreHandle = Arc<Mutex<StoreState>>;

fn answer((code, v): (u16, Value)) -> Reply {
    (StatusCode::from_u16(code).unwrap(), Json(v))
}

async fn serve_store(state: StoreHandle) -> String {
    let app = Router::new()
        .route(
            "/catalog",
            get(|AxState(s): AxState<StoreHandle>| async move { answer(lock(&s).catalog.clone()) }),
        )
        .route(
            "/coverage",
            get(|AxState(s): AxState<StoreHandle>, RawQuery(q): RawQuery| async move {
                let mut s = lock(&s);
                s.coverage_asked.push(q.unwrap_or_default());
                answer(s.coverage.clone())
            }),
        )
        .route(
            "/files",
            get(|AxState(s): AxState<StoreHandle>, RawQuery(q): RawQuery| async move {
                lock(&s).files_asked.push(q.unwrap_or_default());
                Json(json!({ "files": [{ "volume": "v1", "path": format!("{DS}/SPY.OPT/definition/{DAY}.dbn.zst") }] }))
            }),
        )
        .route(
            "/files/v1/{*path}",
            get(|AxState(s): AxState<StoreHandle>| async move { lock(&s).definitions.clone() }),
        )
        .with_state(state);
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(l, app).await });
    format!("http://{addr}")
}

struct Rig {
    jobs: Shared,
    fake: Arc<Fake>,
    store: StoreHandle,
    dir: PathBuf,
}

impl Drop for Rig {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

async fn rig(name: &str) -> Rig {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let dir = std::env::temp_dir().join(format!("tinytrader-jobs-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let store = Arc::new(Mutex::new(ready_store()));
    let url = serve_store(store.clone()).await;
    let fake = Arc::new(Fake::default());
    let ledger = ledger::Ledger::open(&dir.join("submissions.jsonl")).unwrap();
    let mut jobs = Jobs::new(fake.clone(), "key".into(), &url, None, Some(ledger));
    // Short limits on Databento's answers, to keep the tests fast.
    jobs.db = Arc::new(api::Timed {
        inner: fake.clone(),
        read: Duration::from_secs(2),
        submit: Duration::from_millis(300),
    });
    let jobs = Arc::new(jobs);
    Rig {
        jobs,
        fake,
        store,
        dir,
    }
}

impl Rig {
    /// A quote for a whole parent, as `/price` would leave it.
    fn quote(&self, symbol: &str, start: &str, end: &str, cost_usd: f64) -> String {
        let id = format!("q{}", self.jobs.next_quote.fetch_add(1, Ordering::Relaxed));
        let request = Request {
            dataset: DS.to_lowercase(),
            schema: SCHEMA.into(),
            symbols: symbol.into(),
            stype_in: "parent".into(),
            start: start.into(),
            end: end.into(),
        };
        lock(&self.jobs.quotes).insert(
            id.clone(),
            Quote {
                request,
                cost_usd,
                at: Instant::now(),
            },
        );
        id
    }

    async fn submit(&self, quote: &str, again: Option<Value>) -> (StatusCode, Value) {
        let (code, Json(v)) = submit(
            AxState(self.jobs.clone()),
            Json(Submit {
                quote: quote.into(),
                confirm_usd: None,
                again,
            }),
        )
        .await;
        (code, v)
    }

    async fn price_missing(&self) -> (StatusCode, Value) {
        let (code, Json(v)) = price_missing(
            AxState(self.jobs.clone()),
            Json(MissingRequest {
                dataset: DS.into(),
                schema: SCHEMA.into(),
                symbol: "spy.opt".into(),
                start: DAY.into(),
                end: DAY.into(),
                rule: missing::Rule::default(),
            }),
        )
        .await;
        (code, v)
    }

    async fn submit_missing(&self, quote: &str) -> (StatusCode, Value) {
        let (code, Json(v)) = submit_missing(
            AxState(self.jobs.clone()),
            Json(SubmitMissing {
                quote: quote.into(),
                confirm_usd: None,
            }),
        )
        .await;
        (code, v)
    }

    fn write(&self, request: &str, state: &str, job: Option<&str>, at: f64) {
        lock(&self.jobs.ledger)
            .as_mut()
            .unwrap()
            .append(ledger::Entry {
                at,
                request: request.into(),
                state: state.into(),
                job: job.map(Into::into),
                cost_usd: Some(0.0),
                note: None,
            })
            .unwrap();
    }

    fn states(&self) -> Vec<String> {
        let text = std::fs::read_to_string(self.dir.join("submissions.jsonl")).unwrap_or_default();
        text.lines()
            .map(|l| {
                serde_json::from_str::<Value>(l).unwrap()["state"]
                    .as_str()
                    .unwrap()
                    .to_owned()
            })
            .collect()
    }
}

fn whole_day() -> had::Window {
    had::day_window(date(DAY))
}

fn fp(symbols: &[&str]) -> String {
    let s: Vec<String> = symbols.iter().map(|x| (*x).to_owned()).collect();
    ledger::fingerprint(DS, SCHEMA, &s, DAY, "2026-10-03")
}

/// The requests a missing-quote prices: each one's window and contract count.
fn requests(v: &Value) -> Vec<(Value, u64)> {
    v["days"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| (d["window"].clone(), d["contracts"].as_u64().unwrap()))
        .collect()
}

#[tokio::test]
async fn two_concurrent_submits_buy_once() {
    let r = rig("concurrent").await;
    // Both are checked before either is sent, unless one waits its turn.
    lock(&r.fake.s).cost_gate = 2;
    // The same request twice, then two that overlap by a day.
    let (a, b) = (
        r.quote("SPY.OPT", DAY, DAY, 0.0),
        r.quote("SPY.OPT", DAY, DAY, 0.0),
    );
    let ((ca, _), (cb, _)) = tokio::join!(r.submit(&a, None), r.submit(&b, None));
    assert_eq!(r.fake.submits(), 1, "{ca} {cb}");
    let (c, d) = (
        r.quote("QQQ.OPT", "2026-10-01", DAY, 0.0),
        r.quote("QQQ.OPT", DAY, "2026-10-05", 0.0),
    );
    lock(&r.fake.s).cost_gate = 4;
    let ((cc, _), (cd, _)) = tokio::join!(r.submit(&c, None), r.submit(&d, None));
    assert_eq!(r.fake.submits(), 2, "{cc} {cd}");
    assert!([cc, cd].contains(&StatusCode::CONFLICT));
}

#[tokio::test]
async fn a_submission_lost_after_databento_accepted_it_is_never_bought_again() {
    let r = rig("lost").await;
    lock(&r.fake.s).submit_lost = true;
    let (code, _) = r.submit(&r.quote("SPY.OPT", DAY, DAY, 0.0), None).await;
    assert_eq!(code, StatusCode::BAD_GATEWAY);
    // It doesn't show on Databento's list yet: still never bought again.
    lock(&r.fake.s).submit_lost = false;
    let (code, v) = r.submit(&r.quote("SPY.OPT", DAY, DAY, 0.0), None).await;
    assert_eq!(code, StatusCode::CONFLICT, "{v}");
    refresh(&r.jobs).await.unwrap();
    assert_eq!(r.states(), ["sending", "uncertain"]);
    // It shows: reconciled to its job, and still never bought again.
    lock(&r.fake.s).hidden.clear();
    refresh(&r.jobs).await.unwrap();
    assert_eq!(r.states(), ["sending", "uncertain", "submitted"]);
    let (code, _) = r.submit(&r.quote("SPY.OPT", DAY, DAY, 0.0), None).await;
    assert_eq!(code, StatusCode::CONFLICT);
    assert_eq!(r.fake.submits(), 1);
    // The list asked for is complete: every state, all history.
    let asked = lock(&r.fake.s).listed_with.clone();
    assert!(asked.iter().all(|p| {
        p.since.is_some()
            && p.states
                .as_ref()
                .is_some_and(|s| s.contains(&JobState::Expired))
    }));
}

#[tokio::test]
async fn an_old_unresolved_submission_is_never_written_off() {
    let r = rig("old").await;
    r.write(&fp(&["SPY.OPT"]), "uncertain", None, 1.0);
    refresh(&r.jobs).await.unwrap();
    assert_eq!(r.states(), ["uncertain"]);
    let (code, _) = r.submit(&r.quote("SPY.OPT", DAY, DAY, 0.0), None).await;
    assert_eq!(code, StatusCode::CONFLICT);
    // Until the operator says it was never made.
    let (code, Json(v)) = resolve(
        AxState(r.jobs.clone()),
        Json(Resolve {
            request: fp(&["SPY.OPT"]),
            at: 1.0,
            job: None,
            note: "not in Databento's portal".into(),
        }),
    )
    .await;
    assert_eq!(code, StatusCode::OK, "{v}");
    let (code, v) = r.submit(&r.quote("SPY.OPT", DAY, DAY, 0.0), None).await;
    assert_eq!(code, StatusCode::OK, "{v}");
}

#[tokio::test]
async fn multi_symbol_all_symbols_contract_list_and_instrument_id_jobs_block() {
    for (n, (symbols, stype, left)) in [
        (json!("SPY.OPT,QQQ.OPT"), "parent", 0),
        (json!(["QQQ.OPT", "SPY.OPT"]), "parent", 0),
        (json!("ALL_SYMBOLS"), "parent", 0),
        (json!(C1), "raw_symbol", 1),
        (json!([123, 456]), "instrument_id", 0),
    ]
    .into_iter()
    .enumerate()
    {
        let r = rig(&format!("symbols-{n}")).await;
        r.fake
            .add(job("OPRA-OLD", symbols.clone(), stype, whole_day(), "done"));
        let (code, v) = r.submit(&r.quote("SPY.OPT", DAY, DAY, 0.0), None).await;
        assert_eq!(code, StatusCode::CONFLICT, "{symbols}: {v}");
        let (code, v) = r.price_missing().await;
        assert_eq!(code, StatusCode::OK, "{v}");
        let left_now: u64 = requests(&v).iter().map(|(_, n)| n).sum();
        assert_eq!(left_now, left, "{symbols}: {v}");
        assert_eq!(r.fake.submits(), 0);
    }
}

#[tokio::test]
async fn submit_missing_subtracts_ledger_purchases_listed_or_not() {
    let r = rig("ledger").await;
    // Bought here, its job expired and gone from the list.
    r.write(&fp(&[C1]), "submitted", Some("OPRA-GONE"), 1.0);
    let (_, v) = r.price_missing().await;
    assert_eq!(requests(&v), [(json!([DAY, "2026-10-03"]), 1)]);
    // And one whose outcome isn't known.
    r.write(&fp(&[C2]), "uncertain", None, 2.0);
    let (_, v) = r.price_missing().await;
    assert!(requests(&v).is_empty(), "{v}");
}

#[tokio::test]
async fn buying_again_names_exactly_the_expired_jobs() {
    let r = rig("again").await;
    r.write(&fp(&["SPY.OPT"]), "submitted", Some("OPRA-GONE"), 1.0);
    let (code, v) = r.submit(&r.quote("SPY.OPT", DAY, DAY, 0.0), None).await;
    assert_eq!(code, StatusCode::CONFLICT, "{v}");
    assert!(v["error"].as_str().unwrap().contains("[\"OPRA-GONE\"]"));
    let q = r.quote("SPY.OPT", DAY, DAY, 0.0);
    assert_eq!(
        r.submit(&q, Some(json!(true))).await.0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        r.submit(&q, Some(json!(["OPRA-OTHER"]))).await.0,
        StatusCode::CONFLICT
    );
    let q = r.quote("SPY.OPT", DAY, DAY, 0.0);
    let (code, _) = r.submit(&q, Some(json!(["OPRA-GONE", "OPRA-OTHER"]))).await;
    assert_eq!(code, StatusCode::BAD_REQUEST);
    assert_eq!(r.fake.submits(), 0);
    let q = r.quote("SPY.OPT", DAY, DAY, 0.0);
    let (code, v) = r.submit(&q, Some(json!(["OPRA-GONE"]))).await;
    assert_eq!(code, StatusCode::OK, "{v}");
    // A job still on the list is never bought again, named or not.
    let q = r.quote("SPY.OPT", DAY, DAY, 0.0);
    let id = v["job"].as_str().unwrap();
    assert_eq!(
        r.submit(&q, Some(json!([id]))).await.0,
        StatusCode::CONFLICT
    );
    assert_eq!(r.fake.submits(), 1);
}

#[tokio::test]
async fn a_store_that_cant_say_what_it_holds_refuses_every_purchase() {
    let ready = || {
        (
            200,
            json!({ "days": [], "roots": [], "missing_volumes": [] }),
        )
    };
    for (n, (catalog, coverage)) in [
        ((503, json!({ "error": "first scan" })), ready()),
        (
            (200, json!({ "roots": [], "missing_volumes": ["M2-B"] })),
            ready(),
        ),
        (ready(), (200, json!({ "days": [] }))),
        (ready(), (503, json!({}))),
    ]
    .into_iter()
    .enumerate()
    {
        let r = rig(&format!("store-{n}")).await;
        {
            let mut s = lock(&r.store);
            s.catalog = catalog;
            s.coverage = coverage;
        }
        let (code, v) = r.submit(&r.quote("SPY.OPT", DAY, DAY, 0.0), None).await;
        assert_eq!(code, StatusCode::SERVICE_UNAVAILABLE, "{n}: {v}");
        assert_eq!(r.price_missing().await.0, StatusCode::SERVICE_UNAVAILABLE);
    }
    // Nor while Databento's job list can't be read.
    let r = rig("store-list").await;
    lock(&r.fake.s).list_fails = true;
    let (code, _) = r.submit(&r.quote("SPY.OPT", DAY, DAY, 0.0), None).await;
    assert_eq!(code, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(r.fake.submits(), 0);
}

#[tokio::test]
async fn a_catalog_day_coverage_doesnt_explain_is_refused() {
    let dataset = |provider: &str| json!({ "provider": provider, "filed": true, "id": DS, "symbol": "SPY.OPT", "schemas": { SCHEMA: { "days": { DAY: {} } } } });
    // Another provider's holdings don't count for Databento's.
    let r = rig("unexplained").await;
    lock(&r.store).catalog.1 =
        json!({ "roots": [{ "datasets": [dataset("alpaca")] }], "missing_volumes": [] });
    let (code, v) = r.submit(&r.quote("SPY.OPT", DAY, DAY, 0.0), None).await;
    assert_eq!(code, StatusCode::OK, "{v}");
    let r = rig("unexplained-2").await;
    lock(&r.store).catalog.1 =
        json!({ "roots": [{ "datasets": [dataset("databento")] }], "missing_volumes": [] });
    let (code, v) = r.submit(&r.quote("SPY.OPT", DAY, DAY, 0.0), None).await;
    assert_eq!(code, StatusCode::CONFLICT, "{v}");
    let (_, v) = r.price_missing().await;
    assert!(requests(&v).is_empty(), "{v}");
    let s = lock(&r.store);
    assert!(!s.coverage_asked.is_empty() && !s.files_asked.is_empty());
    assert!(
        s.coverage_asked
            .iter()
            .chain(&s.files_asked)
            .all(|q| q.contains("provider=databento"))
    );
}

#[tokio::test]
async fn an_unread_job_during_reconcile_resolves_nothing() {
    let r = rig("unread").await;
    let now = tinytrader_observe::unix_seconds();
    r.write(&fp(&["SPY.OPT"]), "uncertain", None, now);
    r.fake.add(job(
        "OPRA-MATCH",
        json!("SPY.OPT"),
        "parent",
        whole_day(),
        "queued",
    ));
    r.fake.add(job(
        "OPRA-SLOW",
        json!("QQQ.OPT"),
        "parent",
        whole_day(),
        "queued",
    ));
    lock(&r.fake.s).unreadable.insert("OPRA-SLOW".into());
    refresh(&r.jobs).await.unwrap();
    assert_eq!(r.states(), ["uncertain"]);
    lock(&r.fake.s).unreadable.clear();
    refresh(&r.jobs).await.unwrap();
    assert_eq!(r.states(), ["uncertain", "submitted"]);
}

#[tokio::test]
async fn the_missing_hours_of_a_partial_day_are_bought_as_their_own_request() {
    let r = rig("hours").await;
    let day = whole_day();
    lock(&r.store).coverage.1 = json!({ "days": [
        { "day": DAY, "kind": "partial", "filed": true, "volumes": ["v1"], "coverage": { "kind": "contracts", "stype": "raw_symbol", "symbols": [C1], "window": [day.0 + 13 * HOUR, day.1] } },
    ], "missing_volumes": [] });
    let (code, v) = r.price_missing().await;
    assert_eq!(code, StatusCode::OK, "{v}");
    assert_eq!(
        requests(&v),
        [
            (json!([DAY, "2026-10-03"]), 1),
            (json!([DAY, "2026-10-02T13:00:00.000000000Z"]), 1)
        ]
    );
    let hours = (vec![C1.to_owned()], (day.0, day.0 + 13 * HOUR));
    assert!(lock(&r.fake.s).costed.contains(&hours));
    let (code, v) = r.submit_missing(v["quote"].as_str().unwrap()).await;
    assert_eq!(code, StatusCode::OK, "{v}");
    assert_eq!(r.fake.submits(), 2);
    // Both now pending: nothing left to buy.
    let (_, v) = r.price_missing().await;
    assert!(requests(&v).is_empty(), "{v}");
}

#[tokio::test]
async fn a_price_that_isnt_a_number_of_dollars_is_refused() {
    for price in [f64::NAN, -1.0] {
        let r = rig("nan").await;
        lock(&r.fake.s).price = price;
        let (code, v) = r.submit(&r.quote("SPY.OPT", DAY, DAY, price), None).await;
        assert_eq!(code, StatusCode::BAD_GATEWAY, "{v}");
        let (_, v) = r.price_missing().await;
        let (code, v) = r.submit_missing(v["quote"].as_str().unwrap()).await;
        assert_eq!(code, StatusCode::BAD_GATEWAY, "{v}");
        assert_eq!(r.fake.submits(), 0);
    }
}

#[tokio::test]
async fn names_are_canonical_and_datasets_known() {
    let r = rig("names").await;
    let q = r.quote("spy.opt", DAY, DAY, 0.0);
    let (code, v) = r.submit(&q, None).await;
    assert_eq!(code, StatusCode::OK, "{v}");
    let sent = lock(&r.fake.s).submitted[0].clone();
    assert_eq!(
        (sent.dataset.as_str(), sent.symbols.to_api_string()),
        (DS, "SPY.OPT".into())
    );
    let q = r.quote("SPY.OPT", "2026-10-05", "2026-10-05", 0.0);
    lock(&r.jobs.quotes).get_mut(&q).unwrap().request.dataset = "OPRA.NOPE".into();
    assert_eq!(r.submit(&q, None).await.0, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn a_hung_submission_is_uncertain_and_frees_the_lock() {
    let r = rig("hung").await;
    lock(&r.fake.s).submit_hangs = true;
    let both = async {
        let first = r.submit(&r.quote("SPY.OPT", DAY, DAY, 0.0), None).await.0;
        let second = r.submit(&r.quote("SPY.OPT", DAY, DAY, 0.0), None).await.0;
        (first, second)
    };
    let (first, second) = tokio::time::timeout(Duration::from_secs(5), both)
        .await
        .expect("a hung submission must not hold the purchase lock");
    assert_eq!(
        (first, second),
        (StatusCode::BAD_GATEWAY, StatusCode::CONFLICT)
    );
    assert_eq!(r.states(), ["sending", "uncertain"]);
}

#[tokio::test]
async fn reconcile_matches_jobs_from_when_the_submission_was_sent() {
    let r = rig("sent-at").await;
    let now = tinytrader_observe::unix_seconds();
    r.write(&fp(&["SPY.OPT"]), "sending", None, now - 1000.0);
    r.write(&fp(&["SPY.OPT"]), "uncertain", None, now);
    let mut j = job(
        "OPRA-MATCH",
        json!("SPY.OPT"),
        "parent",
        whole_day(),
        "queued",
    );
    j.ts_received -= time::Duration::seconds(990);
    r.fake.add(j);
    refresh(&r.jobs).await.unwrap();
    assert_eq!(r.states(), ["sending", "uncertain", "submitted"]);
}

#[tokio::test]
async fn reconcile_waits_while_a_purchase_is_in_flight() {
    let r = rig("in-flight").await;
    let now = tinytrader_observe::unix_seconds();
    r.write(&fp(&["SPY.OPT"]), "sending", None, now);
    r.fake.add(job(
        "OPRA-MATCH",
        json!("SPY.OPT"),
        "parent",
        whole_day(),
        "queued",
    ));
    {
        let _purchase = r.jobs.purchase.lock().await;
        refresh(&r.jobs).await.unwrap();
        assert_eq!(r.states(), ["sending"]);
    }
    refresh(&r.jobs).await.unwrap();
    assert_eq!(r.states(), ["sending", "submitted"]);
}

#[tokio::test]
async fn an_old_automatic_write_off_blocks_until_matched() {
    let r = rig("written-off").await;
    let now = tinytrader_observe::unix_seconds();
    r.write(&fp(&["SPY.OPT"]), "sending", None, now - 900.0);
    let mut off = ledger::Entry {
        at: now - 100.0,
        request: fp(&["SPY.OPT"]),
        state: "not-submitted".into(),
        job: None,
        cost_usd: None,
        note: Some("no such job appeared in Databento's job list".into()),
    };
    lock(&r.jobs.ledger)
        .as_mut()
        .unwrap()
        .append(off.clone())
        .unwrap();
    let (code, _) = r.submit(&r.quote("SPY.OPT", DAY, DAY, 0.0), None).await;
    assert_eq!(code, StatusCode::CONFLICT);
    // An operator's write-off doesn't block.
    off.note = Some("resolved by the operator: checked".into());
    lock(&r.jobs.ledger).as_mut().unwrap().append(off).unwrap();
    let (code, v) = r.submit(&r.quote("SPY.OPT", DAY, DAY, 0.0), None).await;
    assert_eq!(code, StatusCode::OK, "{v}");
}

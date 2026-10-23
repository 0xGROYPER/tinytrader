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
    /// Databento doesn't answer a submission naming this symbol at all.
    hang_on: Option<String>,
    costed: Vec<(Vec<String>, had::Window)>,
    /// Errors the next prices answer with, in turn.
    cost_errors: std::collections::VecDeque<String>,
    /// Prices the next requests get, in turn, before `price`.
    prices: std::collections::VecDeque<f64>,
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
            let hang = lock(&self.s).hang_on.clone();
            if hang.is_some_and(|h| p.symbols.to_api_string().contains(&h)) {
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
            match s.cost_errors.pop_front() {
                Some(e) => Err(e),
                None => {
                    let p = s.prices.pop_front().unwrap_or(s.price);
                    Ok((p, 1000))
                }
            }
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
    catalog_asked: Vec<String>,
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
        catalog_asked: Vec::new(),
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
            get(|AxState(s): AxState<StoreHandle>, RawQuery(q): RawQuery| async move {
                let mut s = lock(&s);
                s.catalog_asked.push(q.unwrap_or_default());
                answer(s.catalog.clone())
            }),
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
    rig_with(name, |_| {}).await
}

/// A rig whose service is changed by `setup` before it starts.
async fn rig_with(name: &str, setup: impl FnOnce(&mut Jobs)) -> Rig {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let dir = std::env::temp_dir().join(format!("tinytrader-jobs-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let store = Arc::new(Mutex::new(ready_store()));
    let url = serve_store(store.clone()).await;
    let fake = Arc::new(Fake::default());
    let ledger = ledger::Ledger::open(&dir.join("submissions.jsonl")).unwrap();
    // The fake store is plain http: no certificates, so none are loaded
    // from the system (a build sandbox has none).
    let http = reqwest::Client::builder()
        .tls_certs_only(Vec::<reqwest::Certificate>::new())
        .build()
        .unwrap();
    let mut jobs = Jobs::new(fake.clone(), "key".into(), &url, None, Some(ledger), http);
    // Short limits on Databento's answers, to keep the tests fast.
    jobs.db = Arc::new(api::Timed {
        inner: fake.clone(),
        read: Duration::from_secs(2),
        submit: Duration::from_millis(300),
    });
    setup(&mut jobs);
    let jobs = Arc::new(jobs);
    Rig {
        jobs,
        fake,
        store,
        dir,
    }
}

/// The approval token of the rigs that take paid purchases.
const APPROVE: &str = "approve-test";

impl Rig {
    /// Price a request: `/price` with these fields over the defaults (SPY's
    /// whole chain on `DAY`).
    async fn price(&self, fields: Value) -> (StatusCode, Value) {
        let mut v = json!({ "dataset": DS.to_lowercase(), "schema": SCHEMA, "symbol": "spy.opt", "start": DAY, "end": DAY });
        for (k, x) in fields.as_object().unwrap() {
            v[k] = x.clone();
        }
        let (code, Json(v)) = price(
            AxState(self.jobs.clone()),
            Json(serde_json::from_value(v).unwrap()),
        )
        .await;
        (code, v)
    }

    /// SPY's contracts on `DAY`, by the (empty) rule: bought by contract.
    async fn price_contracts(&self, again: Option<Value>) -> (StatusCode, Value) {
        self.price(json!({ "rule": {}, "again": again })).await
    }

    /// The quote for a whole symbol over these days ("" when it can't be priced).
    async fn quote(&self, symbol: &str, start: &str, end: &str) -> String {
        let (_, v) = self
            .price(json!({ "symbol": symbol, "start": start, "end": end }))
            .await;
        v["quote"].as_str().unwrap_or_default().to_owned()
    }

    async fn submit(&self, quote: &str, again: Option<Value>) -> (StatusCode, Value) {
        self.submit_as(quote, again, None, None, None).await
    }

    /// Submit with an approval: the bearer token, who, and the confirmed price.
    async fn submit_as(
        &self,
        quote: &str,
        again: Option<Value>,
        bearer: Option<&str>,
        by: Option<&str>,
        confirm_usd: Option<f64>,
    ) -> (StatusCode, Value) {
        let mut headers = axum::http::HeaderMap::new();
        if let Some(b) = bearer {
            headers.insert("authorization", format!("Bearer {b}").parse().unwrap());
        }
        let (code, Json(v)) = submit(
            AxState(self.jobs.clone()),
            headers,
            Json(Submit {
                quote: quote.into(),
                confirm_usd,
                again,
                approved_by: by.map(Into::into),
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
                ..Default::default()
            })
            .unwrap();
    }

    fn entries(&self) -> Vec<Value> {
        let text = std::fs::read_to_string(self.dir.join("submissions.jsonl")).unwrap_or_default();
        text.lines()
            .map(|l| serde_json::from_str::<Value>(l).unwrap())
            .collect()
    }

    fn states(&self) -> Vec<String> {
        self.entries()
            .iter()
            .map(|e| e["state"].as_str().unwrap().to_owned())
            .collect()
    }

    /// Hold every price until `more` more have been asked for (or 300 ms).
    fn gate(&self, more: usize) {
        let mut s = lock(&self.fake.s);
        s.cost_gate = s.costs_asked + more;
    }
}

fn whole_day() -> had::Window {
    had::day_window(date(DAY))
}

fn fp(symbols: &[&str]) -> String {
    let s: Vec<String> = symbols.iter().map(|x| (*x).to_owned()).collect();
    ledger::fingerprint(DS, SCHEMA, &s, DAY, "2026-10-03")
}

/// The contract-list requests a quote prices: each one's window and count.
fn requests(v: &Value) -> Vec<(Value, u64)> {
    rows(v)
        .iter()
        .filter(|r| r["kind"] == "contracts")
        .map(|r| (r["window"].clone(), r["contracts"].as_u64().unwrap()))
        .collect()
}

/// The whole-symbol requests a quote prices: each one's first and last day.
fn wholes(v: &Value) -> Vec<(String, String)> {
    rows(v)
        .iter()
        .filter(|r| r["kind"] == "whole")
        .map(|r| {
            (
                r["first_day"].as_str().unwrap().to_owned(),
                r["last_day"].as_str().unwrap().to_owned(),
            )
        })
        .collect()
}

fn rows(v: &Value) -> Vec<Value> {
    v["requests"].as_array().cloned().unwrap_or_default()
}

#[tokio::test]
async fn two_concurrent_submits_buy_once() {
    let r = rig("concurrent").await;
    // The same request twice, then two that overlap by a day.
    let (a, b) = (
        r.quote("SPY.OPT", DAY, DAY).await,
        r.quote("SPY.OPT", DAY, DAY).await,
    );
    // Both are checked before either is sent, unless one waits its turn.
    r.gate(2);
    let ((ca, _), (cb, _)) = tokio::join!(r.submit(&a, None), r.submit(&b, None));
    assert_eq!(r.fake.submits(), 1, "{ca} {cb}");
    let (c, d) = (
        r.quote("QQQ.OPT", "2026-10-01", DAY).await,
        r.quote("QQQ.OPT", DAY, "2026-10-05").await,
    );
    r.gate(2);
    let ((cc, _), (cd, _)) = tokio::join!(r.submit(&c, None), r.submit(&d, None));
    assert_eq!(r.fake.submits(), 2, "{cc} {cd}");
    assert!([cc, cd].contains(&StatusCode::CONFLICT));
}

#[tokio::test]
async fn a_submission_lost_after_databento_accepted_it_is_never_bought_again() {
    let r = rig("lost").await;
    lock(&r.fake.s).submit_lost = true;
    let (code, _) = r.submit(&r.quote("SPY.OPT", DAY, DAY).await, None).await;
    assert_eq!(code, StatusCode::BAD_GATEWAY);
    // It doesn't show on Databento's list yet: still never bought again.
    lock(&r.fake.s).submit_lost = false;
    let (code, v) = r.submit(&r.quote("SPY.OPT", DAY, DAY).await, None).await;
    assert_eq!(code, StatusCode::CONFLICT, "{v}");
    refresh(&r.jobs).await.unwrap();
    assert_eq!(r.states(), ["sending", "uncertain"]);
    // It shows: reconciled to its job, and still never bought again.
    lock(&r.fake.s).hidden.clear();
    refresh(&r.jobs).await.unwrap();
    assert_eq!(r.states(), ["sending", "uncertain", "submitted"]);
    let (code, _) = r.submit(&r.quote("SPY.OPT", DAY, DAY).await, None).await;
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
    let (code, _) = r.submit(&r.quote("SPY.OPT", DAY, DAY).await, None).await;
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
    let (code, v) = r.submit(&r.quote("SPY.OPT", DAY, DAY).await, None).await;
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
        // The whole chain, or by contract: never the parent, only what's left.
        for asked in [json!({}), json!({ "rule": {} })] {
            let (code, v) = r.price(asked).await;
            assert_eq!(code, StatusCode::OK, "{v}");
            assert!(wholes(&v).is_empty(), "{symbols}: {v}");
            let left_now: u64 = requests(&v).iter().map(|(_, n)| n).sum();
            assert_eq!(left_now, left, "{symbols}: {v}");
        }
        assert_eq!(r.fake.submits(), 0);
    }
}

#[tokio::test]
async fn ledger_purchases_listed_or_not_are_subtracted() {
    let r = rig("ledger").await;
    // Bought here, its job expired and gone from the list.
    r.write(&fp(&[C1]), "submitted", Some("OPRA-GONE"), 1.0);
    let (_, v) = r.price_contracts(None).await;
    assert_eq!(requests(&v), [(json!([DAY, "2026-10-03"]), 1)]);
    // And one whose outcome isn't known.
    r.write(&fp(&[C2]), "uncertain", None, 2.0);
    let (_, v) = r.price_contracts(None).await;
    assert!(rows(&v).is_empty(), "{v}");
}

#[tokio::test]
async fn buying_again_names_exactly_the_expired_jobs() {
    let r = rig("again").await;
    r.write(&fp(&["SPY.OPT"]), "submitted", Some("OPRA-GONE"), 1.0);
    let (code, v) = r.submit(&r.quote("SPY.OPT", DAY, DAY).await, None).await;
    assert_eq!(code, StatusCode::CONFLICT, "{v}");
    assert!(v["error"].as_str().unwrap().contains("OPRA-GONE"), "{v}");
    for (again, code) in [
        (json!(true), StatusCode::BAD_REQUEST),
        (json!(["OPRA-OTHER"]), StatusCode::BAD_REQUEST),
        (json!(["OPRA-GONE", "OPRA-OTHER"]), StatusCode::BAD_REQUEST),
    ] {
        assert_eq!(r.price(json!({ "again": again })).await.0, code, "{again}");
    }
    assert_eq!(r.fake.submits(), 0);
    let (_, v) = r.price(json!({ "again": ["OPRA-GONE"] })).await;
    assert_eq!(wholes(&v), [(DAY.to_owned(), DAY.to_owned())], "{v}");
    let (code, v) = r.submit(v["quote"].as_str().unwrap(), None).await;
    assert_eq!(code, StatusCode::OK, "{v}");
    // A job still on the list is never bought again, named or not.
    let id = v["submitted"][0]["job"].as_str().unwrap();
    assert_eq!(
        r.price(json!({ "again": [id] })).await.0,
        StatusCode::BAD_REQUEST
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
        // Priced while the store could say, submitted once it can't.
        let q = r.quote("SPY.OPT", DAY, DAY).await;
        {
            let mut s = lock(&r.store);
            s.catalog = catalog;
            s.coverage = coverage;
        }
        let (code, v) = r.submit(&q, None).await;
        assert_eq!(code, StatusCode::SERVICE_UNAVAILABLE, "{n}: {v}");
        assert_eq!(r.price(json!({})).await.0, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(r.fake.submits(), 0);
    }
    // Nor while Databento's job list can't be read.
    let r = rig("store-list").await;
    let q = r.quote("SPY.OPT", DAY, DAY).await;
    lock(&r.fake.s).list_fails = true;
    let (code, _) = r.submit(&q, None).await;
    assert_eq!(code, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(r.price(json!({})).await.0, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(r.fake.submits(), 0);
}

#[tokio::test]
async fn a_catalog_day_coverage_doesnt_explain_is_refused() {
    let dataset = |provider: &str| json!({ "provider": provider, "filed": true, "id": DS, "symbol": "SPY.OPT", "schemas": { SCHEMA: { "days": { DAY: {} } } } });
    // Another provider's holdings don't count for Databento's.
    let r = rig("unexplained").await;
    lock(&r.store).catalog.1 =
        json!({ "roots": [{ "datasets": [dataset("alpaca")] }], "missing_volumes": [] });
    let (code, v) = r.submit(&r.quote("SPY.OPT", DAY, DAY).await, None).await;
    assert_eq!(code, StatusCode::OK, "{v}");
    let r = rig("unexplained-2").await;
    lock(&r.store).catalog.1 =
        json!({ "roots": [{ "datasets": [dataset("databento")] }], "missing_volumes": [] });
    let (code, v) = r.submit(&r.quote("SPY.OPT", DAY, DAY).await, None).await;
    assert_eq!(code, StatusCode::CONFLICT, "{v}");
    let (_, v) = r.price_contracts(None).await;
    assert!(rows(&v).is_empty(), "{v}");
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
    // The whole chain, partly held: by contract, the hours each still needs.
    let (code, v) = r.price(json!({})).await;
    assert_eq!(code, StatusCode::OK, "{v}");
    assert!(wholes(&v).is_empty(), "{v}");
    assert_eq!(
        requests(&v),
        [
            (json!([DAY, "2026-10-02T13:00:00.000000000Z"]), 1),
            (json!([DAY, "2026-10-03"]), 1)
        ]
    );
    let hours = (vec![C1.to_owned()], (day.0, day.0 + 13 * HOUR));
    assert!(lock(&r.fake.s).costed.contains(&hours));
    let (code, v) = r.submit(v["quote"].as_str().unwrap(), None).await;
    assert_eq!(code, StatusCode::OK, "{v}");
    assert_eq!(r.fake.submits(), 2);
    // Both now pending: nothing left to buy.
    let (_, v) = r.price_contracts(None).await;
    assert!(rows(&v).is_empty(), "{v}");
}

#[tokio::test]
async fn a_price_that_isnt_a_number_of_dollars_is_refused() {
    for price in [f64::NAN, -1.0] {
        let r = paid("nan", price).await;
        // Never free: it needs the approval, and approved, it is still refused.
        let (code, v) = r.submit(&r.quote("SPY.OPT", DAY, DAY).await, None).await;
        assert_eq!(code, StatusCode::FORBIDDEN, "{v}");
        let (_, v) = r.price_contracts(None).await;
        assert_eq!(v["needs_approval"], true, "{v}");
        let (code, v) = r
            .submit_as(
                v["quote"].as_str().unwrap(),
                None,
                Some(APPROVE),
                Some("me"),
                Some(price),
            )
            .await;
        assert_ne!(code, StatusCode::OK, "{v}");
        assert_eq!(r.fake.submits(), 0);
    }
}

#[tokio::test]
async fn names_are_canonical_and_datasets_known() {
    let r = rig("names").await;
    let (code, v) = r.submit(&r.quote("spy.opt", DAY, DAY).await, None).await;
    assert_eq!(code, StatusCode::OK, "{v}");
    let sent = lock(&r.fake.s).submitted[0].clone();
    assert_eq!(
        (sent.dataset.as_str(), sent.symbols.to_api_string()),
        (DS, "SPY.OPT".into())
    );
    let (code, _) = r.price(json!({ "dataset": "OPRA.NOPE" })).await;
    assert_eq!(code, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn a_hung_submission_is_uncertain_and_frees_the_lock() {
    let r = rig("hung").await;
    // Databento never answers for SPY: while it hangs, a QQQ purchase still
    // goes through once the SPY submission gives up.
    let (spy, qqq) = (
        r.quote("SPY.OPT", DAY, DAY).await,
        r.quote("QQQ.OPT", DAY, DAY).await,
    );
    lock(&r.fake.s).hang_on = Some("SPY".into());
    let other = async {
        tokio::time::sleep(Duration::from_millis(50)).await;
        r.submit(&qqq, None).await
    };
    let both = async { tokio::join!(r.submit(&spy, None), other) };
    let ((first, why), (second, v)) = tokio::time::timeout(Duration::from_secs(5), both)
        .await
        .expect("a hung submission must not hold the purchase lock");
    assert_eq!(first, StatusCode::BAD_GATEWAY, "{why}");
    let why = why["error"].as_str().unwrap();
    assert!(
        why.contains("no answer from Databento") && why.contains("outcome is unknown"),
        "{why}"
    );
    assert_eq!(second, StatusCode::OK, "{v}");
    // The SPY one is never bought again while its outcome isn't known.
    let (code, v) = r.submit(&r.quote("SPY.OPT", DAY, DAY).await, None).await;
    assert_eq!(code, StatusCode::CONFLICT);
    assert!(
        v["error"].as_str().unwrap().contains("nothing to buy"),
        "{v}"
    );
    assert_eq!(r.states(), ["sending", "uncertain", "sending", "submitted"]);
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
        note: Some("no such job appeared in Databento's job list".into()),
        ..Default::default()
    };
    lock(&r.jobs.ledger)
        .as_mut()
        .unwrap()
        .append(off.clone())
        .unwrap();
    let (code, _) = r.submit(&r.quote("SPY.OPT", DAY, DAY).await, None).await;
    assert_eq!(code, StatusCode::CONFLICT);
    // An operator's write-off doesn't block.
    off.note = Some("resolved by the operator: checked".into());
    lock(&r.jobs.ledger).as_mut().unwrap().append(off).unwrap();
    let (code, v) = r.submit(&r.quote("SPY.OPT", DAY, DAY).await, None).await;
    assert_eq!(code, StatusCode::OK, "{v}");
}

#[tokio::test]
async fn an_old_automatic_write_off_is_matched_by_reconcile() {
    let r = rig("written-off-matched").await;
    let now = tinytrader_observe::unix_seconds();
    r.write(&fp(&["SPY.OPT"]), "sending", None, now - 900.0);
    lock(&r.jobs.ledger)
        .as_mut()
        .unwrap()
        .append(ledger::Entry {
            at: now - 100.0,
            request: fp(&["SPY.OPT"]),
            state: "not-submitted".into(),
            note: Some("no such job appeared in Databento's job list".into()),
            ..Default::default()
        })
        .unwrap();
    // Its job, received soon after it was sent, has expired since.
    let mut j = job(
        "OPRA-MATCH",
        json!("SPY.OPT"),
        "parent",
        whole_day(),
        "expired",
    );
    j.ts_received -= time::Duration::seconds(890);
    r.fake.add(j);
    refresh(&r.jobs).await.unwrap();
    assert_eq!(r.states(), ["sending", "not-submitted", "submitted"]);
    let l = lock(&r.jobs.ledger);
    assert!(l.as_ref().unwrap().submitted().contains("OPRA-MATCH"));
}

fn resolution(request: &str, at: f64, job: Option<&str>) -> Json<Resolve> {
    Json(Resolve {
        request: request.into(),
        at,
        job: job.map(Into::into),
        note: "checked the portal".into(),
    })
}

#[tokio::test]
async fn never_made_only_after_half_an_hour_and_a_full_read_since() {
    let r = rig("resolve-none").await;
    let now = tinytrader_observe::unix_seconds();
    let req = fp(&["SPY.OPT"]);
    r.write(&req, "sending", None, now - 60.0);
    r.write(&req, "uncertain", None, now - 50.0);
    refresh(&r.jobs).await.unwrap();
    let (code, Json(v)) =
        resolve(AxState(r.jobs.clone()), resolution(&req, now - 50.0, None)).await;
    assert_eq!(code, StatusCode::CONFLICT, "{v}");
    assert!(v["error"].as_str().unwrap().contains("only after"), "{v}");
    // Old enough, but no full read since then.
    let r = rig("resolve-none-2").await;
    r.write(&req, "sending", None, now - 3600.0);
    r.write(&req, "uncertain", None, now - 3500.0);
    let (code, _) = resolve(
        AxState(r.jobs.clone()),
        resolution(&req, now - 3500.0, None),
    )
    .await;
    assert_eq!(code, StatusCode::CONFLICT);
    // A read that couldn't see every job doesn't count.
    r.fake.add(job(
        "OPRA-SLOW",
        json!("QQQ.OPT"),
        "parent",
        whole_day(),
        "queued",
    ));
    lock(&r.fake.s).unreadable.insert("OPRA-SLOW".into());
    refresh(&r.jobs).await.unwrap();
    let (code, _) = resolve(
        AxState(r.jobs.clone()),
        resolution(&req, now - 3500.0, None),
    )
    .await;
    assert_eq!(code, StatusCode::CONFLICT);
    lock(&r.fake.s).unreadable.clear();
    refresh(&r.jobs).await.unwrap();
    let (code, Json(v)) = resolve(
        AxState(r.jobs.clone()),
        resolution(&req, now - 3500.0, None),
    )
    .await;
    assert_eq!(code, StatusCode::OK, "{v}");
}

#[tokio::test]
async fn resolving_to_a_job_checks_the_job() {
    let r = rig("resolve-job").await;
    let now = tinytrader_observe::unix_seconds();
    let req = fp(&["SPY.OPT"]);
    r.write(&req, "sending", None, now);
    r.write(&fp(&["QQQ.OPT"]), "submitted", Some("OPRA-TAKEN"), now);
    let mut other = job("OPRA-DEFS", json!("SPY.OPT"), "parent", whole_day(), "done");
    other.schema = Schema::Definition;
    r.fake.add(other);
    r.fake.add(job(
        "OPRA-TAKEN",
        json!("SPY.OPT"),
        "parent",
        whole_day(),
        "done",
    ));
    r.fake.add(job(
        "OPRA-MINE",
        json!("SPY.OPT"),
        "parent",
        whole_day(),
        "done",
    ));
    for id in ["OPRA-NONE", "OPRA-DEFS", "OPRA-TAKEN"] {
        let (code, Json(v)) =
            resolve(AxState(r.jobs.clone()), resolution(&req, now, Some(id))).await;
        assert_eq!(code, StatusCode::CONFLICT, "{id}: {v}");
    }
    let (code, Json(v)) = resolve(
        AxState(r.jobs.clone()),
        resolution(&req, now, Some("OPRA-MINE")),
    )
    .await;
    assert_eq!(code, StatusCode::OK, "{v}");
}

#[tokio::test]
async fn buying_missing_contracts_again_names_exactly_the_expired_jobs() {
    let r = rig("again-missing").await;
    r.write(&fp(&[C1]), "submitted", Some("OPRA-GONE"), 1.0);
    let (_, v) = r.price_contracts(None).await;
    assert_eq!(requests(&v), [(json!([DAY, "2026-10-03"]), 1)]);
    assert!(v["notes"].to_string().contains("OPRA-GONE"), "{v}");
    assert_eq!(
        r.price_contracts(Some(json!(true))).await.0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        r.price_contracts(Some(json!(["OPRA-OTHER"]))).await.0,
        StatusCode::BAD_REQUEST
    );
    let (code, v) = r.price_contracts(Some(json!(["OPRA-GONE"]))).await;
    assert_eq!(code, StatusCode::OK, "{v}");
    assert_eq!(requests(&v), [(json!([DAY, "2026-10-03"]), 2)]);
    assert_eq!(v["again"], json!(["OPRA-GONE"]));
    // A submission that names `again` names exactly what the quote did.
    let quote = v["quote"].as_str().unwrap().to_owned();
    let (code, _) = r.submit(&quote, Some(json!(["OPRA-OTHER"]))).await;
    assert_eq!(code, StatusCode::CONFLICT);
    let (_, v) = r.price_contracts(Some(json!(["OPRA-GONE"]))).await;
    let (code, v) = r
        .submit(v["quote"].as_str().unwrap(), Some(json!(["OPRA-GONE"])))
        .await;
    assert_eq!(code, StatusCode::OK, "{v}");
    assert_eq!(r.fake.submits(), 1);
}

#[tokio::test]
async fn a_purchase_rechecks_against_a_fresh_scan_and_a_price_doesnt() {
    let r = rig("fresh").await;
    let fresh = |r: &Rig| {
        let s = lock(&r.store);
        let f = |v: &[String]| v.iter().map(|q| q.contains("fresh=1")).collect::<Vec<_>>();
        (f(&s.catalog_asked), f(&s.coverage_asked))
    };
    let clear = |r: &Rig| {
        let mut s = lock(&r.store);
        s.catalog_asked.clear();
        s.coverage_asked.clear();
    };
    for asked in [json!({ "rule": {} }), json!({ "symbol": "QQQ.OPT" })] {
        clear(&r);
        let (_, v) = r.price(asked).await;
        assert_eq!(fresh(&r), (vec![false], vec![false]));
        clear(&r);
        let (code, v) = r.submit(v["quote"].as_str().unwrap(), None).await;
        assert_eq!(code, StatusCode::OK, "{v}");
        assert_eq!(fresh(&r), (vec![true], vec![true]));
    }
}

#[tokio::test]
async fn a_paid_purchase_needs_the_consoles_approval() {
    let r = rig_with("paid", |j| j.approve_token = Some(APPROVE.into())).await;
    lock(&r.fake.s).price = 2.5;
    let q = || r.quote("SPY.OPT", DAY, DAY);
    let (_, v) = r.price(json!({})).await;
    assert_eq!(
        (
            v["needs_approval"].clone(),
            v["valid_seconds"].as_u64().unwrap() > 3500
        ),
        (json!(true), true)
    );
    for (bearer, by, confirm, code) in [
        (
            None,
            Some("me@example.com"),
            Some(2.5),
            StatusCode::FORBIDDEN,
        ),
        (
            Some("guess"),
            Some("me@example.com"),
            Some(2.5),
            StatusCode::FORBIDDEN,
        ),
        (Some(APPROVE), None, Some(2.5), StatusCode::FORBIDDEN),
        (Some(APPROVE), Some(" "), Some(2.5), StatusCode::FORBIDDEN),
        (
            Some(APPROVE),
            Some("me@example.com"),
            None,
            StatusCode::PRECONDITION_REQUIRED,
        ),
        (
            Some(APPROVE),
            Some("me@example.com"),
            Some(2.49),
            StatusCode::PRECONDITION_REQUIRED,
        ),
    ] {
        let (got, v) = r.submit_as(&q().await, None, bearer, by, confirm).await;
        assert_eq!(got, code, "{bearer:?} {by:?} {confirm:?}: {v}");
    }
    assert!(r.entries().is_empty() && r.fake.submits() == 0);
    let quote = q().await;
    let (code, v) = r
        .submit_as(
            &quote,
            None,
            Some(APPROVE),
            Some("me@example.com"),
            Some(2.5),
        )
        .await;
    assert_eq!(code, StatusCode::OK, "{v}");
    let e = r.entries();
    assert_eq!(r.states(), ["approved", "sending", "submitted"]);
    assert_eq!(
        (
            e[0]["by"].as_str(),
            e[0]["quote"].as_str(),
            e[0]["cost_usd"].as_f64()
        ),
        (Some("me@example.com"), Some(quote.as_str()), Some(2.5))
    );
    // Without an approval token, paid purchases are off altogether.
    let r = rig("paid-off").await;
    lock(&r.fake.s).price = 2.5;
    let (code, v) = r
        .submit_as(
            &r.quote("SPY.OPT", DAY, DAY).await,
            None,
            Some(APPROVE),
            Some("me"),
            Some(2.5),
        )
        .await;
    assert_eq!(code, StatusCode::FORBIDDEN);
    assert!(
        v["error"].as_str().unwrap().contains("JOBS_APPROVE_TOKEN"),
        "{v}"
    );
    // A free one needs none of it, and lives 15 minutes.
    let r = rig("free").await;
    let (_, v) = r.price(json!({})).await;
    assert!(v["valid_seconds"].as_u64().unwrap() <= 900);
    let (code, v) = r.submit(v["quote"].as_str().unwrap(), None).await;
    assert_eq!(code, StatusCode::OK, "{v}");
    assert_eq!(r.states(), ["sending", "submitted"]);
}

#[tokio::test]
async fn quotes_are_listed_in_full_and_declined_with_the_token() {
    let r = rig_with("quotes", |j| j.approve_token = Some(APPROVE.into())).await;
    lock(&r.fake.s).price = 1.25;
    let quote = r.quote("SPY.OPT", DAY, DAY).await;
    let (_, Json(list)) = quotes_list(AxState(r.jobs.clone())).await;
    let q = &list["quotes"][0];
    assert_eq!(
        (
            q["quote"].as_str(),
            q["dataset"].as_str(),
            q["symbol"].as_str(),
            q["cost_usd"].as_f64(),
            q["state"].as_str()
        ),
        (
            Some(quote.as_str()),
            Some(DS),
            Some("SPY.OPT"),
            Some(1.25),
            Some("ready")
        )
    );
    assert_eq!(q["requests"][0]["kind"], "whole");
    assert!(q["notes"].is_array() && q["again"].is_array() && q["billable_bytes"].is_u64());
    let del = |bearer: Option<&str>| {
        let mut h = axum::http::HeaderMap::new();
        if let Some(b) = bearer {
            h.insert("authorization", format!("Bearer {b}").parse().unwrap());
        }
        quote_delete(AxState(r.jobs.clone()), h, UrlPath(quote.clone()))
    };
    assert_eq!(del(None).await.0, StatusCode::FORBIDDEN);
    assert_eq!(del(Some("guess")).await.0, StatusCode::FORBIDDEN);
    assert_eq!(del(Some(APPROVE)).await.0, StatusCode::OK);
    let (code, _) = r
        .submit_as(&quote, None, Some(APPROVE), Some("me"), Some(1.25))
        .await;
    assert_eq!(code, StatusCode::CONFLICT);
}

#[tokio::test]
async fn contract_lists_and_session_hours() {
    let r = rig("lists").await;
    let rth = json!({ "start": "09:30", "end": "16:01", "tz": "America/New_York" });
    let (code, v) = r
        .price(json!({ "contracts": { DAY: [C1] }, "hours": rth }))
        .await;
    assert_eq!(code, StatusCode::OK, "{v}");
    // 2026-10-02 is in daylight saving: 13:30 to 20:01 UTC.
    assert_eq!(
        requests(&v),
        [(
            json!([
                "2026-10-02T13:30:00.000000000Z",
                "2026-10-02T20:01:00.000000000Z"
            ]),
            1
        )]
    );
    // Contracts not in the day's definitions, or a day without any, are refused.
    let (code, v) = r
        .price(json!({ "contracts": { DAY: [C1, "SPY   261002C00999000"] } }))
        .await;
    assert_eq!(code, StatusCode::BAD_REQUEST);
    assert!(
        v["error"]
            .as_str()
            .unwrap()
            .contains("SPY   261002C00999000"),
        "{v}"
    );
    let (code, _) = r
        .price(json!({ "start": "2026-10-01", "contracts": { "2026-10-01": [C1] } }))
        .await;
    assert_eq!(code, StatusCode::BAD_REQUEST);
    // What's held of those hours is subtracted: C1 held from 13:00 covers it.
    let day = whole_day();
    lock(&r.store).coverage.1 = json!({ "days": [
        { "day": DAY, "kind": "partial", "volumes": ["v1"], "coverage": { "kind": "contracts", "symbols": [C1], "window": [day.0 + 13 * HOUR, day.1] } },
    ], "missing_volumes": [] });
    let (_, v) = r
        .price(json!({ "contracts": { DAY: [C1, C2] }, "hours": rth }))
        .await;
    assert_eq!(
        requests(&v),
        [(
            json!([
                "2026-10-02T13:30:00.000000000Z",
                "2026-10-02T20:01:00.000000000Z"
            ]),
            1
        )]
    );
    assert_eq!(rows(&v)[0]["skipped_held"], 1);
}

#[tokio::test]
async fn untouched_days_are_bought_whole_in_runs() {
    let r = rig("runs").await;
    lock(&r.store).coverage.1 = json!({ "days": [
        { "day": DAY, "kind": "complete", "volumes": ["v1"] },
    ], "missing_volumes": [] });
    let (_, v) = r
        .price(json!({ "start": "2026-09-30", "end": "2026-10-05" }))
        .await;
    assert_eq!(
        wholes(&v),
        [
            ("2026-09-30".to_owned(), "2026-10-01".to_owned()),
            ("2026-10-03".to_owned(), "2026-10-05".to_owned())
        ],
        "{v}"
    );
    assert!(requests(&v).is_empty());
    // With hours, each day is its own request.
    let rth = json!({ "start": "09:30", "end": "16:01" });
    let (_, v) = r
        .price(json!({ "start": "2026-10-05", "end": "2026-10-06", "hours": rth }))
        .await;
    assert_eq!(wholes(&v).len(), 2, "{v}");
    let (code, v) = r.submit(v["quote"].as_str().unwrap(), None).await;
    assert_eq!(code, StatusCode::OK, "{v}");
    let sent = lock(&r.fake.s).submitted.clone();
    assert_eq!(sent[0].stype_in, SType::Parent);
    assert_eq!(
        had::ns(sent[0].date_time_range.start),
        had::day_window(date("2026-10-05")).0 + 13 * HOUR + 30 * 60_000_000_000
    );
}

#[tokio::test]
async fn a_day_partly_had_of_a_plain_symbol_isnt_bought() {
    let r = rig("plain").await;
    r.fake.add(job(
        "OPRA-X",
        json!("SPY"),
        "raw_symbol",
        whole_day(),
        "done",
    ));
    let (_, v) = r
        .price(json!({ "symbol": "SPY", "start": "2026-10-01", "end": "2026-10-03" }))
        .await;
    assert_eq!(
        wholes(&v),
        [
            ("2026-10-01".to_owned(), "2026-10-01".to_owned()),
            ("2026-10-03".to_owned(), "2026-10-03".to_owned())
        ],
        "{v}"
    );
    assert!(v["notes"].to_string().contains("OPRA-X"), "{v}");
}

#[tokio::test]
async fn pricing_is_tried_again_only_on_server_failures() {
    let r = rig("retry").await;
    let errors =
        |e: &[&str]| lock(&r.fake.s).cost_errors = e.iter().map(|x| (*x).to_owned()).collect();
    errors(&[
        "API error: 503 Service Unavailable",
        "no answer from Databento in 60 s",
    ]);
    let (code, v) = r.price(json!({})).await;
    assert_eq!(code, StatusCode::OK, "{v}");
    assert_eq!(lock(&r.fake.s).costed.len(), 3);
    errors(&["API error: 503 Service Unavailable"; 3]);
    assert_eq!(r.price(json!({})).await.0, StatusCode::BAD_GATEWAY);
    lock(&r.fake.s).costed.clear();
    errors(&["API error: 422 Unprocessable Entity bad symbols"]);
    assert_eq!(r.price(json!({})).await.0, StatusCode::BAD_GATEWAY);
    assert_eq!(lock(&r.fake.s).costed.len(), 1, "a 4xx isn't tried again");
}

#[tokio::test]
async fn a_long_price_is_answered_while_it_is_worked_out() {
    let r = rig_with("slow", |j| j.price_wait = Duration::ZERO).await;
    r.gate(100);
    let (code, v) = r.price(json!({ "start": "2026-10-01", "end": "2026-10-05", "hours": { "start": "09:30", "end": "16:01" } })).await;
    assert_eq!(code, StatusCode::ACCEPTED, "{v}");
    assert_eq!(v["state"], "pricing");
    let quote = v["quote"].as_str().unwrap().to_owned();
    // Not yet submittable.
    assert_eq!(r.submit(&quote, None).await.0, StatusCode::CONFLICT);
    let v = loop {
        let (_, Json(v)) = quote_get(AxState(r.jobs.clone()), UrlPath(quote.clone())).await;
        if v["state"] != "pricing" {
            break v;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    };
    assert_eq!(
        (v["state"].as_str(), v["priced"].as_u64(), v["of"].as_u64()),
        (Some("ready"), Some(5), Some(5))
    );
}

#[tokio::test]
async fn whole_days_tile_across_daylight_saving_changes() {
    let r = rig("dst").await;
    let all = json!({ "start": "00:00", "end": "24:00", "tz": "America/New_York" });
    for (start, end) in [("2026-03-07", "2026-03-09"), ("2026-10-31", "2026-11-02")] {
        let (code, v) = r
            .price(json!({ "start": start, "end": end, "hours": all }))
            .await;
        assert_eq!(code, StatusCode::OK, "{v}");
        let w: Vec<Value> = rows(&v).iter().map(|x| x["window"].clone()).collect();
        assert_eq!(w.len(), 3, "{v}");
        assert_eq!((&w[0][1], &w[1][1]), (&w[1][0], &w[2][0]), "{v}");
    }
}

#[test]
fn a_plan_that_would_buy_twice_is_caught() {
    let part = |w: had::Window, whole: bool, symbols: &[&str]| missing::Part {
        day: date(DAY),
        last: date(DAY),
        window: w,
        whole,
        symbols: symbols.iter().map(|x| (*x).to_owned()).collect(),
        stype: SType::RawSymbol,
        cost_usd: 0.0,
        bytes: 0,
        skipped_held: 0,
        skipped_pending: 0,
    };
    assert!(overlap(&[part((0, 10), false, &[C1]), part((10, 20), false, &[C1])]).is_none());
    assert!(overlap(&[part((0, 10), false, &[C1]), part((5, 20), false, &[C2])]).is_none());
    assert!(overlap(&[part((0, 10), false, &[C1]), part((5, 20), false, &[C2, C1])]).is_some());
    assert!(
        overlap(&[
            part((0, 10), true, &["SPY.OPT"]),
            part((9, 20), false, &[C2])
        ])
        .is_some()
    );
}

/// A rig that takes paid purchases, priced at `usd` each.
async fn paid(name: &str, usd: f64) -> Rig {
    let r = rig_with(name, |j| j.approve_token = Some(APPROVE.into())).await;
    lock(&r.fake.s).price = usd;
    r
}

#[tokio::test]
async fn an_unapproved_or_unconfirmed_submit_leaves_the_quote() {
    let r = paid("keep", 2.5).await;
    let q = r.quote("SPY.OPT", DAY, DAY).await;
    assert_eq!(r.submit(&q, None).await.0, StatusCode::FORBIDDEN);
    let me = Some("me@example.com");
    assert_eq!(
        r.submit_as(&q, None, Some(APPROVE), me, Some(2.4)).await.0,
        StatusCode::PRECONDITION_REQUIRED
    );
    let (code, v) = r.submit_as(&q, None, Some(APPROVE), me, Some(2.5)).await;
    assert_eq!(code, StatusCode::OK, "{v}");
    assert_eq!(r.fake.submits(), 1);
}

#[tokio::test]
async fn each_request_is_priced_as_dollars() {
    // Two days' requests at $1 and -$1: zero in all, but not free.
    let r = paid("parts", 0.0).await;
    lock(&r.fake.s).prices = [1.0, -1.0, 1.0, -1.0].into();
    let rth = json!({ "start": "09:30", "end": "16:01" });
    let (_, v) = r
        .price(json!({ "start": "2026-10-05", "end": "2026-10-06", "hours": rth }))
        .await;
    assert_eq!(v["needs_approval"], true, "{v}");
    let q = v["quote"].as_str().unwrap().to_owned();
    assert_eq!(r.submit(&q, None).await.0, StatusCode::FORBIDDEN);
    assert_ne!(
        r.submit_as(&q, None, Some(APPROVE), Some("me"), Some(0.0))
            .await
            .0,
        StatusCode::OK
    );
    assert_eq!(r.fake.submits(), 0);
}

#[tokio::test]
async fn quote_ids_differ_from_one_start_to_the_next() {
    let (a, b) = (rig("boot-a").await, rig("boot-b").await);
    let (qa, qb) = (
        a.quote("SPY.OPT", DAY, DAY).await,
        b.quote("SPY.OPT", DAY, DAY).await,
    );
    assert!(qa.starts_with("q-") && qa != qb, "{qa} {qb}");
}

#[tokio::test]
async fn a_price_that_takes_too_long_fails() {
    let r = rig_with("cap", |j| {
        j.price_wait = Duration::ZERO;
        j.pricing_cap = Duration::from_millis(30);
    })
    .await;
    r.gate(100);
    let (code, v) = r.price(json!({})).await;
    assert_eq!(code, StatusCode::ACCEPTED, "{v}");
    let quote = v["quote"].as_str().unwrap().to_owned();
    tokio::time::sleep(Duration::from_millis(400)).await;
    let (_, Json(v)) = quote_get(AxState(r.jobs.clone()), UrlPath(quote)).await;
    assert_eq!(v["state"], "failed", "{v}");
    assert!(v["error"].as_str().unwrap().contains("longer than"), "{v}");
}

#[tokio::test]
async fn two_approvals_of_one_quote_buy_once() {
    let r = paid("twice", 2.5).await;
    let q = r.quote("SPY.OPT", DAY, DAY).await;
    let me = Some("me@example.com");
    let ((a, _), (b, _)) = tokio::join!(
        r.submit_as(&q, None, Some(APPROVE), me, Some(2.5)),
        r.submit_as(&q, None, Some(APPROVE), me, Some(2.5))
    );
    let mut codes = [a, b];
    codes.sort();
    assert_eq!(codes, [StatusCode::OK, StatusCode::CONFLICT]);
    assert_eq!(r.fake.submits(), 1);
}

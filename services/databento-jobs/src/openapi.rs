//! This service's OpenAPI document, for the endpoints `tt` uses (decision
//! 0021, section 5). Requests are the handlers' own types; the answers,
//! built as JSON values, are described by the types below.
#![allow(dead_code)]

use schemars::JsonSchema;
use tinytrader_openapi::Spec;

use crate::{PriceRequest, Submit, missing::Hours};

/// A quote, as `/price` and `/quotes` show it.
#[derive(JsonSchema)]
struct Quote {
    /// `q-<random per start>-<n>`.
    quote: String,
    state: QuoteState,
    error: Option<String>,
    /// Requests priced so far, of how many; once submitting, `submitted` of `of`.
    priced: usize,
    of: usize,
    submitted: Option<usize>,
    dataset: String,
    schema: String,
    symbol: String,
    stype_in: String,
    start: String,
    end: String,
    hours: Option<Hours>,
    /// Whether a rule chose the contracts.
    rule: bool,
    contracts: Option<usize>,
    contract_days: Option<usize>,
    again: Vec<String>,
    /// Not a number (`null`) when a price wasn't one: never bought.
    cost_usd: Option<f64>,
    billable_bytes: Option<u64>,
    needs_approval: Option<bool>,
    valid_seconds: Option<u64>,
    /// When the quote stops being good: Unix seconds, beside `valid_seconds`.
    expires_at: Option<u64>,
    notes: Option<Vec<String>>,
    requests: Option<Vec<Request>>,
    /// Once submitting, the jobs made so far, in order.
    jobs: Option<Vec<String>>,
}

#[derive(JsonSchema)]
#[serde(rename_all = "lowercase")]
enum QuoteState {
    Pricing,
    Ready,
    Failed,
    Submitting,
    Submitted,
}

/// One request of a quote; once submitted, with its `job`.
#[derive(JsonSchema)]
struct Request {
    kind: Kind,
    first_day: String,
    last_day: String,
    /// A date (midnight UTC) or RFC 3339, each end.
    window: Vec<String>,
    hours: bool,
    cost_usd: Option<f64>,
    billable_bytes: u64,
    condition: Option<Condition>,
    symbol: Option<String>,
    contracts: Option<usize>,
    skipped_held: Option<usize>,
    skipped_pending: Option<usize>,
    job: Option<String>,
}

#[derive(JsonSchema)]
#[serde(rename_all = "lowercase")]
enum Kind {
    Whole,
    Contracts,
}

#[derive(JsonSchema)]
#[serde(rename_all = "lowercase")]
enum Condition {
    Missing,
    Pending,
    Degraded,
}

#[derive(JsonSchema)]
struct Quotes {
    quotes: Vec<Quote>,
}

/// A refused or failed submission: why, and for a quote whose submission
/// had begun, how far it got (`"3 of 10"`).
#[derive(JsonSchema)]
struct SubmitRefused {
    error: String,
    progress: Option<String>,
}

#[derive(JsonSchema)]
struct Submitted {
    submitted: Vec<Request>,
    cost_usd: f64,
}

#[test]
fn openapi() {
    let mut s = Spec::default();
    let (ask, quote) = (s.schema::<PriceRequest>(), s.schema::<Quote>());
    let pricing = (
        "202",
        "still pricing after 20 s: follow it at /quotes/{id}",
        quote.clone(),
    );
    let priced = ("200", "the quote", quote.clone());
    s.op(
        ("post", "/price"),
        "Price what's missing",
        vec![],
        Some(ask),
        &[priced.clone(), pricing],
    );
    let list = s.schema::<Quotes>();
    s.op(
        ("get", "/quotes"),
        "Quotes still good, and submissions of a day",
        vec![],
        None,
        &[("200", "the quotes", list)],
    );
    s.op(
        ("get", "/quotes/{id}"),
        "One quote",
        vec![],
        None,
        &[priced],
    );
    let (submit, done) = (s.schema::<Submit>(), s.schema::<Submitted>());
    let refused = ("default", "refused or failed", s.schema::<SubmitRefused>());
    s.op(
        ("post", "/submit"),
        "Buy a quote",
        vec![],
        Some(submit),
        &[("200", "submitted", done), refused],
    );
    let doc = s.document("databento-jobs", "See docs/api/databento-jobs.md.");
    tinytrader_openapi::check("databento-jobs", &doc);
}

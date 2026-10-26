//! This service's OpenAPI document, for the approvals `tt` sends (decision
//! 0021, section 5). The handlers read and pass on JSON values; these types
//! describe them.
#![allow(dead_code)]

use schemars::JsonSchema;
use serde_json::json;
use tinytrader_openapi::Spec;

/// A person's approval of a paid purchase.
#[derive(JsonSchema)]
struct Approve {
    /// The quoted price, exactly as the jobs service listed it.
    confirm_usd: f64,
    /// When given, exactly the quote's `again`.
    again: Option<Vec<String>>,
}

#[derive(JsonSchema)]
struct Decline {}

#[derive(JsonSchema)]
struct Declined {
    declined: String,
}

#[test]
fn openapi() {
    let mut s = Spec::default();
    let submitted = json!({ "$ref": "databento-jobs.json#/components/schemas/Submitted" });
    let (ask, done) = (
        s.schema::<Approve>(),
        ("200", "the jobs service's answer to /submit", submitted),
    );
    s.op(
        ("post", "/api/job-quotes/{id}/approve"),
        "Approve a paid purchase",
        vec![],
        Some(ask),
        &[done],
    );
    let (ask, done) = (s.schema::<Decline>(), s.schema::<Declined>());
    s.op(
        ("post", "/api/job-quotes/{id}/decline"),
        "Decline a quote",
        vec![],
        Some(ask),
        &[("200", "declined", done)],
    );
    let doc = s.document("console", "Approvals only; see docs/api/console.md.");
    tinytrader_openapi::check("console", &doc);
}

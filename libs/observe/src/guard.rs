//! Writes a web page can't forge. A service on this machine is reachable
//! from any page open in the operator's browser: the page can't read the
//! answers, but a cross-site form or `fetch(…, {mode: "no-cors"})` can still
//! send a POST. Browsers mark such requests with an `Origin` (and
//! `Sec-Fetch-Site: cross-site`), and can only send JSON cross-site after
//! asking the server first, which these services never allow. So:
//!
//! - [`same_origin_writes`] refuses a write (POST, PUT, PATCH, DELETE) whose
//!   `Origin` isn't the server's own host, or that the browser says is
//!   cross-site. Requests without an `Origin` (curl, other services) pass.
//! - [`json_writes`] also refuses a POST, PUT or PATCH not sent as JSON.

use axum::extract::Request;
use axum::http::{HeaderMap, Method, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};

fn is_write(m: &Method) -> bool {
    matches!(
        *m,
        Method::POST | Method::PUT | Method::PATCH | Method::DELETE
    )
}

/// Why a write should be refused, if it should.
#[must_use]
pub fn refuse(method: &Method, headers: &HeaderMap, json_only: bool) -> Option<&'static str> {
    if !is_write(method) {
        return None;
    }
    let get = |h: header::HeaderName| headers.get(h).and_then(|v| v.to_str().ok());
    if headers.get("sec-fetch-site").and_then(|v| v.to_str().ok()) == Some("cross-site") {
        return Some("cross-site requests can't change anything here");
    }
    if let Some(origin) = get(header::ORIGIN) {
        let from = origin.split_once("://").map_or(origin, |(_, rest)| rest);
        if Some(from) != get(header::HOST) {
            return Some("requests from another site can't change anything here");
        }
    }
    if json_only && *method != Method::DELETE {
        let ct = get(header::CONTENT_TYPE).unwrap_or("");
        if !ct
            .split(';')
            .next()
            .is_some_and(|t| t.trim().eq_ignore_ascii_case("application/json"))
        {
            return Some("changes must be sent as application/json");
        }
    }
    None
}

fn refused(why: &str) -> Response {
    (
        StatusCode::FORBIDDEN,
        axum::Json(serde_json::json!({ "error": why })),
    )
        .into_response()
}

/// Middleware: refuse writes from another site (see the module docs).
pub async fn same_origin_writes(req: Request, next: Next) -> Response {
    match refuse(req.method(), req.headers(), false) {
        Some(why) => refused(why),
        None => next.run(req).await,
    }
}

/// Middleware: as [`same_origin_writes`], and writes must be JSON.
pub async fn json_writes(req: Request, next: Next) -> Response {
    match refuse(req.method(), req.headers(), true) {
        Some(why) => refused(why),
        None => next.run(req).await,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn headers(pairs: &[(&'static str, &str)]) -> HeaderMap {
        let mut h = HeaderMap::new();
        for (k, v) in pairs {
            h.insert(*k, v.parse().unwrap());
        }
        h
    }

    #[test]
    fn refuses_what_a_web_page_could_forge() {
        let own = headers(&[
            ("host", "localhost:8080"),
            ("origin", "http://localhost:8080"),
            ("content-type", "application/json"),
        ]);
        assert_eq!(
            refuse(&Method::POST, &own, true),
            None,
            "the console's own page"
        );
        assert_eq!(
            refuse(
                &Method::POST,
                &headers(&[("content-type", "application/json")]),
                true
            ),
            None,
            "curl, no Origin"
        );
        assert_eq!(
            refuse(
                &Method::GET,
                &headers(&[("origin", "https://evil.example")]),
                true
            ),
            None,
            "reads are fine"
        );
        let forged = headers(&[
            ("host", "localhost:8080"),
            ("origin", "https://evil.example"),
            ("content-type", "text/plain"),
        ]);
        assert!(refuse(&Method::POST, &forged, false).is_some());
        let cross = headers(&[("host", "localhost:8080"), ("sec-fetch-site", "cross-site")]);
        assert!(refuse(&Method::PUT, &cross, false).is_some());
        let form = headers(&[
            ("host", "localhost:8080"),
            ("content-type", "application/x-www-form-urlencoded"),
        ]);
        assert!(
            refuse(&Method::POST, &form, true).is_some(),
            "a same-site form still isn't JSON"
        );
        assert_eq!(
            refuse(&Method::POST, &form, false),
            None,
            "without the JSON rule (the store's uploads)"
        );
    }
}

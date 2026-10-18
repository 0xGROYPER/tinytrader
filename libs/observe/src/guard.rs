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
//!
//! A service reachable from other machines also needs to know who is
//! writing. [`WriteToken`] and [`token_writes`] require a bearer token
//! (`Authorization: Bearer <token>`) on every write when one is configured;
//! reads stay open, and with no token configured writes are as before.

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

/// The token every write must carry, if any. `None` (no token configured)
/// leaves writes open, as before tokens existed.
#[derive(Clone, Debug, Default)]
pub struct WriteToken(Option<String>);

impl WriteToken {
    /// No token: writes are open.
    #[must_use]
    pub fn open() -> Self {
        Self(None)
    }

    /// Writes require this token.
    ///
    /// # Errors
    /// If it is empty (after trimming): an empty token would let anything in.
    pub fn new(token: &str) -> Result<Self, String> {
        let t = token.trim();
        if t.is_empty() {
            return Err("the write token is empty".into());
        }
        Ok(Self(Some(t.to_owned())))
    }

    /// From the environment: `NAME`, or the file `NAME_FILE` names (see
    /// [`crate::secret`]). Neither set: writes are open.
    ///
    /// # Errors
    /// If one is set but the file can't be read, or the token is empty: a
    /// service told to require a token must not start without one.
    pub fn from_env(name: &str) -> Result<Self, String> {
        if std::env::var_os(format!("{name}_FILE")).is_none() && std::env::var_os(name).is_none() {
            return Ok(Self::open());
        }
        Self::new(&crate::secret(name)?).map_err(|e| format!("{name}: {e}"))
    }

    /// Whether writes need a token.
    #[must_use]
    pub fn required(&self) -> bool {
        self.0.is_some()
    }

    /// Why a request should be refused, if it should: a write without the
    /// token. Reads always pass.
    #[must_use]
    pub fn refuse(&self, method: &Method, headers: &HeaderMap) -> Option<&'static str> {
        let want = self.0.as_deref()?;
        if !is_write(method) {
            return None;
        }
        let got = headers
            .get(header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| {
                let (scheme, rest) = v.trim().split_once(' ')?;
                scheme.eq_ignore_ascii_case("bearer").then(|| rest.trim())
            });
        match got {
            None => Some("this write needs the service's bearer token (Authorization: Bearer …)"),
            Some(t) if constant_time_eq(t.as_bytes(), want.as_bytes()) => None,
            Some(_) => Some("wrong bearer token"),
        }
    }
}

/// Compare without stopping at the first difference, so the time taken
/// says nothing about how much of a guess was right (only its length).
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |d, (x, y)| d | (x ^ y)) == 0
}

/// Middleware: refuse writes without the [`WriteToken`] (401), when one is
/// configured. Use with `axum::middleware::from_fn_with_state`.
pub async fn token_writes(
    axum::extract::State(token): axum::extract::State<WriteToken>,
    req: Request,
    next: Next,
) -> Response {
    match token.refuse(req.method(), req.headers()) {
        Some(why) => (
            StatusCode::UNAUTHORIZED,
            [(header::WWW_AUTHENTICATE, "Bearer")],
            axum::Json(serde_json::json!({ "error": why })),
        )
            .into_response(),
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

    #[test]
    fn a_write_token_guards_writes_only() {
        let token = WriteToken::new("s3cret").unwrap();
        assert!(token.required());
        let none = headers(&[]);
        let good = headers(&[("authorization", "Bearer s3cret")]);
        let lower = headers(&[("authorization", "bearer  s3cret ")]);
        let wrong = headers(&[("authorization", "Bearer s3cre")]);
        let longer = headers(&[("authorization", "Bearer s3cret!")]);
        let basic = headers(&[("authorization", "Basic s3cret")]);
        for m in [Method::POST, Method::PUT, Method::PATCH, Method::DELETE] {
            assert!(token.refuse(&m, &none).is_some(), "{m} without a token");
            assert_eq!(token.refuse(&m, &good), None, "{m} with it");
            assert_eq!(token.refuse(&m, &lower), None, "{m}, scheme in any case");
            assert!(token.refuse(&m, &wrong).is_some(), "{m}, a prefix of it");
            assert!(token.refuse(&m, &longer).is_some(), "{m}, longer");
            assert!(token.refuse(&m, &basic).is_some(), "{m}, not bearer");
        }
        assert_eq!(token.refuse(&Method::GET, &none), None, "reads stay open");
        assert_eq!(token.refuse(&Method::HEAD, &wrong), None);
    }

    #[test]
    fn no_write_token_keeps_writes_open() {
        let open = WriteToken::open();
        assert!(!open.required());
        assert_eq!(open.refuse(&Method::POST, &headers(&[])), None);
        assert!(WriteToken::new("  ").is_err(), "an empty token is refused");
    }
}

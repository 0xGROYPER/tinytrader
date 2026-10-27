//! The `tt` binary's exit codes: 2 for usage, 128+n on signal n; and what a
//! signal during a submission says.

use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

fn tt(args: &[&str], console: Option<&str>) -> Command {
    let mut c = Command::new(env!("CARGO_BIN_EXE_tt"));
    c.args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    match console {
        Some(u) => c.env("TT_CONSOLE", u),
        None => c.env_remove("TT_CONSOLE"),
    };
    c
}

#[test]
fn usage_errors_exit_2() {
    let bad = tt(
        &["fetch", "databento", "--dataset", "X"],
        Some("http://127.0.0.1:9"),
    )
    .output()
    .unwrap();
    assert_eq!(bad.status.code(), Some(2));
    let said = String::from_utf8_lossy(&bad.stderr);
    assert!(said.starts_with("tt: "), "{said}");
    let unset = tt(&["quotes"], None).output().unwrap();
    assert_eq!(unset.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&unset.stderr).contains("TT_CONSOLE"));
}

#[tokio::test]
async fn a_signal_exits_128_plus_its_number() {
    for (signal, code) in [("-INT", 130), ("-TERM", 143)] {
        signalled(signal, code).await;
    }
}

async fn signalled(signal: &str, want: i32) {
    let asked = Arc::new(AtomicUsize::new(0));
    let seen = asked.clone();
    let app = axum::Router::new().route(
        "/api/jobs/jobs",
        axum::routing::get(move || {
            seen.fetch_add(1, Ordering::Relaxed);
            async {
                axum::Json(serde_json::json!({ "jobs": [{ "id": "J1", "state": "processing" }] }))
            }
        }),
    );
    let (code, said) = interrupt(app, &["jobs", "--follow", "J1"], &asked, signal).await;
    assert_eq!(code, Some(want), "{said}");
    assert!(!said.contains("tt jobs --follow"), "{said}");
}

/// Ctrl-C while a (free) submission is awaited says it goes on, and how to follow it.
#[tokio::test]
async fn a_signal_while_submitting_says_how_to_follow_it() {
    let quote = serde_json::json!({ "quote": "q-t-1", "state": "ready", "dataset": "OPRA.PILLAR",
        "schema": "cbbo-1m", "symbol": "SPY.OPT", "start": "2026-10-01", "end": "2026-10-01",
        "cost_usd": 0.0, "needs_approval": false, "expires_at": 4_000_000_000_u64,
        "requests": [{ "kind": "whole", "first_day": "2026-10-01", "last_day": "2026-10-01" }] });
    let asked = Arc::new(AtomicUsize::new(0));
    let seen = asked.clone();
    let q = quote.clone();
    let app = axum::Router::new()
        .route(
            "/api/jobs/price",
            axum::routing::post(move || async move { axum::Json(q) }),
        )
        .route(
            "/api/jobs/quotes/{id}",
            axum::routing::get(move || async move { axum::Json(quote) }),
        )
        .route(
            "/api/jobs/submit",
            axum::routing::post(move || {
                seen.fetch_add(1, Ordering::Relaxed);
                std::future::pending::<axum::Json<serde_json::Value>>()
            }),
        );
    let fetch = [
        "fetch",
        "databento",
        "--dataset",
        "OPRA.PILLAR",
        "--schema",
        "cbbo-1m",
        "--symbols",
        "SPY.OPT",
        "--start",
        "2026-10-01",
        "--end",
        "2026-10-01",
    ];
    let (code, said) = interrupt(app, &fetch, &asked, "-INT").await;
    assert_eq!(code, Some(130), "{said}");
    assert!(
        said.contains("q-t-1 that the console has begun runs to its end")
            && said.contains("`tt jobs --follow q-t-1`"),
        "{said}"
    );
}

/// `tt` with these arguments against `app`, sent `signal` once `asked`
/// counts a request: its exit code and stderr.
async fn interrupt(
    app: axum::Router,
    args: &[&str],
    asked: &AtomicUsize,
    signal: &str,
) -> (Option<i32>, String) {
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", l.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
    let child = tt(args, Some(&base)).spawn().unwrap();
    while asked.load(Ordering::Relaxed) == 0 {
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    let pid = child.id().to_string();
    assert!(
        Command::new("kill")
            .args([signal, &pid])
            .status()
            .unwrap()
            .success()
    );
    let out = tokio::task::spawn_blocking(move || child.wait_with_output().unwrap())
        .await
        .unwrap();
    (
        out.status.code(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

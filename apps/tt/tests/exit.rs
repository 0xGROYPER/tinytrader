//! The `tt` binary's exit codes: 2 for usage, 128+n on signal n.

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
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", l.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
    let mut child = tt(&["jobs", "--follow", "J1"], Some(&base))
        .spawn()
        .unwrap();
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
    let code = tokio::task::spawn_blocking(move || child.wait().unwrap())
        .await
        .unwrap();
    assert_eq!(code.code(), Some(want));
}

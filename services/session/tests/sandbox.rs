//! A strategy the session starts itself, with and without the sandbox. The same
//! probes run in both: read a file the run doesn't allow, and connect to a port that
//! stands in for the store. Unsandboxed, both work; sandboxed (macOS), neither does,
//! and the strategy still runs the session to the end.
use serde_json::Value;
use session::Session;
use std::net::TcpListener;
use std::path::{Path, PathBuf};

fn root(name: &str) -> PathBuf {
    let p = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("sandbox-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&p);
    std::fs::create_dir_all(&p).unwrap();
    p
}

/// Runs the fixture with `tinytrader-session watch` as the strategy behind the probes.
fn run(name: &str, sandbox: &str) -> (session::Outcome, String, Value, bool) {
    let base = root(name);
    session::fixture::write(&base).unwrap();
    let secret = base.join("secret.txt");
    std::fs::write(&secret, "the future").unwrap();
    let store = TcpListener::bind("127.0.0.1:0").unwrap();
    store.set_nonblocking(true).unwrap();
    let bin = PathBuf::from(env!("CARGO_BIN_EXE_tinytrader-session"));
    let script = format!(
        "cat {} >/dev/null 2>&1 && echo READ-SECRET || echo NO-SECRET; \
         (exec 3<>/dev/tcp/127.0.0.1/{}) 2>/dev/null && echo NET-STORE || echo NO-NET; \
         exec {} watch",
        secret.display(),
        store.local_addr().unwrap().port(),
        bin.display()
    );
    let allow = bin.parent().unwrap().canonicalize().unwrap();
    let mut config = std::fs::read_to_string(base.join("session.toml")).unwrap();
    config.push_str(&format!(
        "\n[strategy]\ncommand = [\"/bin/bash\", \"-c\", {script:?}]\nsandbox = \"{sandbox}\"\nallow_read = [{:?}]\n",
        allow.display().to_string()
    ));
    let dir = base.join("run");
    let s = Session::start(&config, &dir, [127, 0, 0, 1].into()).unwrap();
    let o = s.run().unwrap();
    let out = std::fs::read_to_string(dir.join("strategy.out")).unwrap();
    let rec: Value =
        serde_json::from_str(&std::fs::read_to_string(dir.join("run.json")).unwrap()).unwrap();
    let reached = store.accept().is_ok();
    (o, out, rec, reached)
}

#[test]
fn an_unsandboxed_strategy_can_read_and_connect() {
    let (o, out, rec, reached) = run("off", "off");
    assert_eq!(o.status, "complete", "{o:?} {out}");
    assert!(
        out.contains("READ-SECRET") && out.contains("NET-STORE"),
        "{out}"
    );
    assert!(reached);
    assert_eq!(rec["strategy_process"]["sandbox"], "off");
    assert_eq!(rec["strategy_process"]["exit"], 0);
}

#[cfg(target_os = "macos")]
#[test]
fn a_sandboxed_strategy_can_only_reach_its_session() {
    let (o, out, rec, reached) = run("macos", "macos");
    assert_eq!(o.status, "complete", "{o:?} {out}");
    assert!(out.contains("NO-SECRET") && out.contains("NO-NET"), "{out}");
    assert!(
        !reached,
        "the sandboxed strategy reached the stand-in store"
    );
    assert!(out.contains("complete"), "{out}");
    let sb = &rec["strategy_process"]["sandbox"];
    assert_eq!(sb["kind"], "macos-sandbox-exec");
    assert_eq!(sb["profile_sha256"].as_str().unwrap().len(), 64);
    assert_eq!(rec["strategy_process"]["exit"], 0);
}

#[test]
fn a_strategy_that_exits_at_once_leaves_the_run_incomplete() {
    let base = root("exits");
    session::fixture::write(&base).unwrap();
    let mut config = std::fs::read_to_string(base.join("session.toml")).unwrap();
    config = config.replace("ready_timeout_s = 20", "ready_timeout_s = 2");
    config.push_str("\n[strategy]\ncommand = [\"/bin/sh\", \"-c\", \"exit 3\"]\n");
    let s = Session::start(&config, &base.join("run"), [127, 0, 0, 1].into()).unwrap();
    let o = s.run().unwrap();
    assert_eq!(o.status, "incomplete");
    let rec: Value =
        serde_json::from_str(&std::fs::read_to_string(base.join("run/run.json")).unwrap()).unwrap();
    assert_eq!(rec["strategy_process"]["exit"], 3);
    let _ = Path::new("");
}

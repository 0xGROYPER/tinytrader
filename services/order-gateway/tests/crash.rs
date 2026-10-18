//! SIGKILL of the real binary between `admitted` and `sent`, and between `sent` and the
//! outcome; restart resolves without resending; torn tails, corruption and the lock.
mod support;
use order_gateway::client::Client;
use std::io::{BufRead, BufReader};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};
use support::*;

fn spawn(root: &Path, script: &str, stall: bool) -> (Child, std::net::SocketAddr) {
    let tok = root.join("t.token");
    std::fs::write(&tok, TOKEN).unwrap();
    std::fs::set_permissions(&tok, std::os::unix::fs::PermissionsExt::from_mode(0o600)).unwrap();
    std::fs::write(root.join("script.json"), script).unwrap();
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_order-gateway"));
    cmd.args([
        "--account",
        "sim-1",
        "--broker",
        "sim",
        "--listen",
        "127.0.0.1:0",
    ])
    .arg("--state")
    .arg(root.join("state"))
    .arg("--client")
    .arg(format!("reactor,trade,wse-,{}", tok.display()))
    .args(["--underlyings", "SPY"])
    .arg("--sim-script")
    .arg(root.join("script.json"))
    .stderr(Stdio::piped());
    if stall {
        cmd.arg("--test-stall-after-admitted");
    }
    let mut child = cmd.spawn().unwrap();
    let mut line = String::new();
    BufReader::new(child.stderr.take().unwrap())
        .read_line(&mut line)
        .unwrap();
    let addr = line.trim().rsplit(' ').next().unwrap().parse().unwrap();
    (child, addr)
}

/// Submits one intent and SIGKILLs the process once `phase` is journaled.
fn kill_at(root: &Path, script: &str, stall: bool, phase: &str) {
    let (mut child, addr) = spawn(root, script, stall);
    let mut c = Client::connect(addr, "reactor", TOKEN, None, false).unwrap();
    c.send(
        "submit",
        &format!(r#"{{"request_id":"r1","intent":{}}}"#, buy("wse-1")),
    )
    .unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    let marker = format!(r#""phase":"{phase}""#);
    while !journal(&root.join("state"))
        .iter()
        .any(|l| l.contains(&marker))
    {
        assert!(Instant::now() < deadline, "no {phase} line");
        std::thread::sleep(Duration::from_millis(10));
    }
    child.kill().unwrap(); // SIGKILL
    child.wait().unwrap();
}

#[test]
fn kill_between_admitted_and_sent_never_sends() {
    let root = root("crash-admitted");
    let script = format!(r#"{{"clock_ts":{T0}}}"#);
    kill_at(&root, &script, true, "admitted");
    let gw = scripted(config(root.join("state")), &script);
    let mut c = Conn::open(&gw, "reactor", Some(0));
    let o = c.event("submit", |v| v["phase"] == "outcome");
    assert_eq!(o["outcome"]["reason"], "not_ready");
    assert_eq!(o["outcome"]["detail"]["interrupted"], "admitted");
    assert_eq!(sends(&mut c, "a1"), 0);
    assert_eq!(
        c.submit("r2", &buy("wse-1"))["outcome"],
        "duplicate",
        "the id stays reserved"
    );
    gw.stop().unwrap();
    let lines = journal(&root.join("state"));
    assert_eq!(
        lines
            .iter()
            .filter(|l| l.contains(r#""phase":"sent""#))
            .count(),
        0
    );
}

#[test]
fn kill_between_sent_and_outcome_resolves_by_lookup() {
    let root = root("crash-sent");
    let script = format!(r#"{{"clock_ts":{T0},"rules":[{{"nth":1,"submit":"stall"}}]}}"#);
    kill_at(&root, &script, false, "sent");
    // The broker accepted the order before the crash; the restarted simulator holds it.
    let held = format!(
        r#"{{"clock_ts":{T0},"orders":[{{"client_order_id":"wse-1","symbol":"{CALL}","side":"buy","qty":1,"limit_price":"1.25"}}]}}"#
    );
    let state = root.join("state");
    let gw = scripted(config(state.clone()), &held);
    let mut c = Conn::open(&gw, "reactor", Some(0));
    let o = c.event("submit", |v| v["phase"] == "outcome");
    assert_eq!(
        (
            o["outcome"]["outcome"].as_str(),
            o["outcome"]["reason"].as_str()
        ),
        (Some("unknown"), Some("connection"))
    );
    let r = c.event("resolution", |_| true);
    assert_eq!(r["outcome"], "found");
    assert_eq!(c.order_event("wse-1", "new")["source"], "rest_reconcile");
    assert_eq!(sends(&mut c, "a1"), 0, "never resent after the restart");
    gw.stop().unwrap();

    // A torn final line is the only repair; the chain then verifies.
    let file = std::fs::read_dir(state.join("journal"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let mut text = std::fs::read_to_string(&file).unwrap();
    std::fs::write(&file, format!("{text}{{\"schema\":\"tinytrader.gat")).unwrap();
    let gw = scripted(config(state.clone()), &held);
    assert!(
        order_gateway::start(
            config(state.clone()),
            order_gateway::SimBroker::scripted(&held).unwrap()
        )
        .err()
        .unwrap()
        .contains("lock")
    );
    gw.stop().unwrap();
    let repaired = std::fs::read_to_string(&file).unwrap();
    assert!(
        repaired.starts_with(&text)
            && repaired.ends_with('\n')
            && !repaired.contains("tinytrader.gat\n")
    );

    // Corruption in the middle refuses to start and leaves the file untouched.
    text = repaired.replacen(r#""phase":"sent""#, r#""phase":"SENT""#, 1);
    std::fs::write(&file, &text).unwrap();
    let err = order_gateway::start(
        config(state),
        order_gateway::SimBroker::scripted(&held).unwrap(),
    )
    .err()
    .unwrap();
    assert!(err.contains("hash chain mismatch"), "{err}");
    assert_eq!(std::fs::read_to_string(&file).unwrap(), text);
}

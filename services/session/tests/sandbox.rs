//! A strategy the session starts itself, with and without the sandbox. The same
//! probes run in both: read a file the run doesn't allow, and connect to a port that
//! stands in for the store. Unsandboxed, both work; sandboxed (macOS), neither does,
//! and the strategy still runs the session to the end.
use serde_json::Value;
use session::Session;
use std::net::TcpListener;
use std::path::{Path, PathBuf};

/// Whether the probes can run here: they need bash and the base tools on the
/// strategy's `PATH` (`/usr/bin:/bin`), which a Nix build sandbox doesn't have.
/// With `TT_REQUIRE_SANDBOX` set (CI's macOS job), they must.
fn probes_can_run() -> bool {
    let ok = ["/bin/bash", "/bin/cat", "/bin/chmod"]
        .iter()
        .all(|p| Path::new(p).exists() || Path::new(&p.replace("/bin/", "/usr/bin/")).exists());
    if !ok {
        assert!(
            std::env::var_os("TT_REQUIRE_SANDBOX").is_none(),
            "TT_REQUIRE_SANDBOX is set, but there are no bash and base tools under /usr/bin:/bin"
        );
        eprintln!("skipped: no bash and base tools under /usr/bin:/bin here");
    }
    ok
}

/// Whether `sandbox-exec` works here for the session's binary: it can't nest in
/// another sandbox (a Nix build's, on a machine that sandboxes builds), and inside
/// any Nix build the binary links libraries from the Nix store, which the
/// strategy's profile doesn't let it read.
#[cfg(target_os = "macos")]
fn sandbox_exec_works() -> bool {
    // A Nix build sets NIX_BUILD_TOP; a `nix develop` shell sets it too, but also
    // IN_NIX_SHELL, and its binaries run sandboxed.
    let nix_build =
        std::env::var_os("NIX_BUILD_TOP").is_some() && std::env::var_os("IN_NIX_SHELL").is_none();
    let ok = !nix_build
        && std::process::Command::new("/usr/bin/sandbox-exec")
            .args(["-p", "(version 1)(allow default)", "/usr/bin/true"])
            .status()
            .is_ok_and(|s| s.success());
    if !ok {
        assert!(
            std::env::var_os("TT_REQUIRE_SANDBOX").is_none(),
            "TT_REQUIRE_SANDBOX is set, but sandbox-exec can't run the session here"
        );
        eprintln!("skipped: sandbox-exec can't run the session here (a Nix build?)");
    }
    ok
}

fn root(name: &str) -> PathBuf {
    let p = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("sandbox-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&p);
    std::fs::create_dir_all(&p).unwrap();
    p
}

/// Runs the fixture with `tinytrader-session watch` as the strategy behind the
/// probes. `escapes` adds the probes that would act outside the sandbox if it
/// failed (a launchd job, opening a file with Launch Services), each harmless.
fn run(name: &str, sandbox: &str, escapes: bool) -> (session::Outcome, String, Value, bool) {
    let base = root(name);
    session::fixture::write(&base).unwrap();
    let base = base.canonicalize().unwrap();
    let secret = base.join("secret.txt");
    std::fs::write(&secret, "the future").unwrap();
    let marker = base.join("opened");
    let command = base.join("x.command");
    std::fs::write(&command, format!("#!/bin/sh\ntouch {}\n", marker.display())).unwrap();
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&command, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let store = TcpListener::bind("127.0.0.1:0").unwrap();
    store.set_nonblocking(true).unwrap();
    let bin = PathBuf::from(env!("CARGO_BIN_EXE_tinytrader-session"));
    let home = std::env::var("HOME").unwrap_or_default();
    let mut script = format!(
        "cat {} >/dev/null 2>&1 && echo READ-SECRET || echo NO-SECRET; \
         (exec 3<>/dev/tcp/127.0.0.1/{}) 2>/dev/null && echo NET-STORE || echo NO-NET; \
         [ -n \"$CARGO_MANIFEST_DIR\" ] && echo ENV-LEAK || echo NO-ENV-LEAK; ",
        secret.display(),
        store.local_addr().unwrap().port(),
    );
    if escapes {
        script.push_str(&format!(
            "ls {home} >/dev/null 2>&1 && echo READ-HOME || echo NO-HOME; \
             ls /Library/Logs >/dev/null 2>&1 && echo READ-LOGS || echo NO-LOGS; \
             kill -0 {pid} 2>/dev/null && echo SIGNAL || echo NO-SIGNAL; \
             [ -n \"$(dscacheutil -q host -a name localhost 2>/dev/null)\" ] && echo DNS || echo NO-DNS; \
             launchctl submit -l tt-sandbox-probe -- /usr/bin/true >/dev/null 2>&1; \
             /usr/bin/open -g {} >/dev/null 2>&1; ",
            command.display(),
            pid = std::process::id(),
        ));
    }
    script.push_str(&format!("exec {} watch", bin.display()));
    let allow = bin.parent().unwrap().canonicalize().unwrap();
    let mut config = std::fs::read_to_string(base.join("session.toml")).unwrap();
    config.push_str(&format!(
        "\n[strategy]\ncommand = [\"/bin/bash\", \"-c\", {script:?}]\nsandbox = \"{sandbox}\"\nallow_read = [{:?}]\n",
        allow.display().to_string()
    ));
    let dir = base.join("run");
    let s = Session::start(&config, &dir, [127, 0, 0, 1].into()).unwrap();
    let o = s.run().unwrap();
    let mut out = std::fs::read_to_string(dir.join("strategy.out")).unwrap();
    let rec: Value =
        serde_json::from_str(&std::fs::read_to_string(dir.join("run.json")).unwrap()).unwrap();
    let reached = store.accept().is_ok();
    if escapes {
        std::thread::sleep(std::time::Duration::from_secs(2));
        let job = std::process::Command::new("/bin/launchctl")
            .args(["remove", "tt-sandbox-probe"])
            .status();
        out.push_str(if job.is_ok_and(|s| s.success()) {
            "LAUNCHD-JOB\n"
        } else {
            "NO-LAUNCHD-JOB\n"
        });
        out.push_str(if marker.exists() {
            "OPENED\n"
        } else {
            "NOT-OPENED\n"
        });
    }
    (o, out, rec, reached)
}

#[test]
fn an_unsandboxed_strategy_can_read_and_connect() {
    if !probes_can_run() {
        return;
    }
    let (o, out, rec, reached) = run("off", "off", false);
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
    if !probes_can_run() || !sandbox_exec_works() {
        return;
    }
    let (o, out, rec, reached) = run("macos", "macos", true);
    assert_eq!(o.status, "complete", "{o:?} {out}");
    for want in [
        "NO-SECRET",
        "NO-NET",
        "NO-ENV-LEAK",
        "NO-HOME",
        "NO-LOGS",
        "NO-SIGNAL",
        "NO-DNS",
        "NO-LAUNCHD-JOB",
        "NOT-OPENED",
    ] {
        assert!(out.contains(want), "{want} missing: {out}");
    }
    assert!(
        !reached,
        "the sandboxed strategy reached the stand-in store"
    );
    assert!(out.contains("complete"), "{out}");
    let sb = &rec["strategy_process"]["sandbox"];
    assert_eq!(sb["kind"], "macos-sandbox-exec");
    assert_eq!(sb["profile_sha256"].as_str().unwrap().len(), 64);
    assert!(
        sb["profile"]
            .as_str()
            .unwrap()
            .starts_with("(version 1)\n(deny default)")
    );
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

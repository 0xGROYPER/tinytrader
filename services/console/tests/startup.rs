//! The console refuses to start with a `CONSOLE_LOGIN_HEADER` that isn't a
//! valid HTTP header name, or with `CONSOLE_LIVE_NEEDS_TOKEN=false` and a
//! `CONSOLE_LISTEN` that isn't a loopback address (unless
//! `CONSOLE_TRUST_NETWORK` is `true`).

use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// Start the console with `vars` and wait for it to exit (or `None` once it
/// has kept running for `wait`, when it is killed): the exit code and stderr.
fn run(vars: &[(&str, &str)], wait: Duration) -> Option<(Option<i32>, String)> {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_console"));
    cmd.env("CONSOLE_TOPOLOGY", "nonexistent-topology.json")
        .env_remove("CONSOLE_READ_LISTEN")
        .env_remove("CONSOLE_LIVE_NEEDS_TOKEN")
        .env_remove("CONSOLE_TRUST_NETWORK")
        .env_remove("TINYTRADER_LISTEN")
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    for (k, v) in vars {
        cmd.env(k, v);
    }
    let mut child = cmd.spawn().unwrap();
    let deadline = Instant::now() + wait;
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if Instant::now() > deadline {
            child.kill().unwrap();
            let _ = child.wait();
            return None;
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    let out = child.wait_with_output().unwrap();
    Some((
        status.code(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    ))
}

#[test]
fn an_invalid_login_header_is_refused_at_startup() {
    for bad in ["Remote User", "Remote-User:", "Host"] {
        let (code, stderr) = run(
            &[
                ("CONSOLE_LISTEN", "127.0.0.1:0"),
                ("CONSOLE_LOGIN_HEADER", bad),
            ],
            Duration::from_secs(20),
        )
        .unwrap_or_else(|| panic!("the console started with CONSOLE_LOGIN_HEADER={bad:?}"));
        assert_eq!(code, Some(2), "{bad:?}: {stderr}");
        assert!(stderr.contains("CONSOLE_LOGIN_HEADER"), "{bad:?}: {stderr}");
    }
}

#[test]
fn no_live_token_needs_a_loopback_listen_address() {
    for bad in ["0.0.0.0:0", "[::]:0", "localhost:0"] {
        let (code, stderr) = run(
            &[
                ("CONSOLE_LISTEN", bad),
                ("CONSOLE_LIVE_NEEDS_TOKEN", "false"),
            ],
            Duration::from_secs(20),
        )
        .unwrap_or_else(|| panic!("the console started on {bad} with no live token"));
        assert_eq!(code, Some(2), "{bad}: {stderr}");
        assert!(
            stderr.contains("CONSOLE_LIVE_NEEDS_TOKEN"),
            "{bad}: {stderr}"
        );
        assert!(
            stderr.contains("127.0.0.1") && stderr.contains("[::1]"),
            "{bad}: {stderr}"
        );
    }
    // On loopback it starts (and keeps running until killed).
    for ok in ["127.0.0.1:0", "[::1]:0"] {
        assert!(
            run(
                &[
                    ("CONSOLE_LISTEN", ok),
                    ("CONSOLE_LIVE_NEEDS_TOKEN", "false")
                ],
                Duration::from_secs(2),
            )
            .is_none(),
            "{ok}"
        );
    }
    // With the token needed (the default), a non-loopback address still starts.
    assert!(run(&[("CONSOLE_LISTEN", "0.0.0.0:0")], Duration::from_secs(2)).is_none());
}

#[test]
fn trusting_the_network_lifts_the_loopback_requirement() {
    for listen in ["0.0.0.0:0", "[::]:0"] {
        assert!(
            run(
                &[
                    ("CONSOLE_LISTEN", listen),
                    ("CONSOLE_LIVE_NEEDS_TOKEN", "false"),
                    ("CONSOLE_TRUST_NETWORK", "true"),
                ],
                Duration::from_secs(2),
            )
            .is_none(),
            "{listen}"
        );
    }
    // Anything but an explicit on still refuses.
    for off in ["false", "", "maybe"] {
        let (code, stderr) = run(
            &[
                ("CONSOLE_LISTEN", "0.0.0.0:0"),
                ("CONSOLE_LIVE_NEEDS_TOKEN", "false"),
                ("CONSOLE_TRUST_NETWORK", off),
            ],
            Duration::from_secs(20),
        )
        .unwrap_or_else(|| panic!("the console started with CONSOLE_TRUST_NETWORK={off:?}"));
        assert_eq!(code, Some(2), "{off:?}: {stderr}");
    }
}

//! The console refuses to start with a `CONSOLE_LOGIN_HEADER` that isn't a
//! valid HTTP header name.

use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

#[test]
fn an_invalid_login_header_is_refused_at_startup() {
    for bad in ["Remote User", "Remote-User:", "Host"] {
        let mut child = Command::new(env!("CARGO_BIN_EXE_console"))
            .env("CONSOLE_LISTEN", "127.0.0.1:0")
            .env("CONSOLE_LOGIN_HEADER", bad)
            .env("CONSOLE_TOPOLOGY", "nonexistent-topology.json")
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(20);
        let status = loop {
            if let Some(status) = child.try_wait().unwrap() {
                break status;
            }
            if Instant::now() > deadline {
                child.kill().unwrap();
                panic!("the console started with CONSOLE_LOGIN_HEADER={bad:?}");
            }
            std::thread::sleep(Duration::from_millis(20));
        };
        let out = child.wait_with_output().unwrap();
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert_eq!(status.code(), Some(2), "{bad:?}: {stderr}");
        assert!(stderr.contains("CONSOLE_LOGIN_HEADER"), "{bad:?}: {stderr}");
    }
}

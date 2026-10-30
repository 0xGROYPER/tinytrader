//! A started strategy's confinement on macOS: a `sandbox-exec` profile that
//! denies by default (0026 §5's rule that an actor has no data or network of its
//! own, made enforceable).
//!
//! The strategy may:
//! - read the system's folders and the folders the run allows (its code, its
//!   interpreter);
//! - read and write its own scratch folder in the run folder (`TMPDIR`);
//! - read any file's metadata, though not its contents;
//! - connect to the session's control and data addresses and its venues'
//!   gateways, and nothing else.
//!
//! Everything else is off the list: the store, the caches, home, other hosts.
//! `sandbox-exec` is deprecated by Apple but works on current macOS; a Linux
//! container needs the session on unix sockets, a later step.
use std::fmt::Write as _;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};

/// Read-only system folders every process needs.
const SYSTEM: &[&str] = &[
    "/System",
    "/usr",
    "/bin",
    "/sbin",
    "/Library",
    "/private/etc",
    "/private/var/db/dyld",
    "/private/var/db/timezone",
    "/dev",
    "/nix/store",
];

/// SBPL's string literal.
fn quoted(p: &Path) -> String {
    format!("{:?}", p.to_string_lossy())
}

/// The profile for a strategy reading `allow_read`, writing `scratch`, and
/// connecting to `ports` on loopback.
#[must_use]
pub fn profile(allow_read: &[PathBuf], scratch: &Path, ports: &[SocketAddr]) -> String {
    let mut p = String::from(
        "(version 1)\n(deny default)\n\
         (allow process-exec process-fork signal sysctl-read mach-lookup ipc-posix-shm file-read-metadata)\n\
         (allow file-read* (literal \"/\")",
    );
    for s in SYSTEM {
        let _ = write!(p, " (subpath {})", quoted(Path::new(s)));
    }
    p.push_str(")\n");
    for a in allow_read {
        let _ = writeln!(p, "(allow file-read* (subpath {}))", quoted(a));
    }
    let _ = writeln!(
        p,
        "(allow file-read* file-write* (subpath {}))",
        quoted(scratch)
    );
    p.push_str("(allow file-write* (subpath \"/dev\"))\n");
    for a in ports {
        let _ = writeln!(
            p,
            "(allow network-outbound (remote ip \"localhost:{}\"))",
            a.port()
        );
    }
    p
}

#[cfg(test)]
mod tests {
    use super::profile;
    use std::path::{Path, PathBuf};

    #[test]
    fn the_profile_lists_only_what_is_allowed() {
        let p = profile(
            &[PathBuf::from("/opt/strategy")],
            Path::new("/runs/a/strategy-tmp"),
            &["127.0.0.1:5000".parse().unwrap()],
        );
        assert!(p.starts_with("(version 1)\n(deny default)"));
        assert!(p.contains("(allow file-read* (subpath \"/opt/strategy\"))"));
        assert!(p.contains("(allow file-read* file-write* (subpath \"/runs/a/strategy-tmp\"))"));
        assert!(p.contains("(remote ip \"localhost:5000\")"));
        assert!(!p.contains("network*"));
    }
}

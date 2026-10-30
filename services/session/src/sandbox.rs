//! A started strategy's confinement on macOS: a `sandbox-exec` profile that
//! denies by default (0026 §5's rule that an actor has no data or network of its
//! own, made enforceable).
//!
//! The strategy may:
//! - read the system's own folders (not `/Library`, whose logs can carry market
//!   data) and the folders the run allows (its code, its interpreter);
//! - read and write its own scratch folder in the run folder (`TMPDIR`, `HOME`);
//! - connect to the session's control and data addresses and its venues'
//!   gateways, and nothing else;
//! - signal only processes in its own sandbox.
//!
//! No Mach service lookups are allowed at all, so it can't ask an unsandboxed
//! service (Launch Services, launchd, background downloads, the DNS resolver) to
//! act for it. A run's `deny_read` folders are denied last, so they win over any
//! allowed folder that contains them. `/nix/store` is readable: it holds no
//! plaintext secrets in a sops deployment, but it is world-readable.
//!
//! `sandbox-exec` is deprecated by Apple but works on current macOS; a Linux
//! container needs the session on unix sockets, a later step.
use std::fmt::Write as _;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};

/// Read-only system folders a process needs.
const SYSTEM: &[&str] = &[
    "/System",
    "/usr",
    "/bin",
    "/sbin",
    "/private/etc",
    "/private/var/db/dyld",
    "/private/var/db/timezone",
    "/nix/store",
];

/// Folders a run may never allow, nor any folder containing one: a strategy
/// reading these could see held data or other runs.
pub const PROTECTED: &[&str] = &[
    "/",
    "/Users",
    "/Volumes",
    "/Library",
    "/private",
    "/private/var",
    "/var",
    "/var/lib/tinytrader",
    "/private/var/lib/tinytrader",
];

/// Per-user folders that hold caches, keys and configuration (and, in a
/// research setup, copies of held data): always denied, and never allowed by an
/// `allow_read` inside or containing them.
pub const PRIVATE_IN_HOME: &[&str] = &[
    ".cache", "Library", ".ssh", ".gnupg", ".config", ".aws", ".netrc",
];

/// The folders always denied: [`PRIVATE_IN_HOME`] under `home`, and the run's own.
#[must_use]
pub fn always_denied(home: Option<&Path>, deny_read: &[PathBuf]) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = home
        .into_iter()
        .flat_map(|h| PRIVATE_IN_HOME.iter().map(move |p| h.join(p)))
        .collect();
    out.extend(deny_read.iter().cloned());
    out
}

/// SBPL's string literal.
fn quoted(p: &Path) -> String {
    format!("{:?}", p.to_string_lossy())
}

/// The profile for a strategy reading `allow_read`, never `deny_read`, writing
/// `scratch`, and connecting to `ports` on loopback.
#[must_use]
pub fn profile(
    allow_read: &[PathBuf],
    deny_read: &[PathBuf],
    scratch: &Path,
    ports: &[SocketAddr],
) -> String {
    let mut p = String::from(
        "(version 1)\n(deny default)\n\
         (allow process-exec process-fork sysctl-read)\n\
         (allow signal (target same-sandbox))\n\
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
    p.push_str(
        "(allow file-read* file-write* (literal \"/dev/null\") (literal \"/dev/zero\") \
         (literal \"/dev/tty\") (literal \"/dev/dtracehelper\") (regex #\"^/dev/fd/\"))\n\
         (allow file-read* (literal \"/dev/random\") (literal \"/dev/urandom\"))\n",
    );
    for a in ports {
        let _ = writeln!(
            p,
            "(allow network-outbound (remote ip \"localhost:{}\"))",
            a.port()
        );
    }
    for d in deny_read {
        let _ = writeln!(p, "(deny file-read* (subpath {}))", quoted(d));
    }
    p
}

/// Checks the folders a run allows: each must exist (it is made canonical), and
/// none may be or contain a protected folder, the user's home, or a `deny_read`
/// folder.
///
/// # Errors
/// The first folder that fails, and why.
pub fn check_allow_read(
    allow_read: &[PathBuf],
    deny_read: &[PathBuf],
    home: Option<&Path>,
) -> Result<Vec<PathBuf>, String> {
    let canon = |p: &Path| p.canonicalize().unwrap_or_else(|_| p.to_owned());
    let mut guarded: Vec<PathBuf> = PROTECTED.iter().map(PathBuf::from).collect();
    guarded.extend(home.map(canon));
    let denied: Vec<PathBuf> = always_denied(home.map(canon).as_deref(), deny_read)
        .iter()
        .map(|d| canon(d))
        .collect();
    guarded.extend(denied.iter().cloned());
    let mut out = Vec::new();
    for a in allow_read {
        let c = a
            .canonicalize()
            .map_err(|e| format!("allow_read {}: {e}", a.display()))?;
        if let Some(g) = guarded.iter().find(|g| g.starts_with(&c)) {
            return Err(format!(
                "allow_read {} would let the strategy read {}",
                c.display(),
                g.display()
            ));
        }
        // Inside a denied folder, it would be denied anyway: say so now.
        if let Some(d) = denied.iter().find(|d| c.starts_with(d)) {
            return Err(format!(
                "allow_read {} is inside {}, which a strategy may never read",
                c.display(),
                d.display()
            ));
        }
        out.push(c);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::{check_allow_read, profile};
    use std::path::{Path, PathBuf};

    #[test]
    fn the_profile_lists_only_what_is_allowed() {
        let p = profile(
            &[PathBuf::from("/opt/strategy")],
            &[PathBuf::from("/opt/strategy/cache")],
            Path::new("/runs/a/strategy-tmp"),
            &["127.0.0.1:5000".parse().unwrap()],
        );
        assert!(p.starts_with("(version 1)\n(deny default)"));
        assert!(p.contains("(allow file-read* (subpath \"/opt/strategy\"))"));
        assert!(p.contains("(allow file-read* file-write* (subpath \"/runs/a/strategy-tmp\"))"));
        assert!(p.contains("(remote ip \"localhost:5000\")"));
        assert!(p.ends_with("(deny file-read* (subpath \"/opt/strategy/cache\"))\n"));
        for never in [
            "mach-lookup",
            "network*",
            "/Library",
            "ipc-posix",
            "file-read-metadata)",
        ] {
            assert!(!p.contains(never), "{never}");
        }
    }

    #[test]
    fn folders_that_contain_held_data_are_refused() {
        let tmp = std::env::temp_dir().canonicalize().unwrap();
        let code = tmp.join(format!("sbx-allow-{}", std::process::id()));
        std::fs::create_dir_all(code.join("cache")).unwrap();
        assert!(check_allow_read(std::slice::from_ref(&code), &[], None).is_ok());
        // A folder inside a home's caches is refused, as is the cache folder itself.
        let home = tmp.join(format!("sbx-home-{}", std::process::id()));
        std::fs::create_dir_all(home.join(".cache/uv")).unwrap();
        std::fs::create_dir_all(home.join("code")).unwrap();
        let e = check_allow_read(&[home.join(".cache")], &[], Some(&home)).unwrap_err();
        assert!(e.contains("would let") || e.contains("inside"), "{e}");
        let e = check_allow_read(&[home.join(".cache/uv")], &[], Some(&home)).unwrap_err();
        assert!(e.contains("may never read"), "{e}");
        assert!(check_allow_read(&[home.join("code")], &[], Some(&home)).is_ok());
        assert!(check_allow_read(&[PathBuf::from("/")], &[], None).is_err());
        assert!(check_allow_read(&[PathBuf::from("/Users")], &[], None).is_err());
        let e =
            check_allow_read(std::slice::from_ref(&code), &[code.join("cache")], None).unwrap_err();
        assert!(e.contains("would let the strategy read"), "{e}");
        let e = check_allow_read(std::slice::from_ref(&tmp), &[], Some(&code)).unwrap_err();
        assert!(e.contains("would let"), "{e}");
        assert!(check_allow_read(&[code.join("nope")], &[], None).is_err());
    }
}

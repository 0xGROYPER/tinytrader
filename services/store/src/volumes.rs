//! Drives the store may use (decision 0007): a drive is approved once, which
//! writes a small marker at its root, and from then on the store uses it
//! whenever it is mounted. The store never writes to an unmarked drive that
//! the configuration doesn't name, and never to the system disk.

use std::fs;
use std::os::unix::fs::MetadataExt as _;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::ingest::{Volume, safe};

/// The marker an approved drive carries at its root.
pub const MARKER: &str = ".tinytrader-volume";

/// What a look at the mount directory found: approved drives, and drives
/// that could be approved.
pub struct Found {
    pub marked: Vec<Volume>,
    pub candidates: Vec<Value>,
}

/// A mounted drive the store may consider: a real directory directly in
/// `dir` (not a link, such as macOS's link to the system disk) on a device
/// other than the system disk's.
fn eligible(path: &Path) -> bool {
    let (Ok(meta), Ok(system)) = (fs::symlink_metadata(path), fs::metadata("/")) else {
        return false;
    };
    meta.is_dir() && !meta.file_type().is_symlink() && meta.dev() != system.dev()
}

/// The drives mounted in `dir` (such as `/Volumes`): those with a marker,
/// and the rest as candidates with their size and free space.
#[must_use]
pub fn discover(dir: &Path) -> Found {
    let mut found = Found {
        marked: Vec::new(),
        candidates: Vec::new(),
    };
    let Ok(entries) = fs::read_dir(dir) else {
        return found;
    };
    let mut paths: Vec<PathBuf> = entries.flatten().map(|e| e.path()).collect();
    paths.sort();
    for path in paths.into_iter().filter(|p| eligible(p)) {
        let marker = path.join(MARKER);
        if let Ok(text) = fs::read_to_string(&marker) {
            match serde_json::from_str::<Value>(&text)
                .ok()
                .and_then(|v| v["name"].as_str().map(str::to_owned))
            {
                Some(name) if safe(&name) => found.marked.push(Volume {
                    name,
                    path,
                    writable: true,
                }),
                _ => eprintln!(
                    "store: {} is not a readable marker; ignoring that drive",
                    marker.display()
                ),
            }
            continue;
        }
        found.candidates.push(json!({
            "path": path,
            "label": path.file_name().map(|n| n.to_string_lossy().into_owned()),
            "total_bytes": fs4::total_space(&path).ok(),
            "available_bytes": fs4::available_space(&path).ok(),
        }));
    }
    found
}

/// Approve the drive at `path` (directly in `dir`) for the store as `name`:
/// write its marker, never over an existing one. This is the only write the
/// store makes to a drive before it is approved.
///
/// # Errors
/// If the path isn't an eligible drive in `dir`, already carries a marker,
/// or the name is unsafe or taken.
pub fn approve(dir: &Path, path: &Path, name: &str, taken: &[String]) -> Result<Volume, String> {
    if path.parent() != Some(dir) || !eligible(path) {
        return Err(format!(
            "{} is not a drive mounted in {}",
            path.display(),
            dir.display()
        ));
    }
    if !safe(name) {
        return Err(format!(
            "{name:?} is not a usable name (letters, digits, '.', '_' and '-')"
        ));
    }
    if taken.iter().any(|t| t == name) {
        return Err(format!("the store already has a drive called {name}"));
    }
    let marker =
        json!({ "name": name, "approved_at": tinytrader_observe::unix_seconds(), "path": path });
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path.join(MARKER))
        .map_err(|e| format!("{}: {e}", path.join(MARKER).display()))?;
    std::io::Write::write_all(&mut file, format!("{marker:#}\n").as_bytes())
        .map_err(|e| e.to_string())?;
    file.sync_all().map_err(|e| e.to_string())?;
    Ok(Volume {
        name: name.to_owned(),
        path: path.to_path_buf(),
        writable: true,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    // Test directories sit on the system disk, so `eligible` rejects them;
    // these tests cover what doesn't depend on a second device.
    #[test]
    fn refuses_the_system_disk_and_paths_outside_the_mount_directory() {
        let d = std::env::temp_dir().join(format!("tinytrader-volumes-{}", std::process::id()));
        let drive = d.join("Drive");
        fs::create_dir_all(&drive).unwrap();
        let err = approve(&d, &drive, "drive", &[]).unwrap_err();
        assert!(err.contains("not a drive"), "{err}");
        assert!(!drive.join(MARKER).exists());
        let err = approve(&d, Path::new("/tmp"), "drive", &[]).unwrap_err();
        assert!(err.contains("not a drive"), "{err}");
        assert!(discover(&d).marked.is_empty() && discover(&d).candidates.is_empty());
        let _ = fs::remove_dir_all(&d);
    }
}

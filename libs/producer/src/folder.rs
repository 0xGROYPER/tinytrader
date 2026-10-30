//! Holdings in a local folder, read-only:
//!
//! - a `databento-jobs` cache job (`POST /cache/{job}`): the batch's files,
//!   `metadata.json` and `manifest.json` as Databento sent them, read only
//!   once `COMPLETE` says the copy is whole. Each file's hash is the
//!   manifest's, and is checked.
//! - a folder in the store's layout, such as a copy of a volume:
//!   `<Provider>/<DATASET>/<symbol>/<schema>/<day>.dbn.zst`, or a folder per
//!   day holding its DBN files (the older layout). No hashes are stated, so
//!   they are recorded, not checked.
use crate::{Day, FileRef, Holdings, Query};
use serde_json::Value;
use std::collections::HashMap;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

enum Kind {
    /// A cache job: day (YYYYMMDD) → (file name, hash).
    Job {
        id: String,
        dataset: String,
        schema: String,
        symbols: Vec<String>,
        files: HashMap<String, (String, Option<String>)>,
    },
    /// A volume in the store's layout.
    Layout,
}

/// A local folder of DBN days.
pub struct Folder {
    root: PathBuf,
    kind: Kind,
}

fn is_dbn(name: &str) -> bool {
    name.ends_with(".dbn.zst") || name.ends_with(".dbn")
}

fn read_json(path: &Path) -> Result<Value, String> {
    let text = fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))
}

impl Folder {
    /// A finished `databento-jobs` cache job.
    ///
    /// # Errors
    /// It has no `COMPLETE` marker yet, or its metadata or manifest can't be read.
    pub fn job(root: &Path) -> Result<Self, String> {
        let id = root
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        if !root.join("COMPLETE").is_file() {
            return Err(format!(
                "job {id} isn't complete yet: no COMPLETE marker in {}",
                root.display()
            ));
        }
        let meta = read_json(&root.join("metadata.json"))?;
        let q = meta.get("query").unwrap_or(&meta);
        let text = |k: &str| q[k].as_str().unwrap_or_default().to_owned();
        let symbols = match &q["symbols"] {
            Value::Array(a) => a
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect(),
            Value::String(s) => s.split(',').map(str::to_owned).collect(),
            _ => Vec::new(),
        };
        let manifest = read_json(&root.join("manifest.json"))?;
        let hashes: HashMap<&str, &str> = manifest["files"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|f| Some((f["filename"].as_str()?, f["hash"].as_str()?)))
                    .collect()
            })
            .unwrap_or_default();
        let mut files = HashMap::new();
        for e in fs::read_dir(root).map_err(|e| format!("{}: {e}", root.display()))? {
            let name = e
                .map_err(|e| e.to_string())?
                .file_name()
                .to_string_lossy()
                .into_owned();
            if !is_dbn(&name) {
                continue;
            }
            // `opra-pillar-20261027.cbbo-1m.dbn.zst`: the day is the 8 digits
            // before the first dot.
            let stem = name.split('.').next().unwrap_or_default();
            let day = stem.rsplit('-').next().unwrap_or_default();
            if day.len() == 8 && day.bytes().all(|b| b.is_ascii_digit()) {
                let sha = hashes
                    .get(name.as_str())
                    .map(|h| h.trim_start_matches("sha256:").to_owned());
                files.insert(day.to_owned(), (name, sha));
            }
        }
        Ok(Self {
            root: root.to_owned(),
            kind: Kind::Job {
                id,
                dataset: text("dataset"),
                schema: text("schema"),
                symbols,
                files,
            },
        })
    }

    /// A folder in the store's layout (a volume's root).
    #[must_use]
    pub fn layout(root: &Path) -> Self {
        Self {
            root: root.to_owned(),
            kind: Kind::Layout,
        }
    }

    /// The schema folder for `q`, matching the provider folder's name in any case.
    fn schema_dir(&self, q: &Query) -> Option<PathBuf> {
        let provider = fs::read_dir(&self.root).ok()?.flatten().find(|e| {
            e.file_name()
                .to_string_lossy()
                .eq_ignore_ascii_case(&q.provider)
        })?;
        let dir = provider
            .path()
            .join(&q.dataset)
            .join(&q.symbol)
            .join(&q.schema);
        dir.is_dir().then_some(dir)
    }
}

impl Holdings for Folder {
    fn describe(&self) -> String {
        match &self.kind {
            Kind::Job { id, .. } => format!("job {id} at {}", self.root.display()),
            Kind::Layout => format!("folder {}", self.root.display()),
        }
    }

    fn plan(&self, q: &Query, days: &[String]) -> Result<Vec<Day>, String> {
        match &self.kind {
            Kind::Job {
                id,
                dataset,
                schema,
                symbols,
                files,
            } => {
                if !dataset.eq_ignore_ascii_case(&q.dataset)
                    || schema != &q.schema
                    || !symbols.iter().any(|s| s == &q.symbol)
                {
                    return Err(format!(
                        "job {id} holds {dataset} {schema} {}, not {} {} {}",
                        symbols.join(","),
                        q.dataset,
                        q.schema,
                        q.symbol
                    ));
                }
                Ok(days
                    .iter()
                    .map(|d| match files.get(&d.replace('-', "")) {
                        Some((name, sha)) => Day::Held {
                            files: vec![FileRef {
                                name: name.clone(),
                                sha256: sha.clone(),
                            }],
                            partial: false,
                        },
                        None => Day::Missing {
                            reason: format!("not in job {id}"),
                        },
                    })
                    .collect())
            }
            Kind::Layout => {
                let Some(dir) = self.schema_dir(q) else {
                    let reason = format!(
                        "no {} {} {} {} folder",
                        q.provider, q.dataset, q.symbol, q.schema
                    );
                    return Ok(days
                        .iter()
                        .map(|_| Day::Missing {
                            reason: reason.clone(),
                        })
                        .collect());
                };
                let rel = |p: &Path| {
                    p.strip_prefix(&self.root)
                        .unwrap_or(p)
                        .to_string_lossy()
                        .into_owned()
                };
                Ok(days
                    .iter()
                    .map(|d| {
                        let mut files: Vec<PathBuf> = ["dbn.zst", "dbn"]
                            .iter()
                            .map(|ext| dir.join(format!("{d}.{ext}")))
                            .filter(|p| p.is_file())
                            .take(1)
                            .collect();
                        if files.is_empty()
                            && let Ok(rd) = fs::read_dir(dir.join(d))
                        {
                            files = rd
                                .flatten()
                                .map(|e| e.path())
                                .filter(|p| p.is_file() && is_dbn(&p.to_string_lossy()))
                                .collect();
                            files.sort();
                        }
                        // An empty file is padding, not a day.
                        files.retain(|p| fs::metadata(p).is_ok_and(|m| m.len() > 0));
                        if files.is_empty() {
                            Day::Missing {
                                reason: "not in the folder".into(),
                            }
                        } else {
                            Day::Held {
                                files: files
                                    .iter()
                                    .map(|p| FileRef {
                                        name: rel(p),
                                        sha256: None,
                                    })
                                    .collect(),
                                partial: false,
                            }
                        }
                    })
                    .collect())
            }
        }
    }

    fn open(&self, file: &FileRef) -> Result<Box<dyn Read + Send>, String> {
        let path = self.root.join(&file.name);
        if !path.starts_with(&self.root) || file.name.contains("..") {
            return Err(format!("{}: outside the folder", file.name));
        }
        let f = fs::File::open(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        Ok(Box::new(f))
    }
}

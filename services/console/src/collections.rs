//! Document collections: named sets of Markdown folders the console shows,
//! such as `Docs` and `Research`.
//!
//! Configured with `CONSOLE_COLLECTIONS`, for example
//! `Docs=/srv/docs/ops,/srv/docs/platform;Research=/srv/research`.
//! Each folder is scanned for `.md` files (hidden entries skipped). Documents
//! are addressed by an index into that scan, never by a path from the
//! request, so nothing outside the configured folders can be read. Raw HTML
//! in Markdown is shown as text.

use std::path::{Path, PathBuf};

use pulldown_cmark::{Event, Options, Parser, html};
use serde_json::{Value, json};

pub struct Collection {
    pub id: String,
    pub name: String,
    roots: Vec<PathBuf>,
}

/// Parse `Name=dir,dir;Name=dir`.
pub fn parse(spec: &str) -> Vec<Collection> {
    spec.split(';')
        .filter_map(|part| {
            let (name, dirs) = part.split_once('=')?;
            let name = name.trim();
            if name.is_empty() {
                return None;
            }
            Some(Collection {
                id: name
                    .to_lowercase()
                    .replace(|c: char| !c.is_ascii_alphanumeric(), "-"),
                name: name.to_owned(),
                roots: dirs
                    .split(',')
                    .map(str::trim)
                    .filter(|d| !d.is_empty())
                    .map(PathBuf::from)
                    .collect(),
            })
        })
        .collect()
}

struct Doc {
    /// Folder label: the root's name, then any subfolders.
    group: String,
    /// Path shown and used for links: `<root name>/<relative path>`.
    path: String,
    file: PathBuf,
}

impl Collection {
    fn docs(&self) -> Vec<Doc> {
        let mut out = Vec::new();
        for root in &self.roots {
            let label = root
                .file_name()
                .map_or_else(|| "docs".into(), |n| n.to_string_lossy().into_owned());
            walk(root, root, &label, 0, &mut out);
        }
        out.sort_by(|a, b| a.path.cmp(&b.path));
        out
    }

    /// The collection's documents: `{ id, name, docs: [{ index, path, group, title }] }`.
    pub fn index(&self) -> Value {
        let docs: Vec<Value> = self
            .docs()
            .iter()
            .enumerate()
            .map(|(i, d)| json!({ "index": i, "path": d.path, "group": d.group, "title": title(&d.file) }))
            .collect();
        json!({ "id": self.id, "name": self.name, "docs": docs })
    }

    /// One document rendered to HTML: `{ index, path, title, html }`.
    pub fn render(&self, index: usize) -> Option<Value> {
        let docs = self.docs();
        let d = docs.get(index)?;
        let text = std::fs::read_to_string(&d.file).ok()?;
        let parser = Parser::new_ext(
            &text,
            Options::ENABLE_TABLES
                | Options::ENABLE_STRIKETHROUGH
                | Options::ENABLE_HEADING_ATTRIBUTES,
        )
        .map(|e| match e {
            Event::Html(h) | Event::InlineHtml(h) => Event::Text(h),
            other => other,
        });
        let mut body = String::new();
        html::push_html(&mut body, parser);
        Some(json!({ "index": index, "path": d.path, "title": title(&d.file), "html": body }))
    }
}

fn walk(root: &Path, dir: &Path, label: &str, depth: usize, out: &mut Vec<Doc>) {
    if depth > 4 {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') {
            continue;
        }
        if path.is_dir() {
            walk(root, &path, label, depth + 1, out);
        } else if name.ends_with(".md") {
            let rel = path
                .strip_prefix(root)
                .unwrap_or(&path)
                .to_string_lossy()
                .into_owned();
            let group = Path::new(&rel)
                .parent()
                .map(|p| p.to_string_lossy().into_owned())
                .filter(|p| !p.is_empty())
                .map_or_else(|| label.to_owned(), |p| format!("{label}/{p}"));
            out.push(Doc {
                group,
                path: format!("{label}/{rel}"),
                file: path,
            });
        }
    }
}

/// The first `# ` heading, or the file name.
fn title(file: &Path) -> String {
    std::fs::read_to_string(file)
        .ok()
        .and_then(|t| {
            t.lines()
                .find_map(|l| l.strip_prefix("# ").map(|h| h.trim().to_owned()))
        })
        .unwrap_or_else(|| {
            file.file_stem()
                .map_or_else(String::new, |s| s.to_string_lossy().into_owned())
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_specs() {
        let c = parse("Docs=/a,/b;Research=/c;=/x;bad");
        assert_eq!(c.len(), 2);
        assert_eq!(c[0].id, "docs");
        assert_eq!(c[0].roots.len(), 2);
        assert_eq!(c[1].name, "Research");
    }

    #[test]
    fn lists_and_renders_markdown_without_raw_html() {
        let dir = std::env::temp_dir().join(format!("glacier-docs-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("decisions")).unwrap();
        std::fs::write(
            dir.join("a.md"),
            "# Alpha\n\nHello <script>x</script> **bold**\n",
        )
        .unwrap();
        std::fs::write(dir.join("decisions/0001.md"), "# First\n").unwrap();
        std::fs::write(dir.join("notes.txt"), "ignored").unwrap();
        let c = &parse(&format!("Docs={}", dir.display()))[0];
        let idx = c.index();
        assert_eq!(idx["docs"].as_array().unwrap().len(), 2);
        assert_eq!(idx["docs"][0]["title"], "Alpha");
        let html = c.render(0).unwrap()["html"].as_str().unwrap().to_owned();
        assert!(html.contains("<strong>bold</strong>"));
        assert!(!html.contains("<script>"));
        assert!(c.render(9).is_none());
        std::fs::remove_dir_all(dir).unwrap();
    }
}

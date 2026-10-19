//! Document collections: named sets of Markdown folders the console shows,
//! such as `Docs` and `Research`.
//!
//! Configured with `CONSOLE_COLLECTIONS`, for example
//! `Docs=/srv/docs/ops,/srv/docs/platform;Research=/srv/research`.
//! Each folder is scanned for `.md` files (hidden entries skipped). Documents
//! are addressed by an index into that scan, never by a path from the
//! request, so nothing outside the configured folders can be read. Raw HTML
//! in Markdown is shown as text, and a link or image whose target is a
//! `javascript:`, `data:` or `vbscript:` URL loses its target.

use std::path::{Path, PathBuf};

use pulldown_cmark::{CowStr, Event, Options, Parser, Tag, html};
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
            Event::Start(Tag::Link {
                link_type,
                dest_url,
                title,
                id,
            }) => Event::Start(Tag::Link {
                link_type,
                dest_url: safe_url(dest_url),
                title,
                id,
            }),
            Event::Start(Tag::Image {
                link_type,
                dest_url,
                title,
                id,
            }) => Event::Start(Tag::Image {
                link_type,
                dest_url: safe_url(dest_url),
                title,
                id,
            }),
            other => other,
        });
        let mut body = String::new();
        html::push_html(&mut body, parser);
        Some(json!({ "index": index, "path": d.path, "title": title(&d.file), "html": body }))
    }
}

/// A link or image target, or an empty one when it is a `javascript:`,
/// `data:` or `vbscript:` URL. The scheme is read as a browser reads it:
/// leading spaces and control characters, and tabs and newlines anywhere,
/// are ignored, and case doesn't matter.
fn safe_url(url: CowStr<'_>) -> CowStr<'_> {
    let scheme: String = url
        .trim_start_matches(|c: char| c <= ' ')
        .chars()
        .filter(|c| !matches!(c, '\t' | '\n' | '\r'))
        .take_while(|c| *c != ':')
        .collect::<String>()
        .to_ascii_lowercase();
    let has_scheme = url.contains(':');
    if has_scheme && matches!(scheme.as_str(), "javascript" | "data" | "vbscript") {
        CowStr::Borrowed("")
    } else {
        url
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
        let dir = std::env::temp_dir().join(format!("tinytrader-docs-{}", std::process::id()));
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

    #[test]
    fn script_and_data_targets_are_dropped() {
        let dir = std::env::temp_dir().join(format!("tinytrader-links-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("a.md"),
            "# Links\n\n[one](javascript:alert(1)) [two](JavaScript:alert(2)) \
             [three](<java\tscript:alert(3)>) [four](vbscript:msgbox) \
             [five](data:text/html,x) ![six](data:image/svg+xml,x) \
             [eight](&#106;avascript:alert(8))\n\n\
             [kept](https://example.com/a) [also](other.md) ![pic](img/p.png) [mail](mailto:a@b.c)\n",
        )
        .unwrap();
        let c = &parse(&format!("Docs={}", dir.display()))[0];
        let html = c.render(0).unwrap()["html"]
            .as_str()
            .unwrap()
            .to_ascii_lowercase();
        for bad in ["javascript", "vbscript", "data:"] {
            assert!(!html.contains(bad), "{bad} in {html}");
        }
        assert!(html.contains(r#"href="https://example.com/a""#), "{html}");
        assert!(html.contains(r#"href="other.md""#), "{html}");
        assert!(html.contains(r#"src="img/p.png""#), "{html}");
        assert!(html.contains(r#"href="mailto:a@b.c""#), "{html}");
        // The link text stays; only the target goes.
        assert!(html.contains(">one</a>"), "{html}");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn safe_url_reads_the_scheme_as_a_browser_does() {
        for bad in [
            "javascript:x",
            " JAVASCRIPT:x",
            "\u{1}javascript:x",
            "java\tscript:x",
            "java\nscript:x",
            "Data:x",
            "vbscript:x",
        ] {
            assert_eq!(&*safe_url(CowStr::Borrowed(bad)), "", "{bad:?}");
        }
        for ok in [
            "https://a/b",
            "other.md",
            "#top",
            "mailto:a@b.c",
            "/docs/javascript:x",
            "javascript",
        ] {
            assert_eq!(&*safe_url(CowStr::Borrowed(ok)), ok);
        }
    }
}

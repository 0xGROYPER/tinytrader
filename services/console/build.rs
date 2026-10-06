//! Builds every file under `web/` into the console, as `web::FILES`.

use std::fmt::Write;
use std::path::Path;

fn walk(root: &Path, dir: &Path, out: &mut Vec<String>) {
    let mut entries: Vec<_> = std::fs::read_dir(dir)
        .expect("web/ is readable")
        .map(|e| e.expect("entry").path())
        .collect();
    entries.sort();
    for p in entries {
        if p.is_dir() {
            walk(root, &p, out);
        } else if !p
            .file_name()
            .is_some_and(|n| n.to_string_lossy().starts_with('.'))
        {
            out.push(
                p.strip_prefix(root)
                    .expect("under web/")
                    .to_string_lossy()
                    .replace('\\', "/"),
            );
        }
    }
}

fn main() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("web");
    println!("cargo:rerun-if-changed={}", root.display());
    let mut files = Vec::new();
    walk(&root, &root, &mut files);
    let mut src = String::from("pub static FILES: &[(&str, &[u8])] = &[\n");
    for f in &files {
        writeln!(
            src,
            "    ({f:?}, include_bytes!({:?})),",
            root.join(f).display().to_string()
        )
        .unwrap();
    }
    src.push_str("];\n");
    std::fs::write(
        Path::new(&std::env::var("OUT_DIR").unwrap()).join("web.rs"),
        src,
    )
    .unwrap();
}

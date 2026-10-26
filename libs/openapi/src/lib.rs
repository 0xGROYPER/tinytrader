//! The services' OpenAPI 3.1 documents (decision 0021, section 5). Each
//! service builds its own from Rust types in a test, and [`check`]s it
//! against the copy committed under `docs/api/openapi/`: equal, and with
//! nothing removed or tightened since the base branch. Test support only: no
//! service runs this.

use std::path::Path;
use std::process::Command;

use schemars::generate::{SchemaGenerator, SchemaSettings};
use schemars::{JsonSchema, Schema};
use serde_json::{Map, Value, json};

/// Every error answer: `{"error": "<one human-readable sentence>"}`.
#[derive(JsonSchema)]
pub struct Error {
    pub error: String,
}

/// One service's document, built an operation at a time.
pub struct Spec {
    generator: SchemaGenerator,
    paths: Map<String, Value>,
}

impl Default for Spec {
    fn default() -> Self {
        let settings = SchemaSettings::draft2020_12().with(|s| {
            s.definitions_path = "/components/schemas".into();
        });
        Self {
            generator: settings.into_generator(),
            paths: Map::new(),
        }
    }
}

impl Spec {
    /// A reference to `T`'s schema, which the document then defines.
    pub fn schema<T: JsonSchema>(&mut self) -> Value {
        self.generator.subschema_for::<T>().to_value()
    }

    /// `T`'s fields as query parameters.
    pub fn query<T: JsonSchema>(&mut self) -> Vec<Value> {
        let s: Schema = self.generator.root_schema_for::<T>();
        let s = s.to_value();
        let required = s["required"].as_array().cloned().unwrap_or_default();
        entries(&s["properties"])
            .map(|(k, v)| {
                let r = required.contains(&json!(k));
                json!({ "name": k, "in": "query", "required": r, "schema": v })
            })
            .collect()
    }

    /// An operation. `responses` are (status, description, body schema); every
    /// operation also answers errors as [`Error`]. `{name}`s in `path` are
    /// string path parameters.
    pub fn op(
        &mut self,
        (method, path): (&str, &str),
        summary: &str,
        mut parameters: Vec<Value>,
        body: Option<Value>,
        responses: &[(&str, &str, Value)],
    ) {
        for name in path.split('{').skip(1).filter_map(|s| s.split_once('}')) {
            parameters.push(json!({ "name": name.0, "in": "path", "required": true, "schema": { "type": "string" } }));
        }
        let content = |s: &Value| json!({ "application/json": { "schema": s } });
        let mut answers = Map::new();
        for (code, description, s) in responses {
            answers.insert(
                (*code).into(),
                json!({ "description": description, "content": content(s) }),
            );
        }
        let error = self.schema::<Error>();
        answers.insert(
            "default".into(),
            json!({ "description": "refused or failed", "content": content(&error) }),
        );
        let mut op = json!({ "summary": summary, "parameters": parameters, "responses": answers });
        if let Some(b) = body {
            op["requestBody"] = json!({ "required": true, "content": content(&b) });
        }
        self.paths.entry(path).or_insert_with(|| json!({}))[method] = op;
    }

    #[must_use]
    pub fn document(mut self, title: &str, description: &str) -> Value {
        let schemas = self.generator.take_definitions(true);
        json!({
            "openapi": "3.1.0",
            "info": { "title": title, "description": description, "version": "0" },
            "paths": self.paths,
            "components": { "schemas": schemas },
        })
    }
}

/// Fail unless `doc` is the committed `docs/api/openapi/<service>.json` and
/// breaks nothing in the base branch's copy (the merge base with
/// `OPENAPI_BASE`, by default `origin/main`). `OPENAPI_UPDATE=1` writes `doc`
/// first. Without git history the base comparison is skipped, unless
/// `OPENAPI_REQUIRE_BASE` is set.
///
/// # Panics
/// When either check fails, saying what differs.
pub fn check(service: &str, doc: &Value) {
    let rel = format!("docs/api/openapi/{service}.json");
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let text = serde_json::to_string_pretty(doc).expect("a document") + "\n";
    if std::env::var_os("OPENAPI_UPDATE").is_some() {
        std::fs::write(root.join(&rel), &text).expect("write the document");
    }
    match base(&root, &rel) {
        Ok(Some(old)) => {
            let b = breaking(&old, doc);
            assert!(
                b.is_empty(),
                "breaking changes to {rel}:\n  {}",
                b.join("\n  ")
            );
        }
        Ok(None) => eprintln!("{rel}: not on the base branch yet; nothing to compare"),
        Err(e) if std::env::var_os("OPENAPI_REQUIRE_BASE").is_some() => panic!("{rel}: {e}"),
        Err(e) => eprintln!("{rel}: {e}; breaking-change check skipped"),
    }
    let committed = std::fs::read_to_string(root.join(&rel)).unwrap_or_default();
    assert!(
        committed == text,
        "{rel} isn't what the code generates: run `OPENAPI_UPDATE=1 cargo test -p {service} openapi` and review the change"
    );
}

/// The base branch's copy of `rel`: `None` when it has none.
fn base(root: &Path, rel: &str) -> Result<Option<Value>, String> {
    let git = |args: &[&str]| {
        let o = Command::new("git").args(args).current_dir(root).output();
        o.ok().filter(|o| o.status.success()).map(|o| o.stdout)
    };
    let to = std::env::var("OPENAPI_BASE").unwrap_or_else(|_| "origin/main".into());
    let at = git(&["merge-base", "HEAD", &to]).ok_or(format!("no merge base with {to}"))?;
    let at = String::from_utf8_lossy(&at);
    git(&["show", &format!("{}:{rel}", at.trim())])
        .map(|b| serde_json::from_slice(&b).map_err(|e| format!("base copy: {e}")))
        .transpose()
}

/// What `new` breaks of `old`: a path or method removed, a response field or
/// an enum value removed, or a request field or parameter made required.
#[must_use]
pub fn breaking(old: &Value, new: &Value) -> Vec<String> {
    let mut out = Vec::new();
    let body = |o: &Value| o["requestBody"]["content"]["application/json"]["schema"].clone();
    let answer = |o: &Value| o["content"]["application/json"]["schema"].clone();
    for (path, ops) in entries(&old["paths"]) {
        for (method, op) in entries(ops) {
            let at = format!("{} {path}", method.to_uppercase());
            let Some(now) = new["paths"][path].get(method) else {
                out.push(format!("{at}: removed"));
                continue;
            };
            let was: Vec<&Value> = list(&op["parameters"])
                .filter(|p| p["required"] == true)
                .map(|p| &p["name"])
                .collect();
            for p in list(&now["parameters"]) {
                if p["required"] == true && !was.contains(&&p["name"]) {
                    out.push(format!("{at}: parameter {} is now required", p["name"]));
                }
            }
            let docs = (old, new);
            walk(
                docs,
                &body(op),
                &body(now),
                true,
                &format!("{at} request"),
                &mut out,
            );
            for (code, r) in entries(&op["responses"]) {
                let n = answer(&now["responses"][code]);
                walk(
                    docs,
                    &answer(r),
                    &n,
                    false,
                    &format!("{at} {code}"),
                    &mut out,
                );
            }
        }
    }
    out
}

fn walk(
    docs: (&Value, &Value),
    o: &Value,
    n: &Value,
    request: bool,
    at: &str,
    out: &mut Vec<String>,
) {
    let (o, n) = (resolve(docs.0, o), resolve(docs.1, n));
    if o.is_null() || n.is_null() || at.len() > 1000 {
        return;
    }
    if let (Some(was), Some(now)) = (o["enum"].as_array(), n["enum"].as_array()) {
        for v in was.iter().filter(|v| !now.contains(v)) {
            out.push(format!("{at}: value {v} removed"));
        }
    }
    if request {
        let was: Vec<&Value> = list(&o["required"]).collect();
        for r in list(&n["required"]).filter(|r| !was.contains(r)) {
            out.push(format!("{at}: {r} is now required"));
        }
    }
    for (k, os) in entries(&o["properties"]) {
        match n["properties"].get(k) {
            Some(ns) => walk(docs, os, ns, request, &format!("{at}.{k}"), out),
            None if !request => out.push(format!("{at}.{k}: removed")),
            None => {}
        }
    }
    for k in ["items", "additionalProperties"] {
        walk(docs, &o[k], &n[k], request, &format!("{at}[]"), out);
    }
    for k in ["anyOf", "oneOf", "allOf"] {
        for (os, ns) in list(&o[k]).zip(list(&n[k])) {
            walk(docs, os, ns, request, at, out);
        }
    }
}

fn resolve<'a>(doc: &'a Value, s: &'a Value) -> &'a Value {
    match s["$ref"]
        .as_str()
        .and_then(|r| r.strip_prefix("#/components/schemas/"))
    {
        Some(name) => &doc["components"]["schemas"][name],
        None => s,
    }
}

fn entries(v: &Value) -> impl Iterator<Item = (&String, &Value)> {
    v.as_object().into_iter().flatten()
}

fn list(v: &Value) -> impl Iterator<Item = &Value> {
    v.as_array().into_iter().flatten()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(JsonSchema)]
    #[allow(dead_code)]
    struct Ask {
        name: String,
        limit: Option<u32>,
    }

    #[derive(JsonSchema)]
    #[allow(dead_code)]
    #[serde(rename_all = "lowercase")]
    enum State {
        Ready,
        Failed,
    }

    #[derive(JsonSchema)]
    #[allow(dead_code)]
    struct Answer {
        state: State,
        rows: Vec<Ask>,
    }

    fn doc(edit: impl FnOnce(&mut Value)) -> Value {
        let mut s = Spec::default();
        let (body, answer, q) = (s.schema::<Ask>(), s.schema::<Answer>(), s.query::<Ask>());
        s.op(
            ("post", "/x/{id}"),
            "x",
            q,
            Some(body),
            &[("200", "ok", answer)],
        );
        let mut d = s.document("t", "d");
        edit(&mut d);
        d
    }

    #[test]
    fn additions_pass_and_removals_or_tightening_fail() {
        let old = doc(|_| {});
        type Edit = fn(&mut Value);
        let cases: [(&str, Edit); 7] = [
            ("", |d| d["paths"]["/y"] = json!({ "get": {} })),
            ("", |d| {
                d["components"]["schemas"]["Answer"]["properties"]["more"] = json!({})
            }),
            ("POST /x/{id}: removed", |d| d["paths"] = json!({})),
            ("POST /x/{id} 200.state: value \"failed\" removed", |d| {
                d["components"]["schemas"]["State"]["enum"] = json!(["ready"]);
            }),
            ("POST /x/{id} 200.rows: removed", |d| {
                let a = d["components"]["schemas"]["Answer"]["properties"].as_object_mut();
                a.unwrap().remove("rows");
            }),
            ("POST /x/{id} request: \"limit\" is now required", |d| {
                d["components"]["schemas"]["Ask"]["required"] = json!(["name", "limit"]);
            }),
            ("POST /x/{id}: parameter \"limit\" is now required", |d| {
                let ps = d["paths"]["/x/{id}"]["post"]["parameters"].as_array_mut();
                ps.unwrap()
                    .iter_mut()
                    .for_each(|p| p["required"] = json!(true));
            }),
        ];
        for (want, edit) in cases {
            let new = doc(edit);
            let want: Vec<String> = Some(want)
                .filter(|w| !w.is_empty())
                .map(Into::into)
                .into_iter()
                .collect();
            assert_eq!(breaking(&old, &new), want, "{new}");
        }
    }
}

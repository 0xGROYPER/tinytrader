//! The services' OpenAPI 3.1 documents (decision 0021, section 5). Each
//! service builds its own from Rust types in a test, and [`check`]s it
//! against the copy committed under `docs/api/openapi/`: equal, and with
//! nothing removed or tightened since the base branch. Test support only: no
//! service runs this.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
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
    /// operation also answers errors as [`Error`], unless it names a `default`. `{name}`s in `path` are
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
        answers.entry("default").or_insert_with(
            || json!({ "description": "refused or failed", "content": content(&error) }),
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

/// A document (key `""`) and the sibling documents its `$ref`s name, by file
/// name (`databento-jobs.json`).
pub type Docs = BTreeMap<String, Value>;

const DIR: &str = "docs/api/openapi";
static NULL: Value = Value::Null;

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn read(p: &Path) -> Option<Value> {
    serde_json::from_str(&std::fs::read_to_string(p).ok()?).ok()
}

/// `doc`, with the siblings it refers to, each from `load`.
fn with_siblings(doc: Value, load: impl Fn(&str) -> Option<Value>) -> Docs {
    let mut names = Vec::new();
    siblings(&doc, &mut names);
    let mut docs: Docs = names
        .into_iter()
        .filter_map(|f| Some((f.clone(), load(&f)?)))
        .collect();
    docs.insert(String::new(), doc);
    docs
}

fn siblings(v: &Value, out: &mut Vec<String>) {
    match v {
        Value::Object(m) => {
            let r = m
                .get("$ref")
                .and_then(Value::as_str)
                .and_then(|r| r.split_once('#'));
            if let Some((f, _)) = r.filter(|(f, _)| !f.is_empty()) {
                out.push(f.to_owned());
            }
            m.values().for_each(|x| siblings(x, out));
        }
        Value::Array(a) => a.iter().for_each(|x| siblings(x, out)),
        _ => {}
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
    let rel = format!("{DIR}/{service}.json");
    let text = serde_json::to_string_pretty(doc).expect("a document") + "\n";
    if std::env::var_os("OPENAPI_UPDATE").is_some() {
        std::fs::write(root().join(&rel), &text).expect("write the document");
    }
    match base_commit() {
        Ok(at) => match show(&at, &rel) {
            Some(old) => {
                let old = with_siblings(old, |f| show(&at, &format!("{DIR}/{f}")));
                let new = with_siblings(doc.clone(), |f| read(&root().join(DIR).join(f)));
                let b = breaking(&old, &new);
                assert!(
                    b.is_empty(),
                    "breaking changes to {rel}:\n  {}",
                    b.join("\n  ")
                );
            }
            None => eprintln!("{rel}: not on the base branch yet; nothing to compare"),
        },
        Err(e) if std::env::var_os("OPENAPI_REQUIRE_BASE").is_some() => panic!("{rel}: {e}"),
        Err(e) => eprintln!("{rel}: {e}; breaking-change check skipped"),
    }
    let committed = std::fs::read_to_string(root().join(&rel)).unwrap_or_default();
    assert!(
        committed == text,
        "{rel} isn't what the code generates: run `OPENAPI_UPDATE=1 cargo test -p {service} openapi` and review the change"
    );
}

fn git(args: &[&str]) -> Option<Vec<u8>> {
    let o = Command::new("git").args(args).current_dir(root()).output();
    o.ok().filter(|o| o.status.success()).map(|o| o.stdout)
}

fn base_commit() -> Result<String, String> {
    let to = std::env::var("OPENAPI_BASE").unwrap_or_else(|_| "origin/main".into());
    let at = git(&["merge-base", "HEAD", &to]).ok_or(format!("no merge base with {to}"))?;
    Ok(String::from_utf8_lossy(&at).trim().to_owned())
}

fn show(at: &str, rel: &str) -> Option<Value> {
    serde_json::from_slice(&git(&["show", &format!("{at}:{rel}")])?).ok()
}

/// Fail unless `value` fits the committed document of `service`: what
/// `method path` answers with `status` (or its `default`), or, for `status`
/// `"request"`, its request body.
///
/// # Panics
/// When it doesn't, saying where.
pub fn assert_conforms(service: &str, method: &str, path: &str, status: &str, value: &Value) {
    let dir = root().join(DIR);
    let doc = read(&dir.join(format!("{service}.json"))).expect("the committed document");
    let docs = with_siblings(doc, |f| read(&dir.join(f)));
    let e = conforms(&docs, method, path, status, value);
    assert!(
        e.is_empty(),
        "{method} {path} {status} doesn't fit {DIR}/{service}.json:\n  {}\n{value}",
        e.join("\n  ")
    );
}

/// Where `value` doesn't fit (see [`assert_conforms`]): a type, a required
/// field, an enum value, or a field the document doesn't declare.
#[must_use]
pub fn conforms(docs: &Docs, method: &str, path: &str, status: &str, value: &Value) -> Vec<String> {
    let op = &docs[""]["paths"][path][method];
    let s = if status == "request" {
        body(op)
    } else {
        answer(
            op["responses"]
                .get(status)
                .unwrap_or(&op["responses"]["default"]),
        )
    };
    let mut out = Vec::new();
    if s.is_null() {
        out.push("not in the document".into());
    }
    fits(docs, "", s, value, "", &mut out);
    out
}

fn fits(docs: &Docs, file: &str, s: &Value, v: &Value, at: &str, out: &mut Vec<String>) {
    let (file, s) = resolve(docs, file, s);
    if let Some(alts) = s["anyOf"].as_array().or(s["oneOf"].as_array()) {
        let fit = |a| {
            let mut e = Vec::new();
            fits(docs, file, a, v, at, &mut e);
            e.is_empty()
        };
        if !alts.iter().any(fit) {
            out.push(format!("{at}: fits none of its alternatives"));
        }
        return;
    }
    let (ty, types) = (kind(v), types(s));
    if !types.is_empty()
        && !types
            .iter()
            .any(|t| *t == ty || (*t == "number" && ty == "integer"))
    {
        return out.push(format!("{at}: {ty}, not {}", types.join(" or ")));
    }
    if let Some(e) = s["enum"].as_array()
        && !e.contains(v)
    {
        out.push(format!("{at}: {v} isn't one of {}", Value::from(e.clone())));
    }
    match v {
        Value::Object(m) => {
            for r in list(&s["required"]).filter_map(Value::as_str) {
                if !m.contains_key(r) {
                    out.push(format!("{at}.{r}: missing"));
                }
            }
            for (k, x) in m {
                let at = format!("{at}.{k}");
                match (s["properties"].get(k), &s["additionalProperties"]) {
                    (Some(p), _) => fits(docs, file, p, x, &at, out),
                    (None, a) if a.is_object() => fits(docs, file, a, x, &at, out),
                    (None, _) if s.get("properties").is_some() => {
                        out.push(format!("{at}: not in the document"));
                    }
                    _ => {}
                }
            }
        }
        Value::Array(a) => {
            for (i, x) in a.iter().enumerate() {
                fits(docs, file, &s["items"], x, &format!("{at}[{i}]"), out);
            }
        }
        _ => {}
    }
}

fn kind(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(n) if n.is_i64() || n.is_u64() => "integer",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

fn types(s: &Value) -> Vec<&str> {
    match &s["type"] {
        Value::String(t) => vec![t.as_str()],
        t => list(t).filter_map(Value::as_str).collect(),
    }
}

fn body(op: &Value) -> &Value {
    &op["requestBody"]["content"]["application/json"]["schema"]
}

fn answer(r: &Value) -> &Value {
    &r["content"]["application/json"]["schema"]
}

/// What `new` breaks of `old`: a path, method, parameter, request body,
/// response status, field, alternative or enum value removed; a type
/// changed; a response field that may now be missing or null; or a request
/// field or parameter made required.
#[must_use]
pub fn breaking(old: &Docs, new: &Docs) -> Vec<String> {
    let mut w = Walk {
        old,
        new,
        out: Vec::new(),
    };
    for (path, ops) in entries(&old[""]["paths"]) {
        for (method, op) in entries(ops) {
            let at = format!("{} {path}", method.to_uppercase());
            let Some(now) = new[""]["paths"][path].get(method) else {
                w.out.push(format!("{at}: removed"));
                continue;
            };
            let params = |o: &Value| -> Vec<(String, bool)> {
                let p = list(&o["parameters"]);
                p.map(|p| (p["name"].to_string(), p["required"] == true))
                    .collect()
            };
            let (was, is) = (params(op), params(now));
            for (name, _) in was.iter().filter(|(n, _)| !is.iter().any(|x| x.0 == *n)) {
                w.out.push(format!("{at}: parameter {name} removed"));
            }
            for (name, _) in is.iter().filter(|p| p.1 && !was.contains(p)) {
                w.out
                    .push(format!("{at}: parameter {name} is now required"));
            }
            if !body(op).is_null() && body(now).is_null() {
                w.out.push(format!("{at} request: removed"));
            }
            w.walk(
                ("", body(op)),
                ("", body(now)),
                true,
                &format!("{at} request"),
            );
            for (code, r) in entries(&op["responses"]) {
                match now["responses"].get(code) {
                    None => w.out.push(format!("{at} {code}: removed")),
                    Some(n) => w.walk(
                        ("", answer(r)),
                        ("", answer(n)),
                        false,
                        &format!("{at} {code}"),
                    ),
                }
            }
        }
    }
    w.out
}

struct Walk<'a> {
    old: &'a Docs,
    new: &'a Docs,
    out: Vec<String>,
}

impl Walk<'_> {
    fn walk(&mut self, o: (&str, &Value), n: (&str, &Value), request: bool, at: &str) {
        let (o0, n0) = (o.1, n.1);
        let (of, o) = resolve(self.old, o.0, o.1);
        let (nf, n) = resolve(self.new, n.0, n.1);
        if o.is_null() || n.is_null() || at.len() > 1000 {
            return;
        }
        // Alternatives are matched by what they are, never by position.
        let alts = |s: &Value, named: &Value| -> Vec<Value> {
            let a = s["anyOf"].as_array().or(s["oneOf"].as_array());
            a.cloned().unwrap_or_else(|| vec![named.clone()])
        };
        let (oa, na) = (alts(o, o0), alts(n, n0));
        if oa.len() > 1 || na.len() > 1 {
            for a in &oa {
                match na.iter().find(|b| key(a) == key(b)) {
                    Some(b) => self.walk((of, a), (nf, b), request, at),
                    None if request => self
                        .out
                        .push(format!("{at}: alternative {} removed", key(a))),
                    None => {}
                }
            }
            for b in na
                .iter()
                .filter(|b| !request && !oa.iter().any(|a| key(a) == key(b)))
            {
                self.out.push(match key(b).as_str() {
                    "null" => format!("{at}: may now be null"),
                    k => format!("{at}: alternative {k} added"),
                });
            }
            return;
        }
        let (ot, nt) = (types(o), types(n));
        if !ot.is_empty() && !nt.is_empty() {
            let (from, to) = if request { (&nt, &ot) } else { (&ot, &nt) };
            let bad: Vec<&&str> = to.iter().filter(|t| !from.contains(t)).collect();
            if !request && bad.contains(&&"null") {
                self.out.push(format!("{at}: may now be null"));
            }
            if bad.iter().any(|t| request || **t != "null") {
                self.out.push(format!("{at}: type {ot:?} became {nt:?}"));
            }
        }
        if let (Some(was), Some(now)) = (o["enum"].as_array(), n["enum"].as_array()) {
            for v in was.iter().filter(|v| !now.contains(v)) {
                self.out.push(format!("{at}: value {v} removed"));
            }
        }
        let (or, nr): (Vec<&Value>, Vec<&Value>) = (
            list(&o["required"]).collect(),
            list(&n["required"]).collect(),
        );
        if request {
            for r in nr.iter().filter(|r| !or.contains(r)) {
                self.out.push(format!("{at}: {r} is now required"));
            }
        } else {
            for r in or.iter().filter(|r| !nr.contains(r)) {
                if n["properties"]
                    .get(r.as_str().unwrap_or_default())
                    .is_some()
                {
                    self.out.push(format!(
                        "{at}.{}: may now be missing",
                        r.as_str().unwrap_or_default()
                    ));
                }
            }
        }
        for (k, os) in entries(&o["properties"]) {
            match n["properties"].get(k) {
                Some(ns) => self.walk((of, os), (nf, ns), request, &format!("{at}.{k}")),
                None => self.out.push(format!("{at}.{k}: removed")),
            }
        }
        for k in ["items", "additionalProperties"] {
            self.walk((of, &o[k]), (nf, &n[k]), request, &format!("{at}[]"));
        }
    }
}

/// An alternative's identity: the schema it names, its types, or itself.
fn key(s: &Value) -> String {
    match s["$ref"].as_str() {
        Some(r) => r.rsplit('/').next().unwrap_or(r).to_owned(),
        None if !types(s).is_empty() => types(s).join(","),
        None => s.to_string(),
    }
}

/// `s`, or the schema its `$ref` names, in `file` or a sibling, with the
/// file that holds it.
fn resolve<'a>(docs: &'a Docs, file: &'a str, s: &'a Value) -> (&'a str, &'a Value) {
    let r = s["$ref"]
        .as_str()
        .and_then(|r| r.split_once("#/components/schemas/"));
    let Some((f, name)) = r else {
        return (file, s);
    };
    match docs.get_key_value(if f.is_empty() { file } else { f }) {
        Some((k, d)) => (k.as_str(), &d["components"]["schemas"][name]),
        None => (file, &NULL),
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

    fn fixture() -> Docs {
        let r = |n: &str| json!({ "$ref": format!("#/components/schemas/{n}") });
        let body = |s: Value| json!({ "content": { "application/json": { "schema": s } } });
        let op = json!({
            "parameters": [{ "name": "limit", "in": "query", "required": false }],
            "requestBody": body(r("Ask")),
            "responses": { "200": body(r("Answer")), "202": body(json!({ "type": "object" })) },
        });
        let doc = json!({ "paths": { "/x": { "post": op } }, "components": { "schemas": {
            "Ask": { "type": "object", "required": ["name"], "properties": {
                "name": { "type": "string" }, "limit": { "type": ["integer", "null"] },
                "mode": { "anyOf": [r("Mode"), { "type": "null" }] } } },
            "Mode": { "type": "string", "enum": ["a", "b"] },
            "State": { "type": "string", "enum": ["ready", "failed"] },
            "Answer": { "type": "object", "required": ["state", "total"], "properties": {
                "state": r("State"), "total": { "type": "integer" },
                "rows": { "type": "array", "items": { "type": "object", "properties": { "n": { "type": "integer" } } } },
                "ext": { "$ref": "other.json#/components/schemas/Ext" } } },
        } } });
        let ext = json!({ "type": "object", "properties": { "id": { "type": "string" } } });
        let other = json!({ "components": { "schemas": { "Ext": ext } } });
        Docs::from([(String::new(), doc), ("other.json".into(), other)])
    }

    fn sch<'a>(d: &'a mut Docs, name: &str) -> &'a mut Value {
        &mut d.get_mut("").unwrap()["components"]["schemas"][name]
    }

    fn op(d: &mut Docs) -> &mut Value {
        &mut d.get_mut("").unwrap()["paths"]["/x"]["post"]
    }

    #[test]
    fn additions_pass_and_removals_or_tightening_fail() {
        type Edit = fn(&mut Docs);
        let cases: [(&str, Edit); 18] = [
            ("", |d| op(d)["responses"]["201"] = json!({})),
            ("", |d| sch(d, "Answer")["properties"]["more"] = json!({})),
            ("", |d| {
                sch(d, "Ask")["properties"]["mode"]["anyOf"]
                    .as_array_mut()
                    .unwrap()
                    .reverse()
            }),
            ("POST /x: removed", |d| {
                d.get_mut("").unwrap()["paths"] = json!({})
            }),
            ("POST /x: parameter \"limit\" removed", |d| {
                op(d)["parameters"] = json!([])
            }),
            ("POST /x: parameter \"limit\" is now required", |d| {
                op(d)["parameters"][0]["required"] = json!(true)
            }),
            ("POST /x request: removed", |d| {
                drop(op(d).as_object_mut().unwrap().remove("requestBody"))
            }),
            ("POST /x request.limit: removed", |d| {
                drop(
                    sch(d, "Ask")["properties"]
                        .as_object_mut()
                        .unwrap()
                        .remove("limit"),
                )
            }),
            ("POST /x request: \"limit\" is now required", |d| {
                sch(d, "Ask")["required"] = json!(["name", "limit"])
            }),
            (
                "POST /x request.limit: type [\"integer\", \"null\"] became [\"integer\"]",
                |d| sch(d, "Ask")["properties"]["limit"]["type"] = json!("integer"),
            ),
            ("POST /x request.mode: alternative Mode removed", |d| {
                sch(d, "Ask")["properties"]["mode"] = json!({ "anyOf": [{ "type": "null" }] })
            }),
            ("POST /x 202: removed", |d| {
                drop(op(d)["responses"].as_object_mut().unwrap().remove("202"))
            }),
            ("POST /x 200.state: value \"failed\" removed", |d| {
                sch(d, "State")["enum"] = json!(["ready"])
            }),
            ("POST /x 200.rows: removed", |d| {
                drop(
                    sch(d, "Answer")["properties"]
                        .as_object_mut()
                        .unwrap()
                        .remove("rows"),
                )
            }),
            ("POST /x 200.total: may now be missing", |d| {
                sch(d, "Answer")["required"] = json!(["state"])
            }),
            ("POST /x 200.total: may now be null", |d| {
                sch(d, "Answer")["properties"]["total"]["type"] = json!(["integer", "null"])
            }),
            ("POST /x 200.state: may now be null", |d| {
                sch(d, "Answer")["properties"]["state"] = json!({ "anyOf": [{ "$ref": "#/components/schemas/State" }, { "type": "null" }] });
            }),
            ("POST /x 200.ext.id: removed", |d| {
                d.get_mut("other.json").unwrap()["components"]["schemas"]["Ext"]["properties"] =
                    json!({})
            }),
        ];
        let old = fixture();
        for (want, edit) in cases {
            let mut new = fixture();
            edit(&mut new);
            let want: Vec<String> = Some(want)
                .filter(|w| !w.is_empty())
                .map(Into::into)
                .into_iter()
                .collect();
            assert_eq!(breaking(&old, &new), want);
        }
        let mut new = fixture();
        sch(&mut new, "Answer")["properties"]["total"]["type"] = json!("string");
        assert_eq!(
            breaking(&old, &new),
            ["POST /x 200.total: type [\"integer\"] became [\"string\"]"]
        );
    }

    #[test]
    fn values_must_fit_their_schema() {
        let d = fixture();
        let ok =
            json!({ "state": "ready", "total": 3, "rows": [{ "n": 1 }], "ext": { "id": "x" } });
        assert_eq!(conforms(&d, "post", "/x", "200", &ok), Vec::<String>::new());
        let cases = [
            (json!({ "state": "ready" }), ".total: missing"),
            (
                json!({ "state": "gone", "total": 1 }),
                ".state: \"gone\" isn't one of [\"ready\",\"failed\"]",
            ),
            (
                json!({ "state": "ready", "total": 1.5 }),
                ".total: number, not integer",
            ),
            (
                json!({ "state": "ready", "total": 1, "more": 1 }),
                ".more: not in the document",
            ),
            (
                json!({ "state": "ready", "total": 1, "ext": { "id": 1 } }),
                ".ext.id: integer, not string",
            ),
        ];
        for (v, want) in cases {
            assert_eq!(conforms(&d, "post", "/x", "200", &v), [want]);
        }
        let bad = json!({ "name": "n", "mode": "c" });
        assert_eq!(
            conforms(&d, "post", "/x", "request", &bad),
            [".mode: fits none of its alternatives"]
        );
    }
}

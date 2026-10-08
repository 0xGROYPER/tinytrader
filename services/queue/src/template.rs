//! Job templates (decision 0012): the only work the queue can run, defined by
//! the deployment in Kubernetes' `Job` shape. A request names a template and
//! gives values for its declared parameters; each value must match its
//! parameter's pattern and is substituted into `{name}` slots of the
//! container's `command` and `args` only. Nothing else in a job comes from a
//! request.

use std::collections::BTreeMap;

use regex::Regex;
use serde::{Deserialize, Serialize};

/// Where a template's work runs.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Runner {
    /// A one-off container, created by the queue service.
    Docker,
    /// A command on the host, claimed by `queue host-runner`.
    Host,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Template {
    #[serde(rename = "apiVersion")]
    pub api_version: String,
    pub kind: String,
    pub metadata: Metadata,
    pub spec: Spec,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Metadata {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Spec {
    pub runner: Runner,
    /// Decision 0011's class: `background` or `interactive`.
    #[serde(default = "background")]
    pub class: String,
    #[serde(default)]
    pub parameters: BTreeMap<String, Parameter>,
    /// Seconds a job may run before it is stopped and failed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_deadline_seconds: Option<u64>,
    /// Retries; only 0 is supported (no retries).
    #[serde(default)]
    pub backoff_limit: u32,
    pub template: PodTemplate,
}

fn background() -> String {
    "background".into()
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Parameter {
    /// A regular expression the whole value must match.
    pub pattern: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct PodTemplate {
    pub spec: PodSpec,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct PodSpec {
    pub containers: Vec<Container>,
    #[serde(default)]
    pub volumes: Vec<Volume>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Container {
    /// The image (docker runner); ignored on the host.
    #[serde(default)]
    pub image: String,
    pub command: Vec<String>,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub working_dir: Option<String>,
    #[serde(default)]
    pub env: Vec<EnvVar>,
    #[serde(default)]
    pub resources: Resources,
    #[serde(default)]
    pub volume_mounts: Vec<VolumeMount>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct EnvVar {
    pub name: String,
    pub value: String,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct Resources {
    #[serde(default)]
    pub limits: Limits,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct Limits {
    /// Cores, e.g. `"1"` or `"500m"`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cpu: Option<String>,
    /// Bytes with a suffix, e.g. `"1Gi"` or `"512Mi"`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VolumeMount {
    pub name: String,
    pub mount_path: String,
    #[serde(default)]
    pub read_only: bool,
}

/// A volume: a host path, a Docker named volume (as a claim), or the queue's
/// secrets (the files it was given, mounted read only).
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Volume {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host_path: Option<HostPath>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub persistent_volume_claim: Option<Claim>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub secret: Option<SecretRef>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct HostPath {
    pub path: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Claim {
    pub claim_name: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SecretRef {
    pub secret_name: String,
}

/// Read every template in a YAML file (documents separated by `---`),
/// expanding `${VAR}` from the queue's own environment (deployment
/// configuration, never a request).
///
/// # Errors
/// A document that isn't a valid `tinytrader/v1` `JobTemplate`, a bad
/// parameter pattern, or two templates with one name.
pub fn load(text: &str) -> Result<Vec<Template>, String> {
    let text = expand_env(text);
    let mut out: Vec<Template> = Vec::new();
    for doc in serde_yaml::Deserializer::from_str(&text) {
        let v = serde_yaml::Value::deserialize(doc).map_err(|e| e.to_string())?;
        if v.is_null() {
            continue;
        }
        let t: Template = serde_yaml::from_value(v).map_err(|e| e.to_string())?;
        if t.api_version != "tinytrader/v1" || t.kind != "JobTemplate" {
            return Err(format!(
                "{}: expected apiVersion tinytrader/v1, kind JobTemplate",
                t.metadata.name
            ));
        }
        if t.spec.template.spec.containers.len() != 1 {
            return Err(format!("{}: exactly one container", t.metadata.name));
        }
        if t.spec.backoff_limit != 0 {
            return Err(format!(
                "{}: only backoffLimit 0 is supported",
                t.metadata.name
            ));
        }
        for (name, p) in &t.spec.parameters {
            Regex::new(&anchored(&p.pattern))
                .map_err(|e| format!("{}: parameter {name}: {e}", t.metadata.name))?;
        }
        if out.iter().any(|o| o.metadata.name == t.metadata.name) {
            return Err(format!("template {} defined twice", t.metadata.name));
        }
        out.push(t);
    }
    Ok(out)
}

fn anchored(p: &str) -> String {
    format!("^(?:{})$", p.trim_start_matches('^').trim_end_matches('$'))
}

fn expand_env(text: &str) -> String {
    let re = Regex::new(r"\$\{([A-Z_][A-Z0-9_]*)\}").expect("a valid pattern");
    re.replace_all(text, |c: &regex::Captures| {
        std::env::var(&c[1]).unwrap_or_default()
    })
    .into_owned()
}

/// The command a job runs: the container's `command` and `args` with each
/// `{name}` slot replaced by its checked parameter value.
///
/// # Errors
/// A missing, unknown or non-matching parameter.
pub fn resolve(
    t: &Template,
    params: &BTreeMap<String, String>,
) -> Result<(Vec<String>, Vec<String>), String> {
    for name in params.keys() {
        if !t.spec.parameters.contains_key(name) {
            return Err(format!("{} has no parameter {name}", t.metadata.name));
        }
    }
    for (name, p) in &t.spec.parameters {
        let v = params
            .get(name)
            .ok_or_else(|| format!("parameter {name} is required"))?;
        let re = Regex::new(&anchored(&p.pattern)).map_err(|e| e.to_string())?;
        if !re.is_match(v) {
            return Err(format!("parameter {name} must match {}", p.pattern));
        }
    }
    let fill = |s: &String| {
        params
            .iter()
            .fold(s.clone(), |acc, (k, v)| acc.replace(&format!("{{{k}}}"), v))
    };
    let c = &t.spec.template.spec.containers[0];
    Ok((
        c.command.iter().map(fill).collect(),
        c.args.iter().map(fill).collect(),
    ))
}

/// Kubernetes CPU quantities as nanocores: `"1"`, `"0.5"`, `"500m"`.
#[must_use]
pub fn nano_cpus(q: &str) -> Option<i64> {
    let q = q.trim();
    if let Some(m) = q.strip_suffix('m') {
        return m.parse::<f64>().ok().map(|v| (v * 1e6) as i64);
    }
    q.parse::<f64>().ok().map(|v| (v * 1e9) as i64)
}

/// Kubernetes memory quantities as bytes: `"512Mi"`, `"1Gi"`, `"1G"`, plain bytes.
#[must_use]
pub fn bytes(q: &str) -> Option<i64> {
    let q = q.trim();
    let units: [(&str, f64); 6] = [
        ("Ki", 1024.0),
        ("Mi", 1024.0 * 1024.0),
        ("Gi", 1024.0 * 1024.0 * 1024.0),
        ("K", 1e3),
        ("M", 1e6),
        ("G", 1e9),
    ];
    for (s, m) in units {
        if let Some(n) = q.strip_suffix(s) {
            return n.parse::<f64>().ok().map(|v| (v * m) as i64);
        }
    }
    q.parse::<i64>().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    const YAML: &str = r"
apiVersion: tinytrader/v1
kind: JobTemplate
metadata: { name: research-collector }
spec:
  runner: docker
  parameters:
    script: { pattern: '^collect_[a-z_]+\.py$' }
  activeDeadlineSeconds: 7200
  template:
    spec:
      containers:
        - image: research:local
          command: [python, '{script}']
          workingDir: /work
          resources: { limits: { cpu: '1', memory: 1Gi } }
---
apiVersion: tinytrader/v1
kind: JobTemplate
metadata: { name: build }
spec:
  runner: host
  parameters:
    package: { pattern: '[a-z-]+' }
  template:
    spec:
      containers:
        - command: [cargo, build, -p, '{package}']
          workingDir: ~/repos/tinytrader
";

    #[test]
    fn templates_load_and_parameters_fill_only_their_slots() {
        let ts = load(YAML).unwrap();
        assert_eq!(ts.len(), 2);
        let t = &ts[0];
        let ok = BTreeMap::from([("script".to_string(), "collect_runup.py".to_string())]);
        assert_eq!(resolve(t, &ok).unwrap().0, ["python", "collect_runup.py"]);
        for bad in [
            "collect_x.py; rm -rf /",
            "../collect_x.py",
            "x.py",
            "collect_x.py\n",
        ] {
            let p = BTreeMap::from([("script".to_string(), bad.to_string())]);
            assert!(resolve(t, &p).is_err(), "{bad:?} must be refused");
        }
        let extra = BTreeMap::from([
            ("script".to_string(), "collect_a.py".to_string()),
            ("image".to_string(), "evil".to_string()),
        ]);
        assert!(resolve(t, &extra).is_err());
        assert!(resolve(t, &BTreeMap::new()).is_err());
        assert_eq!(ts[1].spec.runner, Runner::Host);
    }

    #[test]
    fn quantities() {
        assert_eq!(nano_cpus("1"), Some(1_000_000_000));
        assert_eq!(nano_cpus("500m"), Some(500_000_000));
        assert_eq!(bytes("1Gi"), Some(1 << 30));
        assert_eq!(bytes("512Mi"), Some(512 << 20));
    }

    #[test]
    fn refuses_other_kinds_and_retries() {
        assert!(load("apiVersion: batch/v1\nkind: Job\nmetadata: {name: x}\nspec: {runner: host, template: {spec: {containers: [{command: [x]}]}}}").is_err());
        assert!(load("apiVersion: tinytrader/v1\nkind: JobTemplate\nmetadata: {name: x}\nspec: {runner: host, backoffLimit: 2, template: {spec: {containers: [{command: [x]}]}}}").is_err());
    }
}

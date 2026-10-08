//! The docker runner: one-off containers through the Docker Engine API (over
//! HTTP, behind a proxy that allows containers and nothing else). Every
//! request is built from the job's template; the only values from a client
//! are the parameter values already checked into the job's command.

use std::time::Duration;

use serde_json::{Value, json};

use crate::job::Job;
use crate::template::{self, Template};

pub struct Docker {
    base: String,
    http: reqwest::Client,
    /// The Compose network jobs join, so they reach the deployment's services.
    network: Option<String>,
    /// Where the queue's secrets are for a template's `secret` volume:
    /// `VOLUME` or `VOLUME:SUBPATH` (a folder of a volume, mounted alone and
    /// read only).
    secrets_volume: Option<String>,
}

/// What a container is doing.
pub enum Status {
    Running,
    Exited(i64),
    Gone,
}

impl Docker {
    #[must_use]
    pub fn new(base: &str, network: Option<String>, secrets_volume: Option<String>) -> Self {
        Self {
            base: base.trim_end_matches('/').to_owned(),
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(60))
                .build()
                .expect("an HTTP client"),
            network,
            secrets_volume,
        }
    }

    #[must_use]
    pub fn name(job: &Job) -> String {
        format!("queue-{}", job.id)
    }

    /// The container request for a job, from its template alone.
    ///
    /// # Errors
    /// A template volume this runner can't provide.
    pub fn spec(&self, t: &Template, job: &Job) -> Result<Value, String> {
        let c = &t.spec.template.spec.containers[0];
        let mut mounts = Vec::new();
        for m in &c.volume_mounts {
            let v = t
                .spec
                .template
                .spec
                .volumes
                .iter()
                .find(|v| v.name == m.name)
                .ok_or_else(|| format!("volume {} isn't defined", m.name))?;
            let (kind, source) = if let Some(h) = &v.host_path {
                ("bind", h.path.clone())
            } else if let Some(p) = &v.persistent_volume_claim {
                ("volume", p.claim_name.clone())
            } else if v.secret.is_some() {
                let vol = self
                    .secrets_volume
                    .clone()
                    .ok_or("a secret volume needs QUEUE_SECRETS_VOLUME")?;
                ("volume", vol)
            } else {
                return Err(format!(
                    "volume {}: hostPath, persistentVolumeClaim or secret",
                    v.name
                ));
            };
            let mut mount = json!({
                "Type": kind,
                "Source": source,
                "Target": m.mount_path,
                "ReadOnly": m.read_only || v.secret.is_some(),
            });
            if v.secret.is_some()
                && let Some((vol, sub)) = mount["Source"]
                    .as_str()
                    .and_then(|s| s.split_once(':'))
                    .map(|(a, b)| (a.to_owned(), b.to_owned()))
            {
                mount["Source"] = json!(vol);
                mount["VolumeOptions"] = json!({ "Subpath": sub });
            }
            mounts.push(mount);
        }
        let mut host = json!({
            "Mounts": mounts,
            "Init": true,
            "RestartPolicy": { "Name": "no" },
            // Background work gets a small share of the CPU (decision 0011).
            "CpuShares": if t.spec.class == "background" { 128 } else { 512 },
        });
        if let Some(n) = &self.network {
            host["NetworkMode"] = json!(n);
        }
        if let Some(cpu) = c
            .resources
            .limits
            .cpu
            .as_deref()
            .and_then(template::nano_cpus)
        {
            host["NanoCpus"] = json!(cpu);
        }
        if let Some(mem) = c
            .resources
            .limits
            .memory
            .as_deref()
            .and_then(template::bytes)
        {
            host["Memory"] = json!(mem);
        }
        let mut spec = json!({
            "Image": c.image,
            "Entrypoint": job.command,
            "Cmd": job.args,
            "Env": c.env.iter().map(|e| format!("{}={}", e.name, e.value)).collect::<Vec<_>>(),
            "Labels": { "tinytrader.job": job.id, "tinytrader.template": job.template },
            "HostConfig": host,
        });
        if let Some(w) = &c.working_dir {
            spec["WorkingDir"] = json!(w);
        }
        Ok(spec)
    }

    async fn call(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<Value>,
    ) -> Result<reqwest::Response, String> {
        let mut r = self.http.request(method, format!("{}{path}", self.base));
        if let Some(b) = body {
            r = r.json(&b);
        }
        r.send()
            .await
            .map_err(|e| format!("docker: {}", e.without_url()))
    }

    /// Create and start a job's container.
    ///
    /// # Errors
    /// Docker refused either step (the message is Docker's).
    pub async fn start(&self, t: &Template, job: &Job) -> Result<String, String> {
        let name = Self::name(job);
        let spec = self.spec(t, job)?;
        let r = self
            .call(
                reqwest::Method::POST,
                &format!("/containers/create?name={name}"),
                Some(spec),
            )
            .await?;
        if !r.status().is_success() {
            return Err(format!("create: {}", message(r).await));
        }
        let r = self
            .call(
                reqwest::Method::POST,
                &format!("/containers/{name}/start"),
                None,
            )
            .await?;
        if !r.status().is_success() && r.status().as_u16() != 304 {
            return Err(format!("start: {}", message(r).await));
        }
        Ok(name)
    }

    /// Whether a job's container runs, has exited (with its code) or is gone.
    ///
    /// # Errors
    /// Docker couldn't be asked.
    pub async fn status(&self, job: &Job) -> Result<Status, String> {
        let r = self
            .call(
                reqwest::Method::GET,
                &format!("/containers/{}/json", Self::name(job)),
                None,
            )
            .await?;
        if r.status().as_u16() == 404 {
            return Ok(Status::Gone);
        }
        let v: Value = r.json().await.map_err(|e| e.to_string())?;
        Ok(match v["State"]["Status"].as_str() {
            Some("exited" | "dead") => {
                Status::Exited(v["State"]["ExitCode"].as_i64().unwrap_or(-1))
            }
            _ => Status::Running,
        })
    }

    /// The container's output (stdout and stderr), the last `tail` lines or all.
    ///
    /// # Errors
    /// Docker couldn't be asked.
    pub async fn logs(&self, job: &Job, tail: Option<u32>) -> Result<Vec<u8>, String> {
        let tail = tail.map_or("all".to_owned(), |n| n.to_string());
        let r = self
            .call(
                reqwest::Method::GET,
                &format!(
                    "/containers/{}/logs?stdout=1&stderr=1&tail={tail}",
                    Self::name(job)
                ),
                None,
            )
            .await?;
        if !r.status().is_success() {
            return Err(format!("logs: {}", message(r).await));
        }
        Ok(demux(&r.bytes().await.map_err(|e| e.to_string())?))
    }

    /// Stop a job's container (SIGTERM, then SIGKILL after 10 s).
    pub async fn stop(&self, job: &Job) {
        let _ = self
            .call(
                reqwest::Method::POST,
                &format!("/containers/{}/stop?t=10", Self::name(job)),
                None,
            )
            .await;
    }

    /// Remove a finished job's container.
    pub async fn remove(&self, job: &Job) {
        let _ = self
            .call(
                reqwest::Method::DELETE,
                &format!("/containers/{}?force=1", Self::name(job)),
                None,
            )
            .await;
    }
}

async fn message(r: reqwest::Response) -> String {
    let code = r.status().as_u16();
    let v: Value = r.json().await.unwrap_or(Value::Null);
    format!("{code} {}", v["message"].as_str().unwrap_or(""))
}

/// Docker's multiplexed log stream (8-byte frame headers) as plain bytes; a
/// stream without frames (a TTY) as it is.
#[must_use]
pub fn demux(raw: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(raw.len());
    let mut i = 0;
    while i + 8 <= raw.len() {
        if raw[i] > 2 || raw[i + 1..i + 4] != [0, 0, 0] {
            return raw.to_vec();
        }
        let n = u32::from_be_bytes([raw[i + 4], raw[i + 5], raw[i + 6], raw[i + 7]]) as usize;
        let end = (i + 8 + n).min(raw.len());
        out.extend_from_slice(&raw[i + 8..end]);
        i = end;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    #[test]
    fn frames_are_stripped() {
        let mut raw = vec![1, 0, 0, 0, 0, 0, 0, 3];
        raw.extend(b"ab\n");
        raw.extend([2, 0, 0, 0, 0, 0, 0, 2]);
        raw.extend(b"e\n");
        assert_eq!(demux(&raw), b"ab\ne\n");
        assert_eq!(demux(b"plain text\n"), b"plain text\n");
    }

    #[test]
    fn the_container_comes_from_the_template() {
        let t = &template::load(
            r"
apiVersion: tinytrader/v1
kind: JobTemplate
metadata: { name: c }
spec:
  runner: docker
  parameters: { script: { pattern: 'collect_[a-z]+\.py' } }
  template:
    spec:
      containers:
        - image: research:local
          command: [python, '{script}']
          workingDir: /work
          resources: { limits: { cpu: 500m, memory: 512Mi } }
          volumeMounts:
            - { name: work, mountPath: /work }
            - { name: keys, mountPath: /run/queue-secrets }
      volumes:
        - { name: work, hostPath: { path: /srv/research } }
        - { name: keys, secret: { secretName: databento_api_key } }
",
        )
        .unwrap()[0];
        let params = BTreeMap::from([("script".to_string(), "collect_x.py".to_string())]);
        let (command, args) = template::resolve(t, &params).unwrap();
        let job = Job {
            id: "c-1".into(),
            template: "c".into(),
            runner: template::Runner::Docker,
            params,
            command,
            args,
            not_before: None,
            when: crate::job::When::Anytime,
            after: vec![],
            state: crate::job::State::Pending,
            reason: String::new(),
            created: 0.0,
            started: None,
            finished: None,
            exit_code: None,
            worker: None,
            cancel_requested: false,
            added_by: String::new(),
        };
        let d = Docker::new(
            "http://proxy:2375",
            Some("net".into()),
            Some("queue-data:secrets".into()),
        );
        let s = d.spec(t, &job).unwrap();
        assert_eq!(s["Entrypoint"], json!(["python", "collect_x.py"]));
        assert_eq!(s["HostConfig"]["NanoCpus"], 500_000_000);
        assert_eq!(s["HostConfig"]["CpuShares"], 128);
        assert_eq!(s["HostConfig"]["NetworkMode"], "net");
        assert_eq!(s["HostConfig"]["Mounts"][1]["ReadOnly"], true);
        assert_eq!(s["HostConfig"]["Mounts"][1]["Source"], "queue-data");
        assert_eq!(
            s["HostConfig"]["Mounts"][1]["VolumeOptions"]["Subpath"],
            "secrets"
        );
        assert!(s["HostConfig"].get("Privileged").is_none());
        assert_eq!(s["Labels"]["tinytrader.job"], "c-1");
    }
}

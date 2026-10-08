//! `tt`: tinytrader from the command line. For now, the job queue:
//!
//! ```text
//! tt queue list
//! tt queue templates
//! tt queue add TEMPLATE [NAME=VALUE ...] [--not-before "HH:MM" | "YYYY-MM-DD HH:MM"]
//!                       [--when anytime|after-close|background-may-run] [--after ID ...]
//! tt queue cancel ID
//! tt queue run-now ID
//! tt queue log ID
//! ```
//!
//! Times are New York time. The queue's address comes from `TT_QUEUE`
//! (default `http://127.0.0.1:9123`).

use std::collections::BTreeMap;
use std::process::ExitCode;

use serde_json::{Value, json};
use tinytrader_schedule::{ny_date_time, ny_to_unix};

const USAGE: &str = "usage: tt queue list | templates | add TEMPLATE [NAME=VALUE ...] [--not-before T] [--when W] [--after ID ...] | cancel ID | run-now ID | log ID";

/// `HH:MM` (today) or `YYYY-MM-DD HH:MM`, New York time, as Unix seconds.
fn when_at(s: &str) -> Result<f64, String> {
    let now = tinytrader_observe::unix_seconds() as i64;
    let (date, time) = match s.trim().split_once([' ', 'T']) {
        Some((d, t)) => (d.to_owned(), t.to_owned()),
        None => (ny_date_time(now).0, s.trim().to_owned()),
    };
    let n = |p: &str| p.parse::<i64>().map_err(|_| format!("not a time: {s}"));
    let d: Vec<&str> = date.split('-').collect();
    let t: Vec<&str> = time.split(':').collect();
    let [y, m, dd] = d[..] else {
        return Err(format!("not a date: {s}"));
    };
    let [h, mi] = t[..2.min(t.len())] else {
        return Err(format!("not a time: {s}"));
    };
    Ok(ny_to_unix(n(y)?, n(m)?, n(dd)?, n(h)?, n(mi)?) as f64)
}

fn show_time(t: Option<f64>) -> String {
    t.map_or("—".into(), |t| {
        let (d, hm, _) = ny_date_time(t as i64);
        format!("{d} {hm}")
    })
}

async fn call(method: reqwest::Method, path: &str, body: Option<Value>) -> Result<Value, String> {
    let base = std::env::var("TT_QUEUE").unwrap_or_else(|_| "http://127.0.0.1:9123".into());
    let mut r =
        reqwest::Client::new().request(method, format!("{}{path}", base.trim_end_matches('/')));
    if let Some(b) = body {
        r = r.json(&b);
    }
    let r = r
        .send()
        .await
        .map_err(|e| format!("the queue at {base}: {}", e.without_url()))?;
    let ok = r.status().is_success();
    let v: Value = r.json().await.unwrap_or(Value::Null);
    if ok {
        Ok(v)
    } else {
        Err(v["error"].as_str().unwrap_or("refused").to_owned())
    }
}

fn print_job(j: &Value) {
    let params: Vec<String> = j["params"]
        .as_object()
        .into_iter()
        .flatten()
        .map(|(k, v)| format!("{k}={}", v.as_str().unwrap_or("")))
        .collect();
    println!(
        "{:<28} {:<10} {:<18} {:<30} {}",
        j["id"].as_str().unwrap_or(""),
        j["state"].as_str().unwrap_or(""),
        show_time(j["started"].as_f64().or(j["created"].as_f64())),
        params.join(" "),
        j["reason"].as_str().unwrap_or("")
    );
}

async fn queue(args: &[String]) -> Result<(), String> {
    let get = reqwest::Method::GET;
    let post = reqwest::Method::POST;
    match args.first().map(String::as_str) {
        Some("list") => {
            let v = call(get, "/jobs", None).await?;
            for j in v["jobs"].as_array().into_iter().flatten() {
                print_job(j);
            }
        }
        Some("templates") => {
            let v = call(get, "/templates", None).await?;
            for t in v["templates"].as_array().into_iter().flatten() {
                let params: Vec<String> = t["parameters"]
                    .as_object()
                    .into_iter()
                    .flatten()
                    .map(|(k, p)| format!("{k}~{}", p["pattern"].as_str().unwrap_or("")))
                    .collect();
                println!(
                    "{:<22} {:<7} {}",
                    t["name"].as_str().unwrap_or(""),
                    t["runner"].as_str().unwrap_or(""),
                    params.join(" ")
                );
            }
        }
        Some("add") => {
            let template = args.get(1).ok_or(USAGE)?;
            let mut params = BTreeMap::new();
            let (mut not_before, mut when, mut after) = (None, "anytime".to_owned(), Vec::new());
            let mut it = args[2..].iter();
            while let Some(a) = it.next() {
                match a.as_str() {
                    "--not-before" => not_before = Some(when_at(it.next().ok_or(USAGE)?)?),
                    "--when" => it.next().ok_or(USAGE)?.clone_into(&mut when),
                    "--after" => after.push(it.next().ok_or(USAGE)?.clone()),
                    kv => {
                        let (k, v) = kv
                            .split_once('=')
                            .ok_or(format!("{kv}: expected NAME=VALUE"))?;
                        params.insert(k.to_owned(), v.to_owned());
                    }
                }
            }
            let body = json!({ "template": template, "params": params, "notBefore": not_before, "when": when, "after": after, "addedBy": "tt" });
            print_job(&call(post, "/jobs", Some(body)).await?);
        }
        Some("cancel") => print_job(
            &call(
                post,
                &format!("/jobs/{}/cancel", args.get(1).ok_or(USAGE)?),
                None,
            )
            .await?,
        ),
        Some("run-now") => print_job(
            &call(
                post,
                &format!("/jobs/{}/run-now", args.get(1).ok_or(USAGE)?),
                None,
            )
            .await?,
        ),
        Some("log") => print!(
            "{}",
            call(
                get,
                &format!("/jobs/{}/log", args.get(1).ok_or(USAGE)?),
                None
            )
            .await?["log"]
                .as_str()
                .unwrap_or("")
        ),
        _ => return Err(USAGE.into()),
    }
    Ok(())
}

#[tokio::main]
async fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let r = match args.first().map(String::as_str) {
        Some("queue") => queue(&args[1..]).await,
        _ => Err(USAGE.into()),
    };
    match r {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("tt: {e}");
            ExitCode::from(2)
        }
    }
}

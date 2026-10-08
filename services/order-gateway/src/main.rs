//! The `order-gateway` binary (exit 0 stopped, 1 runtime failure, 2 usage). Tokens
//! and keys are read here; values are never printed.
use order_gateway::broker::Broker;
use order_gateway::{
    AlpacaPaper, ClientSpec, Config, Profile, Role, SimBroker, Timing, check_paths, check_state,
    load_credentials, start, trading::Credentials,
};
use signal_hook::consts::{SIGINT, SIGTERM};
use signal_hook::iterator::Signals;
use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::mpsc::{RecvTimeoutError, channel};
use std::time::Duration;

const HELP: &str = "order-gateway --account NAME --broker sim|alpaca-paper --listen 127.0.0.1:PORT --state DIR
    --client NAME,ROLE,PREFIX,TOKEN [--client ...]      (ROLE: trade|observe|admin)
    [--environment paper] [--credentials-file FILE | --credentials env] [--listen-any]
    [--sim-script FILE] [--sim-quotes DBN_FILE|tcp:HOST:PORT]
    [--underlyings SPY,..] [--equities AAPL,..] [--allow-short] [--max-shares N]
    [--max-qty N] [--max-open N] [--max-legs N] [--max-notional DOLLARS]
    [--submits-per-min N] [--submits-per-day N] [--max-rejects N] [--budget N] [--min-options-level N]
    [--resolve-ms 1000,2000,4000] [--test-stall-after-admitted]
Serves one broker account on a loopback port (tinytrader.gateway/1, NDJSON). The sim broker runs
scripted (--sim-script) or against DBN quotes (--sim-quotes; client-clocked with `advance`).
alpaca-paper needs --credentials-file (mode 600, APCA_* or ALPACA_* key names) or --credentials env
(APCA_API_KEY_ID and APCA_API_SECRET_KEY, each from the environment or a *_FILE such as a Docker
secret); values are never printed. It uses the compiled paper endpoints. TOKEN is a file (mode 600,
not a symlink) or env:VAR (VAR or VAR_FILE); either way >= 64 hex chars. --listen-any allows a
non-loopback --listen, only for a container whose port is published on loopback.
";

/// `NAME` from the environment, or the file named by `NAME_FILE` (a Docker secret).
fn from_env(name: &str) -> Result<String, String> {
    if let Ok(v) = std::env::var(name) {
        return Ok(v.trim().to_owned());
    }
    let file =
        std::env::var(format!("{name}_FILE")).map_err(|_| format!("set {name} or {name}_FILE"))?;
    std::fs::read_to_string(&file)
        .map(|t| t.trim().to_owned())
        .map_err(|e| format!("{name}_FILE {file}: {e}"))
}

fn credentials_from_env() -> Result<Credentials, String> {
    let id = from_env("APCA_API_KEY_ID")?;
    let secret = from_env("APCA_API_SECRET_KEY")?;
    Credentials::from_env_text(&format!(
        "APCA_API_KEY_ID={id}\nAPCA_API_SECRET_KEY={secret}\n"
    ))
}

fn token(spec: &str) -> Result<String, String> {
    let t = if let Some(var) = spec.strip_prefix("env:") {
        from_env(var)?
    } else {
        token_file(spec)?
    };
    if t.len() < 64 || !t.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(format!(
            "token {spec} must hold at least 32 random bytes as hex"
        ));
    }
    Ok(t)
}

fn token_file(path: &str) -> Result<String, String> {
    let meta = std::fs::symlink_metadata(path).map_err(|e| format!("token file {path}: {e}"))?;
    if meta.file_type().is_symlink() || meta.permissions().mode() & 0o077 != 0 {
        return Err(format!(
            "token file {path} must be mode 600 and not a symlink"
        ));
    }
    let text = std::fs::read_to_string(path).map_err(|e| format!("token file {path}: {e}"))?;
    Ok(text.trim().to_owned())
}

#[expect(clippy::too_many_lines, reason = "one flat option table")]
fn parse(args: &[String]) -> Result<(Config, [Option<String>; 3]), String> {
    let mut o: BTreeMap<&str, &str> = BTreeMap::new();
    let (mut clients, mut stall, mut allow_short, mut listen_any) =
        (Vec::new(), false, false, false);
    // `--name=value` is the same as `--name value`.
    let args: Vec<&str> = args
        .iter()
        .flat_map(|a| match a.split_once('=') {
            Some((k, v)) if k.starts_with("--") => vec![k, v],
            _ => vec![a.as_str()],
        })
        .collect();
    let mut it = args.iter().copied();
    while let Some(k) = it.next() {
        match k {
            "--test-stall-after-admitted" => stall = true,
            "--allow-short" => allow_short = true,
            "--listen-any" => listen_any = true,
            "--client" => clients.push(it.next().ok_or("--client needs a value")?),
            k if k.starts_with("--") => {
                let v = it.next().ok_or_else(|| format!("{k} needs a value"))?;
                if o.insert(k, v).is_some() {
                    return Err(format!("{k} given twice"));
                }
            }
            k => return Err(format!("unexpected argument {k}")),
        }
    }
    let mut take = |k: &str| o.remove(k);
    let need = |v: Option<&str>, k: &str| v.map(String::from).ok_or(format!("{k} is required"));
    let num = |v: Option<&str>, k: &str, d: u64| -> Result<u64, String> {
        v.map_or(Ok(d), |s| {
            s.parse().map_err(|_| format!("{k} must be an integer"))
        })
    };
    let account = need(take("--account"), "--account")?;
    let broker = need(take("--broker"), "--broker")?;
    let environment = take("--environment").unwrap_or("paper").to_owned();
    if environment != "paper" {
        return Err("only --environment paper is supported in v1".into());
    }
    let listen: SocketAddr = need(take("--listen"), "--listen")?
        .parse()
        .map_err(|e| format!("--listen: {e}"))?;
    let state = PathBuf::from(need(take("--state"), "--state")?);
    let script = take("--sim-script").map(String::from);
    let quotes = take("--sim-quotes").map(String::from);
    let credentials = match (take("--credentials-file"), take("--credentials")) {
        (Some(_), Some(_)) => {
            return Err("--credentials-file or --credentials env, not both".into());
        }
        (Some(f), None) => Some(f.to_owned()),
        (None, Some("env")) => Some("env".to_owned()),
        (None, Some(v)) => return Err(format!("--credentials {v}: only env")),
        (None, None) => None,
    };
    match broker.as_str() {
        "sim" if credentials.is_some() => {
            return Err("credentials need --broker alpaca-paper".into());
        }
        "alpaca-paper" if script.is_some() || quotes.is_some() => {
            return Err("--sim-script and --sim-quotes need --broker sim".into());
        }
        "alpaca-paper" if credentials.is_none() => {
            return Err(
                "--broker alpaca-paper needs --credentials-file or --credentials env".into(),
            );
        }
        "sim" | "alpaca-paper" => {}
        b => return Err(format!("--broker {b}: expected sim or alpaca-paper")),
    }
    let d = Profile::default();
    let n = |v, k, d: usize| num(v, k, d as u64).map(|x| usize::try_from(x).unwrap_or(usize::MAX));
    let n32 = |v, k, d: u32| num(v, k, u64::from(d)).map(|x| u32::try_from(x).unwrap_or(u32::MAX));
    let profile = Profile {
        underlyings: take("--underlyings").map_or(d.underlyings.clone(), |s| {
            s.split(',').map(String::from).collect()
        }),
        equities: take("--equities").map_or(d.equities.clone(), |s| {
            s.split(',').map(String::from).collect()
        }),
        allow_short,
        max_shares: n32(take("--max-shares"), "--max-shares", d.max_shares)?,
        max_legs: n(take("--max-legs"), "--max-legs", d.max_legs)?,
        max_qty: n32(take("--max-qty"), "--max-qty", d.max_qty)?,
        max_open: n(take("--max-open"), "--max-open", d.max_open)?,
        max_notional_cents: i64::try_from(
            num(take("--max-notional"), "--max-notional", 2500)? * 100,
        )
        .unwrap_or(i64::MAX),
        submits_per_min: n(
            take("--submits-per-min"),
            "--submits-per-min",
            d.submits_per_min,
        )?,
        submits_per_day: n(
            take("--submits-per-day"),
            "--submits-per-day",
            d.submits_per_day,
        )?,
        max_rejects: n32(take("--max-rejects"), "--max-rejects", d.max_rejects)?,
        budget_per_min: n32(take("--budget"), "--budget", d.budget_per_min)?,
        min_options_level: n32(
            take("--min-options-level"),
            "--min-options-level",
            d.min_options_level,
        )?,
    };
    let mut timing = Timing::default();
    if let Some(s) = take("--resolve-ms") {
        timing.resolve = s
            .split(',')
            .map(|x| x.parse().map(Duration::from_millis))
            .collect::<Result<_, _>>()
            .map_err(|_| "--resolve-ms: comma-separated milliseconds")?;
    }
    if let Some(k) = o.keys().next() {
        return Err(format!("unknown option {k}"));
    }
    let mut specs = Vec::new();
    for c in clients {
        let p: Vec<&str> = c.split(',').collect();
        let [name, role, prefix, file] = p[..] else {
            return Err("--client NAME,ROLE,PREFIX,TOKEN_FILE".into());
        };
        let role = Role::parse(role).ok_or("client role must be trade, observe or admin")?;
        let (name, prefix, token) = (name.into(), prefix.into(), token(file)?);
        specs.push(ClientSpec {
            name,
            role,
            prefix,
            token,
        });
    }
    if listen_any {
        check_state(&state)?;
    } else {
        check_paths(listen, &state)?;
    }
    let cfg = Config {
        account,
        environment,
        listen,
        state,
        clients: specs,
        profile,
        timing,
        stall_after_admitted: stall,
    };
    Ok((cfg, [script, quotes, credentials]))
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "--help" || a == "-h") {
        print!("{HELP}");
        return ExitCode::SUCCESS;
    }
    let (cfg, [script, quotes, credentials]) = match parse(&args) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("order-gateway: {e}");
            return ExitCode::from(2);
        }
    };
    if let Some(path) = credentials {
        let creds = if path == "env" {
            credentials_from_env()
        } else {
            load_credentials(std::path::Path::new(&path))
        };
        return match creds {
            Ok(c) => serve(&cfg, Ok(AlpacaPaper::paper(c))),
            Err(e) => {
                eprintln!("order-gateway: {e}");
                ExitCode::from(2)
            }
        };
    }
    let script = match script.map(std::fs::read_to_string).transpose() {
        Ok(s) => s.unwrap_or_else(|| "{}".into()),
        Err(e) => {
            eprintln!("order-gateway: --sim-script: {e}");
            return ExitCode::from(2);
        }
    };
    let sim = match &quotes {
        Some(q) => SimBroker::market(&script, q),
        None => SimBroker::scripted(&script),
    };
    serve(&cfg, sim)
}

/// Runs one account until SIGINT/SIGTERM or a fatal session error.
fn serve<B: Broker + Send + 'static>(cfg: &Config, broker: Result<B, String>) -> ExitCode {
    let gw = match broker.and_then(|b| start(cfg.clone(), b)) {
        Ok(g) => g,
        Err(e) => {
            eprintln!("order-gateway: {e}");
            return ExitCode::from(1);
        }
    };
    eprintln!(
        "order-gateway: account {} serving on {}",
        cfg.account,
        gw.addr()
    );
    let (tx, rx) = channel();
    match Signals::new([SIGINT, SIGTERM]) {
        Ok(mut signals) => {
            std::thread::spawn(move || {
                if signals.forever().next().is_some() {
                    let _ = tx.send(());
                }
            });
        }
        Err(e) => {
            eprintln!("order-gateway: signal install failed: {e}");
            return ExitCode::from(1);
        }
    }
    while !gw.finished() {
        match rx.recv_timeout(Duration::from_millis(200)) {
            Ok(()) | Err(RecvTimeoutError::Disconnected) => break,
            Err(RecvTimeoutError::Timeout) => {}
        }
    }
    match gw.stop() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("order-gateway: {e}");
            ExitCode::from(1)
        }
    }
}

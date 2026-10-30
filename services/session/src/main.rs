//! `tinytrader-session run CONFIG --dir RUN_DIR [--listen IP]`: starts a backtest
//! session, prints its `READY` line, serves one strategy and exits 0 when the run is
//! complete, 1 when it isn't, 2 on bad usage.
use session::Session;
use std::net::IpAddr;
use std::path::PathBuf;
use std::process::ExitCode;

const USAGE: &str = "usage: tinytrader-session run CONFIG --dir RUN_DIR [--listen IP]";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (Some("run"), Some(config)) = (args.first().map(String::as_str), args.get(1)) else {
        eprintln!("{USAGE}");
        return ExitCode::from(2);
    };
    let mut dir = None;
    let mut ip: IpAddr = [127, 0, 0, 1].into();
    let mut it = args[2..].iter();
    while let Some(a) = it.next() {
        match (a.as_str(), it.next()) {
            ("--dir", Some(d)) => dir = Some(PathBuf::from(d)),
            ("--listen", Some(l)) => match l.parse() {
                Ok(x) => ip = x,
                Err(e) => {
                    eprintln!("--listen {l}: {e}");
                    return ExitCode::from(2);
                }
            },
            _ => {
                eprintln!("{USAGE}");
                return ExitCode::from(2);
            }
        }
    }
    let Some(dir) = dir else {
        eprintln!("{USAGE}");
        return ExitCode::from(2);
    };
    let text = match std::fs::read_to_string(config) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("{config}: {e}");
            return ExitCode::from(2);
        }
    };
    let s = match Session::start(&text, &dir, ip) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("session: {e}");
            return ExitCode::from(1);
        }
    };
    println!("{}", s.ready().line());
    match s.run() {
        Ok(o) => {
            eprintln!(
                "session: {} ({} steps, {} records) audit {}{}",
                o.status,
                o.steps,
                o.records,
                o.audit,
                o.reason.map(|r| format!(": {r}")).unwrap_or_default()
            );
            if o.status == "complete" {
                ExitCode::SUCCESS
            } else {
                ExitCode::from(1)
            }
        }
        Err(e) => {
            eprintln!("session: {e}");
            ExitCode::from(1)
        }
    }
}

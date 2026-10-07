//! Terminal view of a tinytrader deployment, with five screens:
//!
//! - **Overview:** alerts, the schedule's bundles, accounts, prices, the next
//!   economic releases and the store's room.
//! - **Services:** every service with its status, details of the selected
//!   one (and a relay's clients), and its live event tail, said plainly.
//! - **Market:** accounts with their positions, prices with sparklines, and
//!   the economic calendar.
//! - **Data:** the store's drives as a pool, and what fills each.
//! - **Hosts:** an htop-style view of one machine: CPU (with its recent
//!   history), memory, swap, load and disks (from the host's node_exporter),
//!   and the tinytrader services on it with CPU and memory against limits.
//!
//! ```text
//! tinytrader-tui --console http://localhost:8080          # read a running console
//! tinytrader-tui --topology FILE                           # poll the services it lists
//!             [--docker URL --project NAME]             # ...and a Compose project
//! ```
//!
//! Direct mode polls services itself: those listed in the topology file with
//! an `observe` address, and with `--docker`, a Compose project's labelled
//! containers (reachable only from inside that project's network).
//! `--console` works from anywhere the console is reachable, including over
//! SSH.
//!
//! Keys: 1–5 or Tab/Shift-Tab screens · ↑/↓ or j/k select · p pause
//! events · ←/→ or h/l switch host · c/m/n sort by CPU, memory, name · q quit.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use ratatui::crossterm::event::{self, Event, KeyCode, KeyEventKind};
use ratatui::layout::{Constraint, Layout};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::{DefaultTerminal, Frame};
use tinytrader_fleet::{FleetEvent, Poller, PollerConfig, Snapshot, Source, Topology};

mod data;
mod fmt;
mod screens;
mod theme;

/// What the screen shows, filled in by the background refresh task.
#[derive(Default)]
struct View {
    topology: Topology,
    snapshot: Snapshot,
    events: Vec<FleetEvent>,
    events_after: u64,
    events_for: Option<String>,
    selected: Option<String>,
    paused: bool,
    error: Option<String>,
    updated: Option<Instant>,
    screen: Screen,
    host: usize,
    account: usize,
    sort: Sort,
    /// Each host's CPU use, once per refresh, newest last.
    cpu_history: HashMap<String, VecDeque<u64>>,
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum Screen {
    #[default]
    Overview,
    Services,
    Market,
    Data,
    Hosts,
}

const SCREENS: [(Screen, &str); 5] = [
    (Screen::Overview, "Overview"),
    (Screen::Services, "Services"),
    (Screen::Market, "Market"),
    (Screen::Data, "Data"),
    (Screen::Hosts, "Hosts"),
];

#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum Sort {
    #[default]
    Cpu,
    Memory,
    Name,
}

fn main() {
    let (source, label) = match parse_args() {
        Ok(s) => s,
        Err(e) => {
            eprintln!(
                "tinytrader-tui: {e}\n\nusage: tinytrader-tui --console URL\n       tinytrader-tui --topology FILE [--docker URL --project NAME]"
            );
            std::process::exit(2);
        }
    };
    let view = Arc::new(Mutex::new(View::default()));
    let runtime = tokio::runtime::Runtime::new().expect("tokio runtime");
    runtime.spawn(refresh_forever(source, view.clone()));

    let mut terminal = ratatui::init();
    let result = run(&mut terminal, &view, &label);
    ratatui::restore();
    if let Err(e) = result {
        eprintln!("tinytrader-tui: {e}");
    }
}

fn parse_args() -> Result<(Source, String), String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let value = |flag: &str| {
        args.iter()
            .position(|a| a == flag)
            .and_then(|i| args.get(i + 1).cloned())
    };
    if let Some(url) = value("--console") {
        return Ok((Source::console(&url), format!("console {url}")));
    }
    let docker = value("--docker");
    let topology = value("--topology");
    if docker.is_some() || topology.is_some() {
        let config = PollerConfig {
            topology: topology.unwrap_or_else(|| "topology.json".into()),
            docker,
            project: value("--project"),
        };
        return Ok((
            Source::Direct(Arc::new(Poller::new(config))),
            "direct".into(),
        ));
    }
    Err("choose --console URL, or --topology FILE and/or --docker URL".into())
}

/// Poll the source about once a second, plus the selected service's events.
async fn refresh_forever(source: Source, view: Arc<Mutex<View>>) {
    loop {
        let problem = source.refresh().await;
        let topology = source.topology().await;
        let snapshot = source.snapshot().await;
        let (selected, after, paused, same) = {
            let v = lock(&view);
            (
                v.selected.clone(),
                v.events_after,
                v.paused,
                v.events_for == v.selected,
            )
        };
        let page = match (&selected, paused) {
            (Some(id), false) => Some(source.events(id, if same { after } else { 0 }, 100).await),
            _ => None,
        };
        {
            let mut v = lock(&view);
            match (topology, snapshot) {
                (Ok(t), Ok(s)) => {
                    for (id, h) in &s.hosts {
                        let hist = v.cpu_history.entry(id.clone()).or_default();
                        hist.push_back(
                            h.cpu_percent.unwrap_or(0.0).round().clamp(0.0, 100.0) as u64
                        );
                        while hist.len() > 600 {
                            hist.pop_front();
                        }
                    }
                    v.topology = t;
                    v.snapshot = s;
                    v.error = problem;
                    v.updated = Some(Instant::now());
                }
                (Err(e), _) | (_, Err(e)) => v.error = Some(e),
            }
            if v.selected.is_none() {
                v.selected = v
                    .topology
                    .focus
                    .clone()
                    .or_else(|| v.topology.services.first().map(|s| s.id.clone()));
            }
            if let Some(Ok(p)) = page {
                if !same || p.next < v.events_after {
                    v.events.clear();
                }
                for e in p.events {
                    v.events.insert(0, e);
                }
                v.events.truncate(500);
                v.events_after = p.next;
                v.events_for = selected;
            } else if !same {
                v.events.clear();
                v.events_after = 0;
                v.events_for = selected;
            }
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
}

fn run(terminal: &mut DefaultTerminal, view: &Mutex<View>, label: &str) -> std::io::Result<()> {
    loop {
        terminal.draw(|f| draw(f, &lock(view), label))?;
        if event::poll(Duration::from_millis(200))?
            && let Event::Key(key) = event::read()?
            && key.kind == KeyEventKind::Press
        {
            let mut v = lock(view);
            let at = SCREENS
                .iter()
                .position(|(s, _)| *s == v.screen)
                .unwrap_or(0);
            match key.code {
                KeyCode::Char('q') | KeyCode::Esc => return Ok(()),
                KeyCode::Char(c @ '1'..='5') => v.screen = SCREENS[usize::from(c as u8 - b'1')].0,
                KeyCode::Tab => v.screen = SCREENS[(at + 1) % SCREENS.len()].0,
                KeyCode::BackTab => v.screen = SCREENS[(at + SCREENS.len() - 1) % SCREENS.len()].0,
                KeyCode::Char('p') => v.paused = !v.paused,
                KeyCode::Down | KeyCode::Char('j') => step(&mut v, 1),
                KeyCode::Up | KeyCode::Char('k') => step(&mut v, -1),
                KeyCode::Right | KeyCode::Char('l') => v.host = v.host.saturating_add(1),
                KeyCode::Left | KeyCode::Char('h') => v.host = v.host.saturating_sub(1),
                KeyCode::Char('c') => v.sort = Sort::Cpu,
                KeyCode::Char('m') => v.sort = Sort::Memory,
                KeyCode::Char('n') => v.sort = Sort::Name,
                _ => {}
            }
        }
    }
}

/// Move the current screen's selection: a service, or an account.
fn step(v: &mut View, by: isize) {
    match v.screen {
        Screen::Market => {
            let n = data::accounts(&v.snapshot).len().max(1);
            v.account = (v.account as isize + by).rem_euclid(n as isize) as usize;
        }
        _ => {
            let ids: Vec<&String> = v.topology.services.iter().map(|s| &s.id).collect();
            if ids.is_empty() {
                return;
            }
            let at = v
                .selected
                .as_ref()
                .and_then(|s| ids.iter().position(|i| *i == s))
                .unwrap_or(0);
            let next = (at as isize + by).rem_euclid(ids.len() as isize) as usize;
            v.selected = Some(ids[next].clone());
        }
    }
}

fn draw(f: &mut Frame, v: &View, label: &str) {
    let [top, body, keys] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(5),
        Constraint::Length(1),
    ])
    .areas(f.area());
    let live = v
        .updated
        .is_some_and(|t| t.elapsed() < Duration::from_secs(6));
    let alerts = data::alerts(&v.topology, &v.snapshot).len();
    let (_, hm) = fmt::now_ny();
    let mut header = vec![
        Span::styled("tinytrader", theme::strong()),
        Span::styled(
            ".",
            Style::new().fg(theme::ACCENT).add_modifier(Modifier::BOLD),
        ),
        Span::styled("io ", theme::strong()),
    ];
    for (i, (s, name)) in SCREENS.iter().enumerate() {
        let on = *s == v.screen;
        header.push(Span::styled(
            format!(" {} {name} ", i + 1),
            if on {
                Style::new()
                    .fg(theme::FG)
                    .bg(theme::SELECT)
                    .add_modifier(Modifier::BOLD)
            } else {
                theme::dim()
            },
        ));
    }
    let right = vec![
        Span::styled(
            if alerts > 0 {
                format!("▲ {alerts} ")
            } else {
                String::new()
            },
            Style::new().fg(theme::WARN).add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            "● ",
            Style::new().fg(if live { theme::OK } else { theme::WARN }),
        ),
        Span::styled(if live { "live " } else { "waiting " }, theme::soft()),
        Span::styled(format!("{hm} ET "), theme::text()),
    ];
    let width: usize = right.iter().map(|s| s.content.chars().count()).sum();
    let [left_area, right_area] =
        Layout::horizontal([Constraint::Min(10), Constraint::Length(width as u16)]).areas(top);
    f.render_widget(Line::from(header), left_area);
    f.render_widget(Line::from(right).right_aligned(), right_area);

    match v.screen {
        Screen::Overview => screens::overview(f, body, v),
        Screen::Services => screens::services(f, body, v),
        Screen::Market => screens::market(f, body, v),
        Screen::Data => screens::data_screen(f, body, v),
        Screen::Hosts => screens::hosts(f, body, v),
    }
    let mut hints = theme::hints(&[("1–5", "screens"), ("Tab", "next")]);
    let more: &[(&str, &str)] = match v.screen {
        Screen::Services => &[
            ("↑↓", "service"),
            (
                "p",
                if v.paused {
                    "resume events"
                } else {
                    "pause events"
                },
            ),
        ],
        Screen::Market => &[("↑↓", "account")],
        Screen::Hosts => &[("←→", "host"), ("c m n", "sort by CPU, memory, name")],
        _ => &[],
    };
    if !more.is_empty() {
        hints.push(Span::raw("  "));
        hints.extend(theme::hints(more));
    }
    hints.push(Span::raw("  "));
    hints.extend(theme::hints(&[("q", "quit")]));
    let mut footer = hints;
    if let Some(e) = &v.error {
        footer.push(Span::styled(
            format!("   {e}"),
            Style::new().fg(theme::WARN),
        ));
    } else {
        footer.push(Span::styled(format!("   {label}"), theme::dim()));
    }
    f.render_widget(Line::from(footer), keys);
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

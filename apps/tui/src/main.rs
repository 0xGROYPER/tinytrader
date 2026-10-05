//! Terminal view of a Glacier deployment, with two screens:
//!
//! - **Services:** every service with its status, details of the selected
//!   one, and its live event tail.
//! - **Hosts:** an htop-style view of one machine: CPU, memory, swap, load
//!   and disks (from the host's node_exporter), and the Glacier services on
//!   it with CPU and memory against their limits.
//!
//! ```text
//! glacier-tui --console http://localhost:8080          # read a running console
//! glacier-tui --topology FILE                           # poll the services it lists
//!             [--docker URL --project NAME]             # ...and a Compose project
//! ```
//!
//! Direct mode polls services itself: those listed in the topology file with
//! an `observe` address, and with `--docker`, a Compose project's labelled
//! containers (reachable only from inside that project's network).
//! `--console` works from anywhere the console is reachable, including over
//! SSH.
//!
//! Keys: Tab switch screen · ↑/↓ or j/k select · p pause events ·
//! ←/→ or h/l switch host · c/m/n sort by CPU, memory, name · q quit.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use glacier_fleet::{
    FleetEvent, HostState, Poller, PollerConfig, ServiceState, Snapshot, Source, Status, Topology,
};
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEventKind};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, Cell, Gauge, LineGauge, List, ListItem, ListState, Paragraph, Row, Table, Wrap,
};
use ratatui::{DefaultTerminal, Frame};

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
    sort: Sort,
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum Screen {
    #[default]
    Services,
    Hosts,
}

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
                "glacier-tui: {e}\n\nusage: glacier-tui --console URL\n       glacier-tui --topology FILE [--docker URL --project NAME]"
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
        eprintln!("glacier-tui: {e}");
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
            match key.code {
                KeyCode::Char('q') | KeyCode::Esc => return Ok(()),
                KeyCode::Char('p') => v.paused = !v.paused,
                KeyCode::Tab => {
                    v.screen = if v.screen == Screen::Services {
                        Screen::Hosts
                    } else {
                        Screen::Services
                    };
                }
                KeyCode::Down | KeyCode::Char('j') => move_selection(&mut v, 1),
                KeyCode::Up | KeyCode::Char('k') => move_selection(&mut v, -1),
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

fn move_selection(v: &mut View, step: isize) {
    let ids: Vec<&String> = v.topology.services.iter().map(|s| &s.id).collect();
    if ids.is_empty() {
        return;
    }
    let at = v
        .selected
        .as_ref()
        .and_then(|s| ids.iter().position(|i| *i == s))
        .unwrap_or(0);
    let next = (at as isize + step).rem_euclid(ids.len() as isize) as usize;
    v.selected = Some(ids[next].clone());
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
    let mut header = vec![
        Span::styled("Glacier Labs", Style::new().bold()),
        Span::raw(" / "),
        Span::raw(if v.topology.name.is_empty() {
            "…"
        } else {
            &v.topology.name
        }),
        Span::raw("   "),
        Span::styled(
            "●",
            Style::new().fg(if live { Color::Green } else { Color::Yellow }),
        ),
        Span::raw(if live { " live" } else { " waiting" }),
        Span::styled(format!("   {label}"), Style::new().dark_gray()),
    ];
    if let Some(e) = &v.error {
        header.push(Span::styled(format!("   {e}"), Style::new().yellow()));
    }
    f.render_widget(Line::from(header), top);

    match v.screen {
        Screen::Services => draw_services(f, body, v),
        Screen::Hosts => draw_hosts(f, body, v),
    }
    let help = match v.screen {
        Screen::Services => "Tab hosts · ↑/↓ select · p pause events · q quit",
        Screen::Hosts => "Tab services · ←/→ host · c/m/n sort by CPU, memory, name · q quit",
    };
    f.render_widget(Line::from(help.dark_gray()), keys);
}

fn draw_services(f: &mut Frame, body: Rect, v: &View) {
    let [left, right] =
        Layout::horizontal([Constraint::Percentage(34), Constraint::Min(30)]).areas(body);
    let items: Vec<ListItem> = v
        .topology
        .services
        .iter()
        .map(|s| {
            let state = v.snapshot.services.get(&s.id);
            let status = state.map_or(Status::Idle, glacier_fleet::ServiceState::status);
            ListItem::new(Line::from(vec![
                Span::styled("● ", Style::new().fg(colour(status))),
                Span::raw(s.name().to_owned()),
                Span::styled(
                    format!(
                        "  {}",
                        state
                            .map(glacier_fleet::ServiceState::detail)
                            .unwrap_or_default()
                    ),
                    Style::new().dark_gray(),
                ),
            ]))
        })
        .collect();
    let mut list_state = ListState::default().with_selected(
        v.selected
            .as_ref()
            .and_then(|id| v.topology.services.iter().position(|s| &s.id == id)),
    );
    f.render_stateful_widget(
        List::new(items)
            .block(Block::bordered().title(" Services "))
            .highlight_style(Style::new().add_modifier(Modifier::REVERSED)),
        left,
        &mut list_state,
    );

    let [details, events] =
        Layout::vertical([Constraint::Length(8), Constraint::Min(3)]).areas(right);
    let selected = v
        .selected
        .as_ref()
        .and_then(|id| v.topology.services.iter().find(|s| &s.id == id));
    let mut lines = Vec::new();
    if let Some(s) = selected {
        let st = v.snapshot.services.get(&s.id);
        let health = st
            .and_then(|x| x.health.as_ref())
            .and_then(|h| h.body.as_ref());
        let container = st.and_then(|x| x.container.as_ref());
        lines.push(Line::from(s.role.clone().unwrap_or_default()).italic());
        lines.push(field(
            "State",
            health
                .and_then(|h| h.state.clone())
                .or_else(|| container.and_then(|c| c.state.clone())),
        ));
        lines.push(field("Detail", st.map(glacier_fleet::ServiceState::detail)));
        lines.push(field(
            "Uptime",
            health.and_then(|h| h.uptime_seconds).map(duration),
        ));
        lines.push(field(
            "Restarts",
            container
                .and_then(|c| c.restart_count)
                .map(|r| r.to_string()),
        ));
        lines.push(field(
            "CPU",
            container
                .and_then(|c| c.cpu_percent)
                .map(|c| format!("{c:.1}%")),
        ));
        if let Some(e) = health.and_then(|h| h.last_error.clone()) {
            lines.push(field("Last error", Some(e)));
        }
    }
    f.render_widget(
        Paragraph::new(lines)
            .wrap(Wrap { trim: true })
            .block(Block::bordered().title(format!(" {} ", selected.map_or("", |s| s.name())))),
        details,
    );

    let has_events = selected.is_some_and(|s| s.events.is_some());
    let rows: Vec<ListItem> = if has_events {
        v.events
            .iter()
            .take(events.height as usize)
            .map(|e| {
                ListItem::new(Line::from(vec![
                    Span::styled(clock(e.received_ms), Style::new().dark_gray()),
                    Span::raw("  "),
                    Span::raw(e.summary.clone()),
                ]))
            })
            .collect()
    } else {
        vec![ListItem::new(
            "No event stream for this service.".dark_gray(),
        )]
    };
    let title = if v.paused {
        " Live events (paused) "
    } else {
        " Live events "
    };
    f.render_widget(
        List::new(rows).block(Block::bordered().title(title)),
        events,
    );
}

fn field(name: &str, value: Option<String>) -> Line<'static> {
    Line::from(vec![
        Span::styled(format!("{name:<11}"), Style::new().dark_gray()),
        Span::raw(value.unwrap_or_else(|| "—".into())),
    ])
}

fn colour(s: Status) -> Color {
    match s {
        Status::Ok => Color::Green,
        Status::Warn => Color::Yellow,
        Status::Crit => Color::Red,
        Status::Idle => Color::DarkGray,
    }
}

fn duration(s: f64) -> String {
    let s = s.max(0.0) as u64;
    let (d, h, m) = (s / 86_400, s % 86_400 / 3600, s % 3600 / 60);
    if d > 0 {
        format!("{d}d {h}h")
    } else if h > 0 {
        format!("{h}h {m}m")
    } else {
        format!("{m}m {}s", s % 60)
    }
}

/// `HH:MM:SS.mmm` UTC for a Unix time in milliseconds.
fn clock(ms: f64) -> String {
    let ms = ms.max(0.0) as u64;
    let s = ms / 1000;
    format!(
        "{:02}:{:02}:{:02}.{:03}",
        s / 3600 % 24,
        s / 60 % 60,
        s % 60,
        ms % 1000
    )
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn draw_hosts(f: &mut Frame, body: Rect, v: &View) {
    let hosts = &v.topology.hosts;
    if hosts.is_empty() {
        f.render_widget(
            Paragraph::new("No hosts in this deployment's topology.".dark_gray()),
            body,
        );
        return;
    }
    let index = v.host.min(hosts.len() - 1);
    let host = &hosts[index];
    let state = v.snapshot.hosts.get(&host.id);
    let disk_rows = state.map_or(1, |h| h.disks.len().max(1)) as u16;
    let [machine, disks, services] = Layout::vertical([
        Constraint::Length(6),
        Constraint::Length(2 + disk_rows),
        Constraint::Min(4),
    ])
    .areas(body);

    let title = format!(
        " {} {}{}",
        host.label.as_deref().unwrap_or(&host.id),
        host.sub
            .as_deref()
            .map(|s| format!("· {s} "))
            .unwrap_or_default(),
        if hosts.len() > 1 {
            format!("({}/{}) ", index + 1, hosts.len())
        } else {
            String::new()
        },
    );
    let block = Block::bordered().title(title);
    let inner = block.inner(machine);
    f.render_widget(block, machine);
    match state {
        None => f.render_widget(
            Paragraph::new(
                "No machine figures: give this host an `exporter` (its node_exporter metrics URL) in the topology."
                    .dark_gray(),
            )
            .wrap(Wrap { trim: true }),
            inner,
        ),
        Some(h) if h.error.is_some() => f.render_widget(
            Paragraph::new(format!(
                "node_exporter unreachable: {}",
                h.error.as_deref().unwrap_or("")
            ))
            .yellow(),
            inner,
        ),
        Some(h) => draw_machine(f, inner, h),
    }

    let block = Block::bordered().title(" Disks ");
    let inner = block.inner(disks);
    f.render_widget(block, disks);
    if let Some(h) = state {
        let rows = Layout::vertical(vec![Constraint::Length(1); h.disks.len()]).split(inner);
        for (d, area) in h.disks.iter().zip(rows.iter()) {
            let used = if d.total_bytes > 0.0 {
                1.0 - d.available_bytes / d.total_bytes
            } else {
                0.0
            };
            f.render_widget(
                LineGauge::default()
                    .label(format!(
                        "{:<20} {:>4.0}% {:>10} free ",
                        d.mount,
                        used * 100.0,
                        bytes(d.available_bytes)
                    ))
                    .ratio(used.clamp(0.0, 1.0))
                    .filled_style(Style::new().fg(threshold_colour(used * 100.0))),
                *area,
            );
        }
    }

    let mut rows: Vec<(&glacier_fleet::ServiceInfo, Option<&ServiceState>)> = v
        .topology
        .services
        .iter()
        .filter(|s| s.host.as_deref().unwrap_or("local") == host.id)
        .map(|s| (s, v.snapshot.services.get(&s.id)))
        .collect();
    let cpu = |s: Option<&ServiceState>| {
        s.and_then(|s| s.container.as_ref())
            .and_then(|c| c.cpu_percent)
            .unwrap_or(-1.0)
    };
    let mem = |s: Option<&ServiceState>| {
        s.and_then(|s| s.container.as_ref())
            .and_then(|c| c.memory_use().0)
            .unwrap_or(-1.0)
    };
    match v.sort {
        Sort::Cpu => rows.sort_by(|a, b| cpu(b.1).total_cmp(&cpu(a.1))),
        Sort::Memory => rows.sort_by(|a, b| mem(b.1).total_cmp(&mem(a.1))),
        Sort::Name => rows.sort_by(|a, b| a.0.name().cmp(b.0.name())),
    }
    let table: Vec<Row> = rows
        .iter()
        .map(|(svc, st)| {
            let status = st.map_or(Status::Idle, |s| s.status());
            let c = st.and_then(|s| s.container.as_ref());
            let (used, limit) = c
                .map(glacier_fleet::Container::memory_use)
                .unwrap_or_default();
            let health = st
                .and_then(|s| s.health.as_ref())
                .and_then(|h| h.body.as_ref());
            Row::new(vec![
                Cell::from(Line::from(vec![
                    Span::styled("● ", Style::new().fg(colour(status))),
                    Span::raw(svc.name().to_owned()),
                ])),
                Cell::from(
                    health
                        .and_then(|h| h.state.clone())
                        .or_else(|| c.and_then(|c| c.state.clone()))
                        .unwrap_or_default(),
                ),
                Cell::from(
                    c.and_then(|c| c.cpu_percent)
                        .map_or_else(|| "—".into(), |p| format!("{p:.1}%")),
                ),
                Cell::from(match (used, limit) {
                    (Some(u), Some(l)) => format!("{} / {}", bytes(u), bytes(l)),
                    (Some(u), None) => bytes(u),
                    _ => "—".into(),
                }),
                Cell::from(
                    c.and_then(|c| c.restart_count)
                        .map_or_else(|| "—".into(), |r| r.to_string()),
                ),
                Cell::from(
                    health
                        .and_then(|h| h.uptime_seconds)
                        .map_or_else(|| "—".into(), duration),
                ),
                Cell::from(st.map(ServiceState::detail).unwrap_or_default()).dark_gray(),
            ])
        })
        .collect();
    let sort = match v.sort {
        Sort::Cpu => "CPU",
        Sort::Memory => "memory",
        Sort::Name => "name",
    };
    f.render_widget(
        Table::new(
            table,
            [
                Constraint::Length(20),
                Constraint::Length(10),
                Constraint::Length(7),
                Constraint::Length(21),
                Constraint::Length(9),
                Constraint::Length(9),
                Constraint::Min(10),
            ],
        )
        .header(
            Row::new([
                "SERVICE",
                "STATE",
                "CPU",
                "MEM / LIMIT",
                "RESTARTS",
                "UP",
                "DETAIL",
            ])
            .bold(),
        )
        .block(Block::bordered().title(format!(" Glacier services · sorted by {sort} "))),
        services,
    );
}

fn draw_machine(f: &mut Frame, area: Rect, h: &HostState) {
    let [cpu, mem, swap, info] = Layout::vertical([Constraint::Length(1); 4]).areas(area);
    let pct = h.cpu_percent.unwrap_or(0.0);
    f.render_widget(
        Gauge::default()
            .label(format!(
                "CPU {} · {} cores",
                h.cpu_percent
                    .map_or_else(|| "…".into(), |p| format!("{p:.0}%")),
                h.cores.unwrap_or(0)
            ))
            .ratio((pct / 100.0).clamp(0.0, 1.0))
            .gauge_style(Style::new().fg(threshold_colour(pct))),
        cpu,
    );
    for (name, u, area) in [("Memory", &h.memory, mem), ("Swap", &h.swap, swap)] {
        let (label, ratio) = match (u.used_bytes, u.total_bytes) {
            (Some(used), Some(total)) if total > 0.0 => (
                format!("{name} {} / {}", bytes(used), bytes(total)),
                (used / total).clamp(0.0, 1.0),
            ),
            _ => (format!("{name} —"), 0.0),
        };
        f.render_widget(
            Gauge::default()
                .label(label)
                .ratio(ratio)
                .gauge_style(Style::new().fg(threshold_colour(ratio * 100.0))),
            area,
        );
    }
    let load: Vec<String> = h.load.iter().flatten().map(|l| format!("{l:.2}")).collect();
    f.render_widget(
        Line::from(format!(
            "load {}   up {}",
            if load.is_empty() {
                "—".into()
            } else {
                load.join(" ")
            },
            h.uptime_seconds.map_or_else(|| "—".into(), duration)
        ))
        .dark_gray(),
        info,
    );
}

/// Green below the store's warning threshold, then yellow, then red.
fn threshold_colour(pct: f64) -> Color {
    if pct >= 90.0 {
        Color::Red
    } else if pct >= 80.0 {
        Color::Yellow
    } else {
        Color::Green
    }
}

fn bytes(n: f64) -> String {
    let units = ["B", "KB", "MB", "GB", "TB"];
    let (mut n, mut i) = (n.max(0.0), 0);
    while n >= 1024.0 && i < units.len() - 1 {
        n /= 1024.0;
        i += 1;
    }
    if i == 0 {
        format!("{n:.0} {}", units[i])
    } else {
        format!("{n:.1} {}", units[i])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_time() {
        assert_eq!(clock(3_723_004.0), "01:02:03.004");
        assert_eq!(duration(3_725.0), "1h 2m");
        assert_eq!(duration(90_000.0), "1d 1h");
    }
}

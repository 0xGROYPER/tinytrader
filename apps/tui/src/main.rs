//! Terminal view of a Glacier deployment: every service with its status,
//! details of the selected one, and its live event tail.
//!
//! ```text
//! glacier-tui --console http://localhost:8080          # read a running console
//! glacier-tui --docker http://localhost:2375 \
//!             --project NAME [--topology FILE]          # poll the deployment directly
//! ```
//!
//! Direct mode needs to reach the services at the addresses discovery finds
//! (Compose service names), so it suits running inside the deployment's
//! network or on its host; `--console` works from anywhere the console is
//! reachable, including over SSH.
//!
//! Keys: ↑/↓ or j/k select · p pause events · q quit.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use glacier_fleet::{FleetEvent, Poller, PollerConfig, Snapshot, Source, Status, Topology};
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEventKind};
use ratatui::layout::{Constraint, Layout};
use ratatui::style::{Color, Modifier, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, List, ListItem, ListState, Paragraph, Wrap};
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
}

fn main() {
    let (source, label) = match parse_args() {
        Ok(s) => s,
        Err(e) => {
            eprintln!(
                "glacier-tui: {e}\n\nusage: glacier-tui --console URL\n       glacier-tui --docker URL --project NAME [--topology FILE]"
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
    if let Some(docker) = value("--docker") {
        let config = PollerConfig {
            topology: value("--topology").unwrap_or_else(|| "topology.json".into()),
            docker: Some(docker),
            project: value("--project"),
        };
        return Ok((
            Source::Direct(Arc::new(Poller::new(config))),
            "direct".into(),
        ));
    }
    Err("choose --console or --docker".into())
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
                KeyCode::Down | KeyCode::Char('j') => move_selection(&mut v, 1),
                KeyCode::Up | KeyCode::Char('k') => move_selection(&mut v, -1),
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

    f.render_widget(
        Line::from("↑/↓ select · p pause events · q quit".dark_gray()),
        keys,
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

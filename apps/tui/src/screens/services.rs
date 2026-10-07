//! Services: each service, its details (and a relay's clients), and its event tail.

use super::*;

pub fn services(f: &mut Frame, area: Rect, v: &View) {
    let [left, right] =
        Layout::horizontal([Constraint::Length(44), Constraint::Min(30)]).areas(area);
    let items: Vec<ListItem> = v
        .topology
        .services
        .iter()
        .map(|s| {
            let st = v.snapshot.services.get(&s.id);
            let status = st.map_or(Status::Idle, ServiceState::status);
            let state = st
                .and_then(|x| x.health.as_ref())
                .and_then(|h| h.body.as_ref())
                .and_then(|b| b.state.clone())
                .unwrap_or_default();
            ListItem::new(Line::from(vec![
                Span::styled("● ", Style::new().fg(theme::status(status))),
                Span::styled(format!("{:<22}", s.name()), strong()),
                Span::styled(state, Style::new().fg(theme::status(status))),
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
            .block(panel(
                &format!("Services · {}", v.topology.services.len()),
                true,
            ))
            .highlight_style(theme::selected())
            .highlight_symbol("▌"),
        left,
        &mut list_state,
    );

    let selected = v
        .selected
        .as_ref()
        .and_then(|id| v.topology.services.iter().find(|s| &s.id == id));
    let Some(svc) = selected else {
        empty(f, right, "Details", "Select a service.");
        return;
    };
    let st = v.snapshot.services.get(&svc.id);
    let health = st
        .and_then(|x| x.health.as_ref())
        .and_then(|h| h.body.as_ref());
    let c = st.and_then(|x| x.container.as_ref());
    let clients = data::clients(&v.snapshot, svc);
    let mut lines = vec![
        Line::styled(
            svc.role.clone().unwrap_or_default(),
            soft().add_modifier(Modifier::ITALIC),
        ),
        Line::raw(""),
    ];
    let pair = |k1: &str, v1: String, k2: &str, v2: String| {
        Line::from(vec![
            Span::styled(format!("{k1:<10}"), dim()),
            Span::styled(format!("{v1:<24}"), text()),
            Span::styled(format!("{k2:<10}"), dim()),
            Span::styled(v2, text()),
        ])
    };
    let status = st.map_or(Status::Idle, ServiceState::status);
    lines.push(Line::from(vec![
        Span::styled(format!("{:<10}", "State"), dim()),
        Span::styled(
            health
                .and_then(|h| h.state.clone())
                .unwrap_or_else(|| "—".into()),
            Style::new()
                .fg(theme::status(status))
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("  {}", st.map(ServiceState::detail).unwrap_or_default()),
            soft(),
        ),
    ]));
    lines.push(pair(
        "Uptime",
        health
            .and_then(|h| h.uptime_seconds)
            .map_or_else(|| "—".into(), fmt::duration),
        "Restarts",
        c.and_then(|c| c.restart_count)
            .map_or_else(|| "—".into(), |r| r.to_string()),
    ));
    let (used, limit) = c
        .map(tinytrader_fleet::Container::memory_use)
        .unwrap_or_default();
    lines.push(pair(
        "CPU",
        c.and_then(|c| c.cpu_percent)
            .map_or_else(|| "—".into(), |p| format!("{p:.1}%")),
        "Memory",
        match (used, limit) {
            (Some(u), Some(l)) => format!("{} / {}", fmt::bytes(u), fmt::bytes(l)),
            (Some(u), None) => fmt::bytes(u),
            _ => "—".into(),
        },
    ));
    if let Some(r) = v.rates.get(&svc.id) {
        let mut spans = vec![Span::styled(format!("{:<10}", "Feed"), dim())];
        spans.extend(feed_spans(r));
        lines.push(Line::from(spans));
    }
    if let Some(e) = health.and_then(|h| h.last_error.clone()) {
        lines.push(Line::from(vec![
            Span::styled(format!("{:<10}", "Last error"), dim()),
            Span::styled(e, Style::new().fg(theme::WARN)),
        ]));
    }
    if !clients.is_empty() {
        lines.push(Line::raw(""));
        lines.push(Line::styled(format!("Clients · {}", clients.len()), dim()));
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0.0, |d| d.as_secs_f64());
        for cl in &clients {
            lines.push(Line::from(vec![
                Span::styled("  ● ", Style::new().fg(theme::OK)),
                Span::styled(
                    format!(
                        "{:<20}",
                        cl.name.clone().unwrap_or_else(|| "unnamed".into())
                    ),
                    if cl.name.is_some() { strong() } else { dim() },
                ),
                Span::styled(
                    cl.account
                        .as_ref()
                        .map_or_else(String::new, |a| format!("trades {a}  ")),
                    Style::new().fg(theme::ACCENT),
                ),
                Span::styled(
                    format!(
                        "{} for {}",
                        if cl.replay { "replaying" } else { "live" },
                        fmt::duration(now - cl.since)
                    ),
                    dim(),
                ),
            ]));
        }
    }
    let details_h = (lines.len() as u16 + 2).min(right.height / 2).max(6);
    let [details, events] =
        Layout::vertical([Constraint::Length(details_h), Constraint::Min(3)]).areas(right);
    f.render_widget(
        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .block(panel(svc.name(), false)),
        details,
    );

    let rows: Vec<ListItem> = if svc.events.is_some() {
        v.events
            .iter()
            .take(usize::from(events.height))
            .map(|e| {
                let (said, colour) = fmt::event(&e.summary);
                ListItem::new(Line::from(vec![
                    Span::styled(fmt::clock(e.received_ms), dim()),
                    Span::raw("  "),
                    Span::styled(said, Style::new().fg(colour)),
                ]))
            })
            .collect()
    } else {
        vec![ListItem::new(Span::styled(
            "No event stream for this service.",
            dim(),
        ))]
    };
    f.render_widget(
        List::new(rows).block(panel(
            if v.paused {
                "Live events · paused"
            } else {
                "Live events"
            },
            false,
        )),
        events,
    );
}

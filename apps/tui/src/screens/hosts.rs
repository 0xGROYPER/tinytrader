//! Hosts: a machine's CPU, memory, swap, load and disks, and the services on it.

use super::*;

pub fn hosts(f: &mut Frame, area: Rect, v: &View) {
    let hosts = &v.topology.hosts;
    if hosts.is_empty() {
        empty(f, area, "Hosts", "No hosts in this deployment's topology.");
        return;
    }
    let index = v.host.min(hosts.len() - 1);
    let host = &hosts[index];
    let state = v.snapshot.hosts.get(&host.id);
    let disk_rows = state.map_or(1, |h| h.disks.len().max(1)) as u16;
    let [machine, disks, table] = Layout::vertical([
        Constraint::Length(8),
        Constraint::Length(2 + disk_rows),
        Constraint::Min(4),
    ])
    .areas(area);
    let title = format!(
        "{}{}{}",
        host.label.as_deref().unwrap_or(&host.id),
        host.sub
            .as_deref()
            .map(|s| format!(" · {s}"))
            .unwrap_or_default(),
        if hosts.len() > 1 {
            format!(" ({}/{})", index + 1, hosts.len())
        } else {
            String::new()
        }
    );
    let block = panel(&title, false);
    let inner = block.inner(machine);
    f.render_widget(block, machine);
    match state {
        None => f.render_widget(Paragraph::new(Span::styled("No machine figures: give this host an `exporter` (its node_exporter metrics URL) in the topology.", dim())).wrap(Wrap { trim: true }), inner),
        Some(h) if h.error.is_some() => f.render_widget(Paragraph::new(Span::styled(format!("node_exporter unreachable: {}", h.error.as_deref().unwrap_or("")), Style::new().fg(theme::WARN))), inner),
        Some(h) => {
            let [gauges, spark] = Layout::horizontal([Constraint::Percentage(55), Constraint::Min(10)]).areas(inner);
            let rows = Layout::vertical([Constraint::Length(1); 6]).split(gauges);
            let meter = |f: &mut Frame, area: Rect, name: &str, ratio: f64, value: String| {
                let [label, bar, val] = Layout::horizontal([Constraint::Length(8), Constraint::Min(6), Constraint::Length(22)]).areas(area);
                f.render_widget(Span::styled(name.to_owned(), dim()), label);
                f.render_widget(LineGauge::default().ratio(ratio.clamp(0.0, 1.0)).label("").filled_style(Style::new().fg(theme::level(ratio * 100.0))).unfilled_style(Style::new().fg(theme::LINE)).filled_symbol(symbols::line::THICK.horizontal).unfilled_symbol(symbols::line::THICK.horizontal), bar);
                f.render_widget(Line::from(Span::styled(value, text())).right_aligned(), val);
            };
            let cpu = h.cpu_percent.unwrap_or(0.0);
            meter(f, rows[0], "CPU", cpu / 100.0, format!("{cpu:.0}% of {} cores", h.cores.unwrap_or(0)));
            for (i, (name, u)) in [("Memory", &h.memory), ("Swap", &h.swap)].into_iter().enumerate() {
                match (u.used_bytes, u.total_bytes) {
                    (Some(used), Some(total)) if total > 0.0 => meter(f, rows[i + 1], name, used / total, format!("{} / {}", fmt::bytes(used), fmt::bytes(total))),
                    _ => meter(f, rows[i + 1], name, 0.0, "—".into()),
                }
            }
            let load: Vec<String> = h.load.iter().flatten().map(|l| format!("{l:.2}")).collect();
            let cores = f64::from(h.cores.unwrap_or(1).max(1));
            let load1 = h.load.first().copied().flatten().unwrap_or(0.0);
            f.render_widget(
                Line::from(vec![
                    Span::styled(format!("{:<8}", "Load"), dim()),
                    Span::styled(if load.is_empty() { "—".into() } else { load.join("  ") }, Style::new().fg(theme::level(load1 / cores * 100.0))),
                    Span::styled(format!("   up {}", h.uptime_seconds.map_or_else(|| "—".into(), fmt::duration)), dim()),
                ]),
                rows[4],
            );
            let hist = v.cpu_history.get(&host.id).map(|d| d.iter().copied().collect::<Vec<u64>>()).unwrap_or_default();
            let tail = &hist[hist.len().saturating_sub(usize::from(spark.width.saturating_sub(2)))..];
            f.render_widget(Sparkline::default().data(tail).max(100).style(Style::new().fg(theme::ACCENT_2)).block(ratatui::widgets::Block::new().title(Span::styled(" CPU, last minutes", dim()))), spark);
        }
    }

    let block = panel("Disks", false);
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
            let [label, bar, val] = Layout::horizontal([
                Constraint::Length(20),
                Constraint::Min(6),
                Constraint::Length(24),
            ])
            .areas(*area);
            f.render_widget(Span::styled(d.mount.clone(), soft()), label);
            f.render_widget(
                LineGauge::default()
                    .ratio(used.clamp(0.0, 1.0))
                    .label("")
                    .filled_style(Style::new().fg(theme::level(used * 100.0)))
                    .unfilled_style(Style::new().fg(theme::LINE))
                    .filled_symbol(symbols::line::THICK.horizontal)
                    .unfilled_symbol(symbols::line::THICK.horizontal),
                bar,
            );
            f.render_widget(
                Line::from(vec![
                    Span::styled(
                        format!("{:>4.0}%", used * 100.0),
                        Style::new().fg(theme::level(used * 100.0)),
                    ),
                    Span::styled(format!("  {} free", fmt::bytes(d.available_bytes)), text()),
                ])
                .right_aligned(),
                val,
            );
        }
    }

    let mut rows: Vec<(&tinytrader_fleet::ServiceInfo, Option<&ServiceState>)> = v
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
    let body: Vec<Row> = rows
        .iter()
        .map(|(svc, st)| {
            let status = st.map_or(Status::Idle, |s| s.status());
            let c = st.and_then(|s| s.container.as_ref());
            let (used, limit) = c
                .map(tinytrader_fleet::Container::memory_use)
                .unwrap_or_default();
            let health = st
                .and_then(|s| s.health.as_ref())
                .and_then(|h| h.body.as_ref());
            let mem_pct = used.zip(limit).map(|(u, l)| u / l * 100.0).unwrap_or(0.0);
            Row::new(vec![
                Cell::from(Line::from(vec![
                    Span::styled("● ", Style::new().fg(theme::status(status))),
                    Span::styled(svc.name().to_owned(), strong()),
                ])),
                Cell::from(Span::styled(
                    health
                        .and_then(|h| h.state.clone())
                        .or_else(|| c.and_then(|c| c.state.clone()))
                        .unwrap_or_default(),
                    Style::new().fg(theme::status(status)),
                )),
                Cell::from(
                    Line::from(
                        c.and_then(|c| c.cpu_percent)
                            .map_or_else(|| "—".into(), |p| format!("{p:.1}%")),
                    )
                    .right_aligned(),
                ),
                Cell::from(
                    Line::from(Span::styled(
                        match (used, limit) {
                            (Some(u), Some(l)) => format!("{} / {}", fmt::bytes(u), fmt::bytes(l)),
                            (Some(u), None) => fmt::bytes(u),
                            _ => "—".into(),
                        },
                        Style::new().fg(if mem_pct >= 80.0 {
                            theme::level(mem_pct)
                        } else {
                            theme::FG
                        }),
                    ))
                    .right_aligned(),
                ),
                Cell::from(
                    Line::from(
                        c.and_then(|c| c.restart_count)
                            .map_or_else(|| "—".into(), |r| r.to_string()),
                    )
                    .right_aligned(),
                ),
                Cell::from(
                    Line::from(
                        health
                            .and_then(|h| h.uptime_seconds)
                            .map_or_else(|| "—".into(), fmt::duration),
                    )
                    .right_aligned(),
                ),
                Cell::from(Span::styled(
                    st.map(ServiceState::detail).unwrap_or_default(),
                    dim(),
                )),
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
            body,
            [
                Constraint::Length(22),
                Constraint::Length(10),
                Constraint::Length(7),
                Constraint::Length(22),
                Constraint::Length(9),
                Constraint::Length(9),
                Constraint::Min(10),
            ],
        )
        .header(Row::new(
            [
                "SERVICE",
                "STATE",
                "CPU",
                "MEM / LIMIT",
                "RESTARTS",
                "UP",
                "DETAIL",
            ]
            .map(|h| Cell::from(Span::styled(h, dim()))),
        ))
        .column_spacing(2)
        .block(panel(&format!("Services · sorted by {sort}"), true)),
        table,
    );
}

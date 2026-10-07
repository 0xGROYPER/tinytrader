//! Overview: alerts, the schedule, accounts, feed rates, the next releases and storage.

use super::*;

pub fn overview(f: &mut Frame, area: Rect, v: &View) {
    // The first two rows take what they hold; releases and storage the rest.
    let top_h = data::alerts(&v.topology, &v.snapshot)
        .len()
        .max(data::bundles(&v.topology, &v.snapshot).len() + 1)
        .max(1) as u16
        + 2;
    let mid_h = data::accounts(&v.snapshot)
        .len()
        .max(data::prices(&v.topology, &v.snapshot).len().min(12))
        .max(1) as u16
        + 2;
    let rows = Layout::vertical([
        Constraint::Length(top_h.clamp(4, 12)),
        Constraint::Length(mid_h.clamp(4, 14)),
        Constraint::Min(6),
    ])
    .split(area);
    let [alerts, schedule] =
        Layout::horizontal([Constraint::Percentage(50), Constraint::Percentage(50)]).areas(rows[0]);
    let [accounts, prices] =
        Layout::horizontal([Constraint::Percentage(50), Constraint::Percentage(50)]).areas(rows[1]);
    let [releases, storage] =
        Layout::horizontal([Constraint::Percentage(50), Constraint::Percentage(50)]).areas(rows[2]);

    let list = data::alerts(&v.topology, &v.snapshot);
    let lines: Vec<Line> = if list.is_empty() {
        vec![Line::from(vec![
            Span::styled("● ", Style::new().fg(theme::OK)),
            Span::styled("Every service is running or off schedule.", soft()),
        ])]
    } else {
        list.iter()
            .map(|a| {
                Line::from(vec![
                    Span::styled("● ", Style::new().fg(theme::status(a.status))),
                    Span::styled(format!("{:<18}", a.name), strong()),
                    Span::styled(a.detail.clone(), soft()),
                ])
            })
            .collect()
    };
    f.render_widget(
        Paragraph::new(lines).block(panel(&format!("Alerts · {}", list.len()), false)),
        alerts,
    );

    let bundles = data::bundles(&v.topology, &v.snapshot);
    let mut lines: Vec<Line> = bundles
        .iter()
        .map(|b| {
            let live = b.live > 0;
            Line::from(vec![
                Span::styled(
                    "● ",
                    Style::new().fg(if live { theme::OK } else { theme::MUTED }),
                ),
                Span::styled(format!("{:<16}", b.name), strong()),
                Span::styled(format!("{:<12}", b.class), dim()),
                if live {
                    Span::styled(
                        format!("live · {}/{}", b.live, b.members),
                        Style::new().fg(theme::OK),
                    )
                } else {
                    Span::styled(
                        b.next
                            .as_ref()
                            .map_or_else(|| "idle".to_owned(), |n| format!("next {n}")),
                        soft(),
                    )
                },
            ])
        })
        .collect();
    for svc in &v.topology.services {
        let yields = svc
            .schedule
            .as_ref()
            .and_then(|s| s.as_array())
            .is_some_and(|a| a.iter().any(|x| x["kind"] == "yield"));
        if yields {
            let detail = v
                .snapshot
                .services
                .get(&svc.id)
                .map(ServiceState::detail)
                .unwrap_or_default();
            let yielding = detail.starts_with("yielding");
            lines.push(Line::from(vec![
                Span::styled("◌ ", dim()),
                Span::styled(format!("{:<16}", svc.name()), strong()),
                Span::styled(format!("{:<12}", "background"), dim()),
                Span::styled(
                    if yielding {
                        "yielding to capture".to_owned()
                    } else {
                        "running".to_owned()
                    },
                    Style::new().fg(if yielding { theme::WARN } else { theme::OK }),
                ),
            ]));
        }
    }
    if lines.is_empty() {
        lines.push(Line::styled(
            "No bundles in this deployment's topology.",
            dim(),
        ));
    }
    f.render_widget(
        Paragraph::new(lines).block(panel("Schedule", false)),
        schedule,
    );

    let accts = data::accounts(&v.snapshot);
    if accts.is_empty() {
        empty(f, accounts, "Accounts", "No account service is running.");
    } else {
        let rows: Vec<Row> = accts
            .iter()
            .map(|a| {
                Row::new(vec![
                    Cell::from(Span::styled(a.label.clone(), strong())),
                    Cell::from(Span::styled(
                        a.environment.clone(),
                        Style::new().fg(if a.environment == "live" {
                            theme::ACCENT
                        } else {
                            theme::MUTED
                        }),
                    )),
                    Cell::from(
                        Line::from(a.equity.map_or_else(|| "—".into(), fmt::money)).right_aligned(),
                    ),
                    Cell::from(
                        Line::from(Span::styled(
                            a.day.map_or_else(String::new, fmt::signed_money),
                            Style::new().fg(theme::signed(a.day.unwrap_or(0.0))),
                        ))
                        .right_aligned(),
                    ),
                    Cell::from(
                        Line::from(Span::styled(
                            a.day_pct.map_or_else(String::new, fmt::pct),
                            Style::new().fg(theme::signed(a.day.unwrap_or(0.0))),
                        ))
                        .right_aligned(),
                    ),
                ])
            })
            .collect();
        f.render_widget(
            Table::new(
                rows,
                [
                    Constraint::Min(10),
                    Constraint::Length(6),
                    Constraint::Length(13),
                    Constraint::Length(12),
                    Constraint::Length(8),
                ],
            )
            .block(panel("Accounts", false)),
            accounts,
        );
    }

    let mut feeds: Vec<Line> = v
        .topology
        .services
        .iter()
        .filter_map(|svc| v.rates.get(&svc.id).map(|r| (svc, r)))
        .map(|(svc, r)| {
            let mut spans = vec![Span::styled(format!("{:<16}", svc.name()), strong())];
            spans.extend(feed_spans(r));
            Line::from(spans)
        })
        .collect();
    if feeds.is_empty() {
        feeds.push(Line::styled("Measuring the feeds (two polls)…", dim()));
    }
    f.render_widget(Paragraph::new(feeds).block(panel("Feeds", false)), prices);

    let (today, _) = fmt::now_ny();
    let rel = data::releases(&v.snapshot, &today);
    if rel.is_empty() {
        empty(
            f,
            releases,
            "Next releases",
            "No economic calendar is being served.",
        );
    } else {
        let lines: Vec<Line> = rel
            .iter()
            .take(usize::from(releases.height.saturating_sub(2)))
            .map(release_line)
            .collect();
        f.render_widget(
            Paragraph::new(lines).block(panel("Next releases", false)),
            releases,
        );
    }

    let (drives, found) = data::drives(&v.snapshot);
    if drives.is_empty() {
        empty(f, storage, "Storage", "The store is not reachable.");
    } else {
        let room: f64 = drives
            .iter()
            .filter(|d| d.present && d.writable)
            .filter_map(|d| d.room)
            .sum();
        let mut lines = vec![Line::from(vec![
            Span::styled(
                fmt::bytes(room),
                Style::new()
                    .fg(theme::level(if room < 100e9 { 95.0 } else { 0.0 }))
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(" room for new data", soft()),
        ])];
        for d in &drives {
            let pct = d.used.unwrap_or(0.0) * 100.0;
            lines.push(Line::from(vec![
                Span::styled(format!("{:<10}", d.name), strong()),
                Span::styled(
                    format!("{pct:>3.0}% used  "),
                    Style::new().fg(theme::level(pct)),
                ),
                Span::styled(
                    format!("{} free", d.free.map_or_else(|| "—".into(), fmt::bytes)),
                    soft(),
                ),
                if d.present {
                    Span::raw("")
                } else {
                    Span::styled("  missing", Style::new().fg(theme::CRIT))
                },
            ]));
        }
        if !found.is_empty() {
            lines.push(Line::styled(
                format!("New drive to approve: {}", found.join(", ")),
                Style::new().fg(theme::ACCENT_2),
            ));
        }
        f.render_widget(
            Paragraph::new(lines).block(panel("Storage", false)),
            storage,
        );
    }
}

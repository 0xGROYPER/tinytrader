//! Data: the store's drives as a pool, and what fills each.

use super::*;

pub fn data_screen(f: &mut Frame, area: Rect, v: &View) {
    let (drives, found) = data::drives(&v.snapshot);
    if drives.is_empty() {
        empty(f, area, "Storage", "The store is not reachable.");
        return;
    }
    let room: f64 = drives
        .iter()
        .filter(|d| d.present && d.writable)
        .filter_map(|d| d.room)
        .sum();
    let [head, body] = Layout::vertical([Constraint::Length(3), Constraint::Min(4)]).areas(area);
    let mut head_line = vec![
        Span::styled(
            fmt::bytes(room),
            Style::new()
                .fg(theme::level(if room < 100e9 { 95.0 } else { 0.0 }))
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(" room for new data across the store's drives", soft()),
    ];
    if !found.is_empty() {
        head_line.push(Span::styled(
            format!(
                "   new drive to approve in the console: {}",
                found.join(", ")
            ),
            Style::new().fg(theme::ACCENT_2),
        ));
    }
    f.render_widget(
        Paragraph::new(Line::from(head_line)).block(panel("Storage pool", false)),
        head,
    );
    let cols = Layout::horizontal(vec![
        Constraint::Ratio(1, drives.len() as u32);
        drives.len()
    ])
    .split(body);
    for (d, col) in drives.iter().zip(cols.iter()) {
        let block = panel(&format!("{} · {}", d.name, d.path), false);
        let inner = block.inner(*col);
        f.render_widget(block, *col);
        if !d.present {
            f.render_widget(
                Paragraph::new(Span::styled(
                    "Not available: the drive may be unplugged.",
                    Style::new().fg(theme::CRIT),
                )),
                inner,
            );
            continue;
        }
        let [gauge, facts, _, list] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Min(1),
        ])
        .areas(inner);
        let pct = d.used.unwrap_or(0.0) * 100.0;
        f.render_widget(
            LineGauge::default()
                .ratio(d.used.unwrap_or(0.0).clamp(0.0, 1.0))
                .label(Span::styled(
                    format!("{pct:>3.0}% ",),
                    Style::new().fg(theme::level(pct)),
                ))
                .filled_style(Style::new().fg(theme::level(pct)))
                .unfilled_style(Style::new().fg(theme::LINE))
                .filled_symbol(symbols::line::THICK.horizontal)
                .unfilled_symbol(symbols::line::THICK.horizontal),
            gauge,
        );
        f.render_widget(
            Line::from(vec![
                Span::styled(
                    format!(
                        "{} free of {}",
                        d.free.map_or_else(|| "—".into(), fmt::bytes),
                        d.total.map_or_else(|| "—".into(), fmt::bytes)
                    ),
                    soft(),
                ),
                Span::styled(
                    format!("  · {} room", d.room.map_or_else(|| "—".into(), fmt::bytes)),
                    dim(),
                ),
                Span::styled(
                    format!("  · {}", d.status),
                    Style::new().fg(theme::level(pct)),
                ),
            ]),
            facts,
        );
        let total = d.total.unwrap_or(1.0).max(1.0);
        let lines: Vec<Line> = d
            .folders
            .iter()
            .take(usize::from(list.height))
            .map(|(name, b)| {
                let width = ((b / total) * 20.0).round() as usize;
                Line::from(vec![
                    Span::styled(
                        format!("{:<20}", "█".repeat(width.min(20))),
                        Style::new().fg(if name.ends_with("(other)") {
                            theme::ACCENT_2
                        } else {
                            theme::ACCENT
                        }),
                    ),
                    Span::styled(format!(" {:>9} ", fmt::bytes(*b)), text()),
                    Span::styled(name.clone(), soft()),
                ])
            })
            .collect();
        f.render_widget(Paragraph::new(lines), list);
    }
}

//! Market: a candlestick chart, accounts with positions, and prices.

use super::*;

pub fn market(f: &mut Frame, area: Rect, v: &View) {
    let [chart, rest] =
        Layout::vertical([Constraint::Percentage(52), Constraint::Min(8)]).areas(area);
    draw_chart(f, chart, v);
    let [left, right] =
        Layout::horizontal([Constraint::Percentage(55), Constraint::Percentage(45)]).areas(rest);
    let accts = data::accounts(&v.snapshot);
    if accts.is_empty() {
        empty(f, left, "Accounts", "No account service is running.");
    } else {
        let pick = v.account.min(accts.len() - 1);
        let [list, detail] = Layout::vertical([
            Constraint::Length(accts.len() as u16 + 3),
            Constraint::Min(4),
        ])
        .areas(left);
        let rows: Vec<Row> = accts
            .iter()
            .enumerate()
            .map(|(i, a)| {
                let style = if i == pick {
                    theme::selected()
                } else {
                    Style::new()
                };
                Row::new(vec![
                    Cell::from(Span::styled(a.label.clone(), strong())),
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
                    Cell::from(
                        Line::from(a.cash.map_or_else(|| "—".into(), fmt::money)).right_aligned(),
                    ),
                    Cell::from(
                        Line::from(a.buying_power.map_or_else(|| "—".into(), fmt::money))
                            .right_aligned(),
                    ),
                ])
                .style(style)
            })
            .collect();
        f.render_widget(
            Table::new(
                rows,
                [
                    Constraint::Min(8),
                    Constraint::Length(13),
                    Constraint::Length(12),
                    Constraint::Length(8),
                    Constraint::Length(13),
                    Constraint::Length(13),
                ],
            )
            .header(header([
                "ACCOUNT",
                "EQUITY",
                "TODAY",
                "",
                "CASH",
                "BUYING POWER",
            ]))
            .block(panel("Accounts", true)),
            list,
        );
        let a = &accts[pick];
        if a.positions.is_empty() {
            empty(
                f,
                detail,
                &format!("{} · positions", a.label),
                "No open positions.",
            );
        } else {
            let rows: Vec<Row> = a
                .positions
                .iter()
                .map(|p| {
                    Row::new(vec![
                        Cell::from(Span::styled(p.symbol.clone(), strong())),
                        Cell::from(Line::from(p.qty.clone()).right_aligned()),
                        Cell::from(
                            Line::from(p.value.map_or_else(|| "—".into(), fmt::money))
                                .right_aligned(),
                        ),
                        Cell::from(
                            Line::from(Span::styled(
                                p.unrealized.map_or_else(String::new, fmt::signed_money),
                                Style::new().fg(theme::signed(p.unrealized.unwrap_or(0.0))),
                            ))
                            .right_aligned(),
                        ),
                        Cell::from(
                            Line::from(Span::styled(
                                p.unrealized_pct.map_or_else(String::new, fmt::pct),
                                Style::new().fg(theme::signed(p.unrealized.unwrap_or(0.0))),
                            ))
                            .right_aligned(),
                        ),
                        Cell::from(
                            Line::from(Span::styled(
                                p.today_pct.map_or_else(String::new, fmt::pct),
                                Style::new().fg(theme::signed(p.today_pct.unwrap_or(0.0))),
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
                        Constraint::Min(12),
                        Constraint::Length(6),
                        Constraint::Length(12),
                        Constraint::Length(12),
                        Constraint::Length(8),
                        Constraint::Length(8),
                    ],
                )
                .header(header([
                    "SYMBOL",
                    "QTY",
                    "VALUE",
                    "UNREALIZED",
                    "",
                    "TODAY",
                ]))
                .block(panel(&format!("{} · positions", a.label), false)),
                detail,
            );
        }
    }
    series(f, right, v, "Prices", false);
}

fn draw_chart(f: &mut Frame, area: Rect, v: &View) {
    let days = if v.chart_days == 0 { 1 } else { v.chart_days };
    match &v.chart {
        None => empty(
            f,
            area,
            "Chart",
            if data::chart_choices(&v.topology).is_empty() {
                "No relay serves bars yet."
            } else {
                "Loading bars…"
            },
        ),
        Some(Err(e)) => empty(f, area, "Chart", &format!("Bars unavailable: {e}")),
        Some(Ok((sym, bars))) => {
            let last = bars.last().map_or_else(String::new, |b| {
                format!(
                    " · {:.2} at {} ET",
                    b[4],
                    fmt::clock(b[0] * 1000.0).get(..5).unwrap_or("")
                )
            });
            let block = panel(
                &format!(
                    "{sym} · one-minute bars · {days} day{}{last}",
                    if days == 1 { "" } else { "s" }
                ),
                true,
            );
            let inner = block.inner(area);
            f.render_widget(block, area);
            if bars.is_empty() {
                f.render_widget(
                    Paragraph::new(Span::styled("No bars in this range yet.", dim())),
                    inner,
                );
            } else {
                f.render_widget(crate::candles::Candles { bars }, inner);
            }
        }
    }
}

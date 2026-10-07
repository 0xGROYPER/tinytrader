//! The screens: Overview, Services, Market, Data and Hosts.

use std::time::Duration;

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::symbols;
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Cell, LineGauge, List, ListItem, ListState, Paragraph, Row, Sparkline, Table, Wrap,
};
use tinytrader_fleet::{ServiceState, Status};

use crate::data;
use crate::fmt;
use crate::theme::{self, dim, panel, soft, strong, text};
use crate::{Sort, View};

/// `41k rec/s · 3.2 MB/s · last 0.2 s · 3 clients · 12 skips/min`, coloured
/// where it matters: skips and a stale last record in amber or red.
fn feed_spans(r: &data::Rates) -> Vec<Span<'static>> {
    let rate = if r.records_s >= 1000.0 {
        format!("{:.1}k rec/s", r.records_s / 1000.0)
    } else {
        format!("{:.0} rec/s", r.records_s)
    };
    let mut out = vec![
        Span::styled(format!("{rate:>11}"), text()),
        Span::styled(format!("  {:>9}/s", fmt::bytes(r.bytes_s)), soft()),
    ];
    if let Some(age) = r.last_age {
        let colour = if age > 30.0 {
            theme::CRIT
        } else if age > 5.0 {
            theme::WARN
        } else {
            theme::FG_2
        };
        out.push(Span::styled(
            format!("  last {age:.1} s"),
            Style::new().fg(colour),
        ));
    }
    out.push(Span::styled(
        format!(
            "  {:.0} client{}",
            r.clients,
            if (r.clients - 1.0).abs() < 0.5 {
                ""
            } else {
                "s"
            }
        ),
        soft(),
    ));
    if r.skips_min > 0.0 {
        out.push(Span::styled(
            format!("  {:.0} skips/min", r.skips_min),
            Style::new()
                .fg(if r.skips_min >= 10.0 {
                    theme::CRIT
                } else {
                    theme::WARN
                })
                .add_modifier(Modifier::BOLD),
        ));
    }
    out
}

/// A table header: the first column left-aligned, the rest right.
fn header<const N: usize>(cols: [&'static str; N]) -> Row<'static> {
    Row::new(cols.into_iter().enumerate().map(|(i, h)| {
        let l = Line::from(Span::styled(h, dim()));
        Cell::from(if i == 0 { l } else { l.right_aligned() })
    }))
}

fn empty(f: &mut Frame, area: Rect, title: &str, why: &str) {
    f.render_widget(
        Paragraph::new(Span::styled(why.to_owned(), dim()))
            .wrap(Wrap { trim: true })
            .block(panel(title, false)),
        area,
    );
}

/// Values scaled to 0–100 for a sparkline, so small moves show.
fn scaled(points: &[f64]) -> Vec<u64> {
    let (lo, hi) = points
        .iter()
        .fold((f64::MAX, f64::MIN), |(l, h), p| (l.min(*p), h.max(*p)));
    let span = (hi - lo).max(f64::EPSILON);
    points
        .iter()
        .map(|p| (1.0 + (p - lo) / span * 99.0) as u64)
        .collect()
}

// ---------- Overview ----------

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

fn release_line(r: &data::Release) -> Line<'static> {
    let label = r.date.get(5..).unwrap_or(&r.date).to_owned();
    Line::from(vec![
        Span::styled(format!("{label} {} ", r.time), dim()),
        Span::styled(
            if r.high { "● " } else { "○ " },
            Style::new().fg(if r.high { theme::ACCENT } else { theme::MUTED }),
        ),
        Span::styled(r.title.clone(), if r.high { strong() } else { soft() }),
        Span::styled(format!("  {}", r.source), dim()),
    ])
}

/// Each price series: its key, last value, change and a sparkline.
fn series(f: &mut Frame, area: Rect, v: &View, title: &str, focused: bool) {
    let list = data::prices(&v.topology, &v.snapshot);
    if list.is_empty() {
        empty(
            f,
            area,
            title,
            "No prices yet: they appear while the market data relays run.",
        );
        return;
    }
    let block = panel(title, focused);
    let inner = block.inner(area);
    f.render_widget(block, area);
    let rows = Layout::vertical(vec![
        Constraint::Length(1);
        list.len().min(usize::from(inner.height))
    ])
    .split(inner);
    for (s, row) in list.iter().zip(rows.iter()) {
        let [name, last, chg, spark] = Layout::horizontal([
            Constraint::Length(22),
            Constraint::Length(10),
            Constraint::Length(9),
            Constraint::Min(4),
        ])
        .areas(*row);
        let (first, end) = (s.points[0], *s.points.last().unwrap_or(&0.0));
        let change = if first == 0.0 {
            0.0
        } else {
            (end - first) / first
        };
        f.render_widget(
            Line::from(vec![
                Span::styled(s.key.clone(), strong()),
                Span::styled(format!(" {}", s.via), dim()),
            ]),
            name,
        );
        f.render_widget(
            Line::from(Span::styled(format!("{end:.2}"), text())).right_aligned(),
            last,
        );
        f.render_widget(
            Line::from(Span::styled(
                fmt::pct(change),
                Style::new().fg(theme::signed(change)),
            ))
            .right_aligned(),
            chg,
        );
        let pts = scaled(&s.points);
        let tail = &pts[pts
            .len()
            .saturating_sub(usize::from(spark.width.saturating_sub(1)))..];
        f.render_widget(
            Sparkline::default()
                .data(tail)
                .style(Style::new().fg(theme::signed(change))),
            Rect {
                x: spark.x + 1,
                width: spark.width.saturating_sub(1),
                ..spark
            },
        );
    }
}

// ---------- Services ----------

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

// ---------- Market ----------

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

// ---------- Data ----------

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

// ---------- Hosts ----------

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

// ---------- Book ----------

pub fn book(f: &mut Frame, area: Rect, v: &View) {
    let Some(feeds) = &v.feeds else {
        empty(
            f,
            area,
            "Book",
            "No options relay in this deployment's topology.",
        );
        return;
    };
    let ob = feeds
        .options
        .book
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let stock = feeds.stock.as_ref().map(|s| {
        s.book
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    });
    let [chain_area, side] =
        Layout::horizontal([Constraint::Min(70), Constraint::Length(46)]).areas(area);

    // Expiries still to come (OPRA stamps a day's expiry at its midnight UTC).
    let today_utc = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
        / 86_400
        * 86_400
        * 1_000_000_000;
    let mut expiries: Vec<u64> = ob
        .contracts
        .values()
        .map(|c| c.expiry)
        .filter(|e| *e >= today_utc)
        .collect();
    expiries.sort_unstable();
    expiries.dedup();
    if expiries.is_empty() {
        let why = if ob.records == 0 {
            format!("{} · {}", feeds.root, fmt::event(&ob.status).0)
        } else {
            format!(
                "{}: waiting for option definitions ({})",
                feeds.root,
                fmt::event(&ob.status).0
            )
        };
        empty(f, chain_area, &format!("{} options", feeds.root), &why);
    } else {
        let expiry = expiries[v.book_exp % expiries.len()];
        let mut by: std::collections::BTreeMap<i64, (Option<u32>, Option<u32>)> =
            std::collections::BTreeMap::new();
        for (id, c) in ob.contracts.iter().filter(|(_, c)| c.expiry == expiry) {
            let e = by.entry((c.strike * 1000.0).round() as i64).or_default();
            if c.call {
                e.0 = Some(*id)
            } else {
                e.1 = Some(*id)
            }
        }
        let strikes: Vec<(f64, Option<u32>, Option<u32>)> = by
            .into_iter()
            .map(|(k, (c, p))| (k as f64 / 1000.0, c, p))
            .collect();
        let mid = |id: Option<u32>| {
            id.and_then(|i| ob.quotes.get(&i))
                .and_then(crate::book::Quote::mid)
        };
        // Spot: the underlying's own quote, else where calls and puts cross.
        let root = feeds.root.split('/').next().unwrap_or("");
        let spot = stock
            .as_ref()
            .and_then(|s| {
                s.names
                    .iter()
                    .find(|(_, n)| n.as_str() == root)
                    .and_then(|(id, _)| s.quotes.get(id))
                    .and_then(crate::book::Quote::mid)
            })
            .or_else(|| {
                strikes
                    .iter()
                    .filter_map(|(k, c, p)| Some((*k, (mid(*c)? - mid(*p)?).abs())))
                    .min_by(|a, b| a.1.total_cmp(&b.1))
                    .map(|(k, _)| k)
            });
        let center = spot.map_or(strikes.len() / 2, |s| {
            strikes
                .iter()
                .enumerate()
                .min_by(|a, b| (a.1.0 - s).abs().total_cmp(&(b.1.0 - s).abs()))
                .map_or(0, |(i, _)| i)
        });
        let rows_h = usize::from(chain_area.height.saturating_sub(4));
        let start = center
            .saturating_sub(rows_h / 2)
            .min(strikes.len().saturating_sub(rows_h));
        let fresh = |s: &crate::book::Side| s.changed.elapsed() < Duration::from_millis(800);
        let cell = |s: Option<crate::book::Side>, size: bool| -> Cell<'static> {
            match s {
                None => Cell::from(Line::from(Span::styled("—", dim())).right_aligned()),
                Some(x) => {
                    let t = if size {
                        x.sz.to_string()
                    } else {
                        format!("{:.2}", x.px)
                    };
                    let style = if fresh(&x) {
                        Style::new()
                            .fg(if x.up { theme::OK } else { theme::CRIT })
                            .add_modifier(Modifier::BOLD)
                    } else if size {
                        dim()
                    } else {
                        text()
                    };
                    Cell::from(Line::from(Span::styled(t, style)).right_aligned())
                }
            }
        };
        let rows: Vec<Row> = strikes[start..(start + rows_h).min(strikes.len())]
            .iter()
            .map(|(k, c, p)| {
                let (cq, pq) = (
                    c.and_then(|i| ob.quotes.get(&i))
                        .copied()
                        .unwrap_or_default(),
                    p.and_then(|i| ob.quotes.get(&i))
                        .copied()
                        .unwrap_or_default(),
                );
                let atm = spot.is_some_and(|s| {
                    (k - s).abs() < 1e-9
                        || strikes.get(center).is_some_and(|x| (x.0 - k).abs() < 1e-9)
                });
                let itm_call = spot.is_some_and(|s| *k < s);
                let shade = Style::new().bg(theme::SELECT);
                let call_style = if itm_call { shade } else { Style::new() };
                let put_style = if spot.is_some_and(|s| *k > s) {
                    shade
                } else {
                    Style::new()
                };
                let mut cells = vec![
                    cell(cq.bid, true).style(call_style),
                    cell(cq.bid, false).style(call_style),
                    cell(cq.ask, false).style(call_style),
                    cell(cq.ask, true).style(call_style),
                ];
                cells.push(Cell::from(
                    Line::from(Span::styled(
                        crate::book::strike_text(*k),
                        if atm {
                            Style::new().fg(theme::ACCENT).add_modifier(Modifier::BOLD)
                        } else {
                            strong()
                        },
                    ))
                    .centered(),
                ));
                cells.extend([
                    cell(pq.bid, true).style(put_style),
                    cell(pq.bid, false).style(put_style),
                    cell(pq.ask, false).style(put_style),
                    cell(pq.ask, true).style(put_style),
                ]);
                Row::new(cells)
            })
            .collect();
        let (date, _, _) =
            tinytrader_schedule::ny_date_time((expiry / 1_000_000_000) as i64 + 43_200);
        let (today, _) = fmt::now_ny();
        let title = format!(
            "{} options · expires {date}{} · {}/{} · {} contracts{}",
            feeds.root,
            if date == today { " (today)" } else { "" },
            v.book_exp % expiries.len() + 1,
            expiries.len(),
            ob.contracts.len(),
            spot.map_or_else(String::new, |s| format!(" · spot {s:.2}"))
        );
        let w = [
            Constraint::Length(6),
            Constraint::Length(8),
            Constraint::Length(8),
            Constraint::Length(6),
            Constraint::Length(9),
            Constraint::Length(8),
            Constraint::Length(8),
            Constraint::Length(6),
            Constraint::Length(6),
        ];
        f.render_widget(
            Table::new(rows, w)
                .header(Row::new(
                    [
                        "size", "bid", "ask", "size", "STRIKE", "bid", "ask", "size", "size",
                    ]
                    .into_iter()
                    .enumerate()
                    .map(|(i, h)| {
                        Cell::from(if i == 4 {
                            Line::from(Span::styled(h, dim())).centered()
                        } else {
                            Line::from(Span::styled(h, dim())).right_aligned()
                        })
                    }),
                ))
                .column_spacing(1)
                .block(panel(&title, true).title_bottom(Line::from(vec![
                    Span::styled(" CALLS ", Style::new().fg(theme::OK)),
                    Span::styled("·", dim()),
                    Span::styled(" PUTS ", Style::new().fg(theme::CRIT)),
                    Span::styled(&*format!(" {} ", fmt::event(&ob.status).0), dim()),
                ]))),
            chain_area,
        );
    }

    // The underlying's quote, then the tape.
    let [quote, tape] = Layout::vertical([Constraint::Length(5), Constraint::Min(4)]).areas(side);
    let root = feeds.root.split('/').next().unwrap_or("").to_owned();
    let lines = match stock.as_ref().and_then(|s| {
        s.names
            .iter()
            .find(|(_, n)| **n == root)
            .and_then(|(id, _)| s.quotes.get(id))
            .copied()
    }) {
        Some(q) => {
            let px = |s: Option<crate::book::Side>| {
                s.map_or_else(|| "—".into(), |x| format!("{:.2}", x.px))
            };
            let sz =
                |s: Option<crate::book::Side>| s.map_or_else(String::new, |x| x.sz.to_string());
            vec![
                Line::from(vec![
                    Span::styled(
                        format!("{:>9}", px(q.bid)),
                        Style::new().fg(theme::OK).add_modifier(Modifier::BOLD),
                    ),
                    Span::styled("  ×  ", dim()),
                    Span::styled(
                        px(q.ask),
                        Style::new().fg(theme::CRIT).add_modifier(Modifier::BOLD),
                    ),
                ]),
                Line::from(vec![
                    Span::styled(format!("{:>9}", sz(q.bid)), dim()),
                    Span::styled("     ", dim()),
                    Span::styled(sz(q.ask), dim()),
                ]),
                Line::from(Span::styled(
                    q.bid
                        .zip(q.ask)
                        .map_or_else(String::new, |(b, a)| format!("spread {:.2}", a.px - b.px)),
                    soft(),
                )),
            ]
        }
        None => vec![Line::styled(
            if feeds.stock.is_some() {
                "Waiting for the IEX quote…"
            } else {
                "No live quote for this underlying (IEX doesn't carry it); spot is estimated from the chain."
            },
            dim(),
        )],
    };
    f.render_widget(
        Paragraph::new(lines)
            .wrap(Wrap { trim: true })
            .block(panel(&format!("{root} · IEX"), false)),
        quote,
    );
    let mut trades: Vec<crate::book::Trade> = ob.tape.iter().cloned().collect();
    if let Some(s) = &stock {
        trades.extend(s.tape.iter().cloned());
    }
    trades.sort_by_key(|t| std::cmp::Reverse(t.at_ns));
    let items: Vec<ListItem> = trades
        .iter()
        .take(usize::from(tape.height))
        .map(|t| {
            ListItem::new(Line::from(vec![
                Span::styled(fmt::clock(t.at_ns as f64 / 1e6), dim()),
                Span::styled(format!("  {:<8}", t.name), strong()),
                Span::styled(format!("{:>9.2}", t.px), text()),
                Span::styled(format!(" × {}", t.size), soft()),
            ]))
        })
        .collect();
    let items = if items.is_empty() {
        vec![ListItem::new(Span::styled("No trades yet.", dim()))]
    } else {
        items
    };
    f.render_widget(List::new(items).block(panel("Tape", false)), tape);
}

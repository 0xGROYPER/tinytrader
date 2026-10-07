//! The screens: Overview, Services, Market, Book, Data and Hosts, one module
//! each; the helpers they share are here.

mod book;
mod hosts;
mod market;
mod overview;
mod services;
mod storage;

pub use book::book;
pub use hosts::hosts;
pub use market::market;
pub use overview::overview;
pub use services::services;
pub use storage::data_screen;

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
pub(crate) fn feed_spans(r: &data::Rates) -> Vec<Span<'static>> {
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
pub(crate) fn header<const N: usize>(cols: [&'static str; N]) -> Row<'static> {
    Row::new(cols.into_iter().enumerate().map(|(i, h)| {
        let l = Line::from(Span::styled(h, dim()));
        Cell::from(if i == 0 { l } else { l.right_aligned() })
    }))
}

pub(crate) fn empty(f: &mut Frame, area: Rect, title: &str, why: &str) {
    f.render_widget(
        Paragraph::new(Span::styled(why.to_owned(), dim()))
            .wrap(Wrap { trim: true })
            .block(panel(title, false)),
        area,
    );
}

/// Values scaled to 0–100 for a sparkline, so small moves show.
pub(crate) fn scaled(points: &[f64]) -> Vec<u64> {
    let (lo, hi) = points
        .iter()
        .fold((f64::MAX, f64::MIN), |(l, h), p| (l.min(*p), h.max(*p)));
    let span = (hi - lo).max(f64::EPSILON);
    points
        .iter()
        .map(|p| (1.0 + (p - lo) / span * 99.0) as u64)
        .collect()
}

pub(crate) fn release_line(r: &data::Release) -> Line<'static> {
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
pub(crate) fn series(f: &mut Frame, area: Rect, v: &View, title: &str, focused: bool) {
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

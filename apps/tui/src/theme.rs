//! The look: one palette (after the console's Dusk theme, on the terminal's
//! own background), rounded panels, and the few styles everything uses.

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Span;
use ratatui::widgets::{Block, BorderType, Padding};
use tinytrader_fleet::Status;

pub const ACCENT: Color = Color::Rgb(181, 126, 232);
pub const ACCENT_2: Color = Color::Rgb(122, 162, 247);
pub const FG: Color = Color::Rgb(224, 222, 244);
pub const FG_2: Color = Color::Rgb(184, 180, 214);
pub const MUTED: Color = Color::Rgb(110, 106, 140);
pub const LINE: Color = Color::Rgb(68, 64, 96);
pub const OK: Color = Color::Rgb(152, 210, 128);
pub const WARN: Color = Color::Rgb(232, 178, 98);
pub const CRIT: Color = Color::Rgb(240, 112, 132);
pub const SELECT: Color = Color::Rgb(52, 44, 78);

pub fn text() -> Style {
    Style::new().fg(FG)
}
pub fn dim() -> Style {
    Style::new().fg(MUTED)
}
pub fn soft() -> Style {
    Style::new().fg(FG_2)
}
pub fn strong() -> Style {
    Style::new().fg(FG).add_modifier(Modifier::BOLD)
}
pub fn selected() -> Style {
    Style::new().bg(SELECT).add_modifier(Modifier::BOLD)
}

/// A panel: rounded border in the line colour (the accent when it has the
/// keyboard), its title in bold.
pub fn panel(title: &str, focused: bool) -> Block<'static> {
    Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(if focused { ACCENT } else { LINE }))
        .title(Span::styled(
            format!(" {title} "),
            Style::new()
                .fg(if focused { ACCENT } else { FG_2 })
                .add_modifier(Modifier::BOLD),
        ))
        .padding(Padding::horizontal(1))
}

pub fn status(s: Status) -> Color {
    match s {
        Status::Ok => OK,
        Status::Warn => WARN,
        Status::Crit => CRIT,
        Status::Idle => MUTED,
    }
}

/// Green below the store's warning threshold, then amber, then red.
pub fn level(pct: f64) -> Color {
    if pct >= 90.0 {
        CRIT
    } else if pct >= 80.0 {
        WARN
    } else {
        OK
    }
}

/// Up green, down red, unchanged plain.
pub fn signed(v: f64) -> Color {
    if v > 0.0 {
        OK
    } else if v < 0.0 {
        CRIT
    } else {
        FG_2
    }
}

/// `key label` pairs for the footer: the key in the accent, the label dim.
pub fn hints(pairs: &[(&str, &str)]) -> Vec<Span<'static>> {
    let mut out = Vec::new();
    for (i, (k, l)) in pairs.iter().enumerate() {
        if i > 0 {
            out.push(Span::styled("  ", dim()));
        }
        out.push(Span::styled(
            (*k).to_owned(),
            Style::new().fg(ACCENT_2).add_modifier(Modifier::BOLD),
        ));
        out.push(Span::styled(format!(" {l}"), dim()));
    }
    out
}

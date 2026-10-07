//! Candlesticks in the terminal: one column per one-minute bar, the wick as
//! a line from high to low and the body from open to close (green up, red
//! down), volume as small bars beneath, a price scale on the right and New
//! York times along the bottom.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::widgets::Widget;
use tinytrader_schedule::ny_date_time;

use crate::theme;

/// One bar: start (Unix seconds), open, high, low, close, volume.
pub type Bar = [f64; 6];

pub struct Candles<'a> {
    pub bars: &'a [Bar],
}

const SCALE: u16 = 10;
const VOLUME: u16 = 3;

impl Widget for Candles<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.width <= SCALE + 4 || area.height < VOLUME + 5 || self.bars.is_empty() {
            return;
        }
        let plot_w = area.width - SCALE;
        let price_h = area.height - VOLUME - 1;
        let bars = &self.bars[self.bars.len().saturating_sub(usize::from(plot_w))..];
        let lo = bars.iter().map(|b| b[3]).fold(f64::MAX, f64::min);
        let hi = bars.iter().map(|b| b[2]).fold(f64::MIN, f64::max);
        let span = (hi - lo).max(1e-9);
        let row = |p: f64| area.y + ((hi - p) / span * f64::from(price_h - 1)).round() as u16;

        // Faint guides at the top, middle and bottom prices.
        for (i, p) in [hi, (hi + lo) / 2.0, lo].into_iter().enumerate() {
            let y = row(p);
            for x in area.x..area.x + plot_w {
                buf[(x, y)]
                    .set_symbol("┈")
                    .set_style(Style::new().fg(theme::LINE));
            }
            let label = format!(" {p:.2}");
            buf.set_string(
                area.x + plot_w,
                y,
                label,
                Style::new().fg(if i == 1 { theme::MUTED } else { theme::FG_2 }),
            );
        }

        let max_v = bars.iter().map(|b| b[5]).fold(0.0, f64::max).max(1.0);
        let levels = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
        for (i, b) in bars.iter().enumerate() {
            let x = area.x + i as u16;
            let [_, o, h, l, c, v] = *b;
            let colour = if c >= o { theme::OK } else { theme::CRIT };
            for y in row(h)..=row(l) {
                buf[(x, y)]
                    .set_symbol("│")
                    .set_style(Style::new().fg(colour));
            }
            let (top, bottom) = (row(o.max(c)), row(o.min(c)));
            for y in top..=bottom {
                buf[(x, y)]
                    .set_symbol(if top == bottom { "━" } else { "┃" })
                    .set_style(Style::new().fg(colour));
            }
            // Volume, in eighths of a cell over its rows.
            let eighths = (v / max_v * f64::from(VOLUME * 8)).round() as u16;
            let base = area.y + price_h + VOLUME - 1;
            for k in 0..VOLUME {
                let fill = eighths.saturating_sub(k * 8).min(8);
                if fill > 0 {
                    buf[(x, base - k)]
                        .set_symbol(&levels[usize::from(fill - 1)].to_string())
                        .set_style(
                            Style::new()
                                .fg(if c >= o { theme::OK } else { theme::CRIT })
                                .add_modifier(ratatui::style::Modifier::DIM),
                        );
                }
            }
        }

        // The last price, marked on the scale.
        let last = bars[bars.len() - 1];
        let colour = if last[4] >= last[1] {
            theme::OK
        } else {
            theme::CRIT
        };
        buf.set_string(
            area.x + plot_w,
            row(last[4]),
            format!("◀{:.2}", last[4]),
            Style::new()
                .fg(colour)
                .add_modifier(ratatui::style::Modifier::BOLD),
        );

        // Times along the bottom, about every 15 columns.
        let axis = area.y + area.height - 1;
        let mut next_free = area.x;
        for (i, b) in bars.iter().enumerate() {
            let x = area.x + i as u16;
            if x < next_free || i % 15 != 0 {
                continue;
            }
            let (_, hm, _) = ny_date_time(b[0] as i64);
            buf.set_string(x, axis, &hm, Style::new().fg(theme::MUTED));
            next_free = x + hm.len() as u16 + 2;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn draws_bodies_wicks_volume_and_the_last_price() {
        let bars: Vec<Bar> = (0..30)
            .map(|i| {
                let o = 100.0 + f64::from(i % 7);
                [
                    1_791_400_000.0 + f64::from(i) * 60.0,
                    o,
                    o + 2.0,
                    o - 2.0,
                    if i % 2 == 0 { o + 1.0 } else { o - 1.0 },
                    1000.0 + f64::from(i) * 10.0,
                ]
            })
            .collect();
        let area = Rect::new(0, 0, 60, 20);
        let mut buf = Buffer::empty(area);
        Candles { bars: &bars }.render(area, &mut buf);
        let text: String = (0..area.height)
            .map(|y| {
                (0..area.width)
                    .map(|x| buf[(x, y)].symbol().to_owned())
                    .collect::<String>()
                    + "\n"
            })
            .collect();
        assert!(text.contains('┃') && text.contains('│'), "{text}");
        assert!(text.contains('◀'), "the last price is marked:\n{text}");
        assert!(
            ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█']
                .iter()
                .any(|c| text.contains(*c)),
            "volume:\n{text}"
        );
    }
}

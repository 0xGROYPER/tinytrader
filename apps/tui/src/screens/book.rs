//! Book: a live options chain, the underlying's quote, and a tape.

use super::*;

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

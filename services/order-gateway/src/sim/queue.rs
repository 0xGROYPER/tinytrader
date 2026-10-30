//! The queue model for top-of-book data (profile `queue`): a resting limit order
//! fills only on trade prints (`tcbbo`), only once the displayed size ahead of it
//! has traded, and only as much as the prints leave.
//!
//! - **Prints.** A print counts only inside its own pre-trade quote
//!   (bid ≤ price ≤ ask): outside it are mostly complex-order legs, sweeps,
//!   auctions and late reports. Trade conditions aren't read, so a complex-order
//!   leg printing inside the quote still counts. Prints and displayed sizes are
//!   consolidated: a print on any exchange uses up the queue, and the size ahead is
//!   the size shown across all of them.
//! - **Joining.** When an order first rests (and every leg has a bid and an ask),
//!   each leg joins a queue: a single's at its limit; a combo leg's, under
//!   `combo = "touch"` (the default), at its passive touch (the bid for a buy, the
//!   ask for a sell), and under `"implied"` at its implied price. The size ahead is
//!   `ahead` (a fraction, 1 by default) of the displayed size on its side, if the
//!   price is at or behind the touch (behind it, the touch's size, which understates
//!   the queue), and nothing inside the spread. It keeps its place if the book
//!   moves.
//! - **Clearing.** A print through a leg's queue price (below a buy's, above a
//!   sell's) makes its whole size available to the leg. A print at the price (within
//!   half a cent) first uses up the size ahead, and what's left is available, unless
//!   `fill_on_print = "through"`. Several resting orders on one contract share a
//!   print, earliest (`t_act`, then arrival) first.
//! - **Filling.** A single fills at its limit, as much as is available. A combo
//!   fills when every leg has had size available within `combo_window_s` (60 s by
//!   default, a rolling window), as many units as every leg allows, each leg at its
//!   implied price: the net limit split over the legs in proportion to their mids,
//!   rounded to the cent against the order, the last leg then moved back so the net
//!   is never worse than the limit. This is legging the spread, not trading in a complex book. What isn't
//!   filled keeps its place, at the front.
//! - Each leg's queue is in the order's `broker_raw.order.queue`.
//! - Not modelled: trade conditions, market impact, orders joining behind.
use super::{CENT, SimBroker};
use std::fmt::Write as _;

/// Half a cent, nanodollars: a print within it of a price is at that price.
const HALF_CENT: i64 = 5_000_000;

/// The profile's queue settings.
#[derive(Clone, Copy, Debug)]
pub(crate) struct QueueModel {
    /// The fraction of the displayed size ahead of a joining order, in thousandths.
    pub(crate) ahead_permille: u64,
    /// Only prints strictly through a price count.
    pub(crate) through_only: bool,
    /// A combo's legs queue at their implied prices, not their passive touches.
    pub(crate) combo_implied: bool,
    /// How close in time a combo's legs must clear, ns.
    pub(crate) window: u64,
}

/// One leg's place in its queue.
#[derive(Clone, Debug)]
pub(crate) struct LegQueue {
    /// Its fill price, nanodollars.
    pub(crate) price: i64,
    /// The price whose prints clear it, nanodollars.
    pub(crate) at: i64,
    /// Contracts still ahead of it.
    pub(crate) ahead: u64,
    /// Contracts prints have left for it, not yet filled.
    pub(crate) avail: u64,
    /// When size last became available, ns.
    pub(crate) cleared: Option<u64>,
}

impl SimBroker {
    /// Joins order `i`'s legs to their queues, if it has none yet; the queue
    /// model must be on and the order resting. A leg without a bid and an ask
    /// leaves it unqueued until one has both.
    pub(super) fn join_queue(&mut self, i: usize) {
        let Some(model) = self.s.queue else { return };
        if self.orders[i].queue.is_some() {
            return;
        }
        let Some(m) = &self.market else { return };
        let o = &self.orders[i];
        let mut quotes = Vec::new();
        for l in &o.legs {
            let Some(q) = m.ids.get(&l.symbol).and_then(|id| m.book.get(id)) else {
                return;
            };
            let (Some(b), Some(a)) = (q.bid, q.ask) else {
                return;
            };
            quotes.push((b, a, q.bid_sz, q.ask_sz));
        }
        let limit = i64::try_from(i128::from(o.limit_cents) * i128::from(CENT)).unwrap_or(0);
        let prices: Vec<i64> = if o.combo {
            let Some(p) = implied(o, &quotes, limit) else {
                return;
            };
            p
        } else {
            vec![limit.abs()]
        };
        let legs = o
            .legs
            .iter()
            .zip(prices)
            .zip(&quotes)
            .map(|((l, price), (b, a, bsz, asz))| {
                let (touch, size) = if l.buy { (*b, *bsz) } else { (*a, *asz) };
                let at = if o.combo && !model.combo_implied {
                    touch
                } else {
                    price
                };
                let at_touch = (at - touch).abs() <= HALF_CENT;
                let behind = if l.buy { at < touch } else { at > touch };
                let shown = if at_touch || behind { size } else { 0 };
                LegQueue {
                    price,
                    at,
                    ahead: (shown * model.ahead_permille).div_ceil(1000),
                    avail: 0,
                    cleared: None,
                }
            })
            .collect();
        self.orders[i].queue = Some(legs);
    }

    /// A trade print on instrument `iid`, inside its quote: the queues of resting
    /// orders on it, then the fills it allows.
    pub(super) fn on_print(&mut self, iid: u32, price: i64, size: u64) {
        let Some(model) = self.s.queue else { return };
        let Some(m) = &self.market else { return };
        let Some(symbol) = m.names.get(&iid).cloned() else {
            return;
        };
        let day = crate::safety::et_day(self.now);
        let mut order: Vec<usize> = (0..self.orders.len())
            .filter(|&i| {
                self.open(i)
                    && self.orders[i].active
                    && self.orders[i].queue.is_some()
                    && self.orders[i].legs.iter().any(|l| l.symbol == symbol)
                    && !self.orders[i]
                        .legs
                        .iter()
                        .any(|l| self.expired(&l.symbol, day))
            })
            .collect();
        order.sort_by_key(|&i| (self.orders[i].t_act, self.orders[i].seq));
        let now = self.now;
        let mut left = size;
        for &i in &order {
            let o = &mut self.orders[i];
            let remaining = o.qty - o.filled;
            let Some(queue) = &mut o.queue else { continue };
            for (l, q) in o.legs.iter().zip(queue.iter_mut()) {
                if l.symbol != symbol {
                    continue;
                }
                let need = (remaining * l.ratio).saturating_sub(q.avail);
                let through = if l.buy {
                    price < q.at - HALF_CENT
                } else {
                    price > q.at + HALF_CENT
                };
                let at = (price - q.at).abs() <= HALF_CENT;
                let free = if through {
                    left
                } else if at && !model.through_only {
                    let used = q.ahead.min(size);
                    q.ahead -= used;
                    (size - used).min(left)
                } else {
                    0
                };
                let grant = free.min(need);
                if grant > 0 {
                    q.avail += grant;
                    q.cleared = Some(now);
                    left -= grant;
                }
            }
        }
        for i in order {
            self.fill_queued(i, model.window);
        }
    }

    /// Fills what order `i`'s queues allow: every leg's available size, all within
    /// the window.
    fn fill_queued(&mut self, i: usize, window: u64) {
        let now = self.now;
        let o = &mut self.orders[i];
        let Some(queue) = &mut o.queue else { return };
        let mut units = o.qty - o.filled;
        for (l, q) in o.legs.iter().zip(queue.iter_mut()) {
            // Size from a clearing that's left the window is gone.
            if q.cleared.is_none_or(|t| now.saturating_sub(t) > window) {
                q.avail = 0;
            }
            units = units.min(q.avail / l.ratio);
        }
        if units == 0 {
            return;
        }
        for (l, q) in o.legs.iter().zip(queue.iter_mut()) {
            q.avail -= units * l.ratio;
        }
        let prices: Vec<i64> = queue.iter().map(|q| q.price).collect();
        self.execute(i, units, &prices);
    }

    /// Each leg's queue, for the order's `broker_raw`.
    pub(super) fn queue_raw(queue: &[LegQueue], legs: &[super::SimLeg]) -> String {
        let mut out = String::from("[");
        for (k, (q, l)) in queue.iter().zip(legs).enumerate() {
            if k > 0 {
                out.push(',');
            }
            let _ = write!(
                out,
                "{{\"symbol\":{:?},\"price\":\"{}\",\"queue_price\":\"{}\",\"ahead\":{},\"available\":{},\"cleared_ns\":{}}}",
                l.symbol,
                super::fmt_nanos(i128::from(q.price)),
                super::fmt_nanos(i128::from(q.at)),
                q.ahead,
                q.avail,
                q.cleared.map_or("null".into(), |t| t.to_string())
            );
        }
        out.push(']');
        out
    }
}

/// A combo's leg prices: the net limit split over the legs in proportion to their
/// mids, each rounded to the cent against the order (a buy up, a sell down); if
/// that nets worse than the limit, the last leg moves back so the net meets it.
/// `None` if a leg would be priced at nothing.
fn implied(o: &super::SimOrder, quotes: &[(i64, i64, u64, u64)], limit: i64) -> Option<Vec<i64>> {
    let mids: Vec<i128> = quotes
        .iter()
        .map(|(b, a, _, _)| i128::from(*b + *a) / 2)
        .collect();
    let (mut net_mid, mut weight) = (0_i128, 0_i128);
    for (l, mid) in o.legs.iter().zip(&mids) {
        let s = if l.buy { 1 } else { -1 };
        net_mid += s * i128::from(l.ratio) * mid;
        weight += i128::from(l.ratio) * mid;
    }
    if weight == 0 {
        return None;
    }
    let tick = i128::from(CENT);
    let ceil = |p: i128| (p + tick - 1).div_euclid(tick) * tick;
    let mut net = 0_i128;
    let mut out: Vec<i128> = Vec::new();
    for (l, mid) in o.legs.iter().zip(&mids) {
        let s: i128 = if l.buy { 1 } else { -1 };
        let p = mid + s * (i128::from(limit) - net_mid) * mid / weight;
        let p = if l.buy {
            ceil(p)
        } else {
            p.div_euclid(tick) * tick
        };
        net += s * i128::from(l.ratio) * p;
        out.push(p);
    }
    // Worse than the limit by `over`: the last leg gives it back, to the tick.
    let over = net - i128::from(limit);
    if over > 0 {
        let (l, p) = (o.legs.last()?, out.last_mut()?);
        let r = i128::from(l.ratio);
        let step = ceil((over + r - 1) / r);
        *p += if l.buy { -step } else { step };
    }
    if out.iter().any(|p| *p <= 0) {
        return None;
    }
    out.into_iter().map(|p| i64::try_from(p).ok()).collect()
}

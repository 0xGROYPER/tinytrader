//! The queue model for top-of-book data (profile `queue`): a resting limit order
//! fills only on trade prints (`tcbbo`), and only once the displayed size ahead of
//! it at its price has traded.
//!
//! - **Marketable orders** fill at the touch as always; the model applies to an
//!   order resting behind it.
//! - **Joining.** When an order first rests, each leg joins a queue at its price:
//!   a single's limit; a combo leg's implied price, the net limit split over the legs
//!   in proportion to their mids. The size ahead is `ahead` (a fraction, 1 by
//!   default) of the displayed size on its side at that price when it joins: the
//!   bid for a buy at the bid, nothing for a buy inside the spread.
//! - **Prints.** A print through a leg's price (below a buy's, above a sell's)
//!   clears the leg. A print at its price (within half a cent) clears it once its
//!   size has used up the queue ahead, unless `fill_on_print = "through"`, where
//!   only prints through count.
//! - **Filling.** A single fills its remaining quantity at its limit when its leg
//!   clears. A combo fills, every leg at its implied price, when all its legs have
//!   cleared within the same minute; a leg cleared in an earlier minute must clear
//!   again. Each leg's queue state is in the order's `broker_raw.queue`.
//! - Market impact and partial fills from the prints' sizes aren't modelled.
use super::{CENT, SimBroker};
use std::fmt::Write as _;

/// Half a cent, nanodollars: a print within it of a leg's price is at that price.
const HALF_CENT: i64 = 5_000_000;
const MINUTE: u64 = 60_000_000_000;

/// The profile's queue settings.
#[derive(Clone, Copy, Debug)]
pub(crate) struct QueueModel {
    /// The fraction of the displayed size ahead of a joining order, in thousandths.
    pub(crate) ahead_permille: u64,
    /// Only prints strictly through a price fill it.
    pub(crate) through_only: bool,
}

/// One leg's place in its queue.
#[derive(Clone, Debug)]
pub(crate) struct LegQueue {
    /// Its price, nanodollars.
    pub(crate) price: i64,
    /// Contracts still ahead of it.
    pub(crate) ahead: u64,
    /// The minute (market time / 60 s) it last cleared in.
    pub(crate) cleared: Option<u64>,
}

impl SimBroker {
    /// Joins order `i`'s legs to their queues, if it has none yet; the queue
    /// model must be on and the order resting.
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
            // net = Σ s·r·p with p = mid + s·k·mid: solve for k.
            let mids: Vec<i64> = quotes.iter().map(|(b, a, _, _)| (b + a) / 2).collect();
            let mut net_mid = 0_i128;
            let mut weight = 0_i128;
            for (l, mid) in o.legs.iter().zip(&mids) {
                let s = if l.buy { 1 } else { -1 };
                net_mid += s * i128::from(l.ratio) * i128::from(*mid);
                weight += i128::from(l.ratio) * i128::from(*mid);
            }
            if weight == 0 {
                return;
            }
            o.legs
                .iter()
                .zip(&mids)
                .map(|(l, mid)| {
                    let s: i128 = if l.buy { 1 } else { -1 };
                    let adj = (i128::from(limit) - net_mid) * i128::from(*mid) / weight;
                    i64::try_from(i128::from(*mid) + s * adj)
                        .unwrap_or(*mid)
                        .max(0)
                })
                .collect()
        } else {
            vec![if o.legs[0].buy { limit } else { limit.abs() }]
        };
        let legs = o
            .legs
            .iter()
            .zip(prices)
            .zip(&quotes)
            .map(|((l, price), (b, a, bsz, asz))| {
                // The displayed size at its price on its side, if it's there.
                let (touch, size) = if l.buy { (*b, *bsz) } else { (*a, *asz) };
                let at = (price - touch).abs() <= HALF_CENT;
                let behind = if l.buy { price < touch } else { price > touch };
                let shown = if at || behind { size } else { 0 };
                LegQueue {
                    price,
                    ahead: (shown * model.ahead_permille).div_ceil(1000),
                    cleared: None,
                }
            })
            .collect();
        self.orders[i].queue = Some(legs);
    }

    /// A trade print on instrument `iid`: the queues of resting orders on it.
    pub(super) fn on_print(&mut self, iid: u32, price: i64, size: u64) {
        let Some(model) = self.s.queue else { return };
        let Some(m) = &self.market else { return };
        let Some(symbol) = m.names.get(&iid).cloned() else {
            return;
        };
        let minute = self.now / MINUTE;
        let mut due = Vec::new();
        for i in 0..self.orders.len() {
            if !self.open(i) || !self.orders[i].active {
                continue;
            }
            let o = &mut self.orders[i];
            let Some(queue) = &mut o.queue else { continue };
            for (l, q) in o.legs.iter().zip(queue.iter_mut()) {
                if l.symbol != symbol {
                    continue;
                }
                let through = if l.buy {
                    price < q.price - HALF_CENT
                } else {
                    price > q.price + HALF_CENT
                };
                let at = (price - q.price).abs() <= HALF_CENT;
                if through {
                    q.cleared = Some(minute);
                } else if at && !model.through_only {
                    if size > q.ahead {
                        q.ahead = 0;
                        q.cleared = Some(minute);
                    } else {
                        q.ahead -= size;
                    }
                }
            }
            if queue.iter().all(|q| q.cleared == Some(minute)) {
                due.push(i);
            }
        }
        for i in due {
            let prices: Vec<i64> = self.orders[i]
                .queue
                .as_ref()
                .map(|q| q.iter().map(|l| l.price).collect())
                .unwrap_or_default();
            let units = self.orders[i].qty - self.orders[i].filled;
            self.execute(i, units, &prices);
        }
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
                "{{\"symbol\":{:?},\"price\":\"{}\",\"ahead\":{},\"cleared_minute\":{}}}",
                l.symbol,
                super::fmt_nanos(i128::from(q.price)),
                q.ahead,
                q.cleared.map_or("null".into(), |m| m.to_string())
            );
        }
        out.push(']');
        out
    }
}

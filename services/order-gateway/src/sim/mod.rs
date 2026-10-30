//! `SimBroker`: a deterministic simulated broker behind the same `Broker` seam.
//! Scripted mode replays `FakeBroker` scenarios; market mode fills against DBN quotes in
//! client-clocked market time. Its cash, positions and activities are broker state.
mod expiry;
mod fill;
mod market;
mod script;

use crate::broker::{
    AccountInfo, Broker, BrokerError, BrokerOrder, CancelResult, Capabilities, Closing,
    FlattenResult, MarketClock, OrderRef, OrderUpdate, Position, Query, Reply, SessionDay,
    SubmitResult, Update, terminal,
};
use crate::intent::{Intent, fmt_cents, hex};
use crate::protocol::{Obj, arr, quote};
use crate::safety::{date_string, et_day, et_ns, hhmm, parse_date};
use crate::session::wall_ns;
pub use market::Feed;
use market::{QuoteSource, Rec};
use script::{OnCancel, OnSubmit, Rule, Script};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashMap};
use std::time::{Duration, Instant};

/// The simulator's model version, in every pin.
pub const MODEL: &str = "tinytrader-sim/4";

/// Whether `symbol` is an option whose expiry is before ET day `day`.
pub(crate) fn expired(symbol: &str, day: i64) -> bool {
    crate::intent::occ(symbol).is_some_and(|(_, expiry)| expiry < day)
}

/// Nanodollars per cent.
const CENT: i64 = 10_000_000;

pub(crate) struct SimLeg {
    symbol: String,
    ratio: u64,
    buy: bool,
    open: bool,
    filled: u64,
    cost: i128,
}

pub(crate) struct SimOrder {
    id: String,
    cid: String,
    combo: bool,
    legs: Vec<SimLeg>,
    qty: u64,
    limit_cents: i64,
    filled: u64,
    cost: i128,
    status: String,
    rule: Rule,
    t_act: u64,
    seq: u64,
    active: bool,
    updated: u64,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct Quote {
    bid: Option<i64>,
    ask: Option<i64>,
    bid_sz: u64,
    ask_sz: u64,
    used_bid: u64,
    used_ask: u64,
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum Action {
    Activate(usize),
    Cancel(usize),
    Expire(usize),
}

pub(crate) struct Market {
    src: QuoteSource,
    book: HashMap<u32, Quote>,
    /// Instruments a quote record other than `tcbbo` has given a book.
    quoted: std::collections::HashSet<u32>,
    /// The ET day last checked for expired contracts.
    day: i64,
    /// Each instrument id's symbol, the reverse of `ids`.
    names: HashMap<u32, String>,
    /// Fed and clocked by a session in process: the market time, shared with the feed.
    fed: Option<std::sync::Arc<std::sync::atomic::AtomicU64>>,
    ids: HashMap<String, u32>,
    actions: Vec<(u64, u64, Action)>,
    n: u64,
}

/// The simulated broker.
pub struct SimBroker {
    s: Script,
    script_sha: Option<String>,
    now: u64,
    market: Option<Market>,
    orders: Vec<SimOrder>,
    submits: u64,
    cancels: u64,
    execs: u64,
    cash: i128,
    pos: BTreeMap<String, (i64, i128)>,
    activities: Vec<String>,
    /// Non-trade activities so far, for their ids.
    acts: u64,
    /// The next session close at which options are settled, once known.
    settle_at: Option<u64>,
    updates: Vec<Update>,
    streamed: u64,
    down: Option<Instant>,
    dropped: bool,
}

/// Decimal text of a nanodollar amount, at least two places.
pub(crate) fn fmt_nanos(v: i128) -> String {
    let sign = if v < 0 { "-" } else { "" };
    let a = v.unsigned_abs();
    let frac = format!("{:09}", a % 1_000_000_000);
    let frac = frac.trim_end_matches('0');
    format!("{sign}{}.{frac:0<2}", a / 1_000_000_000)
}

fn avg(cost: i128, n: u64) -> Option<String> {
    (n > 0).then(|| fmt_nanos(cost / i128::from(n)))
}

fn error(http: u16, code: &str, message: &str) -> BrokerError {
    BrokerError {
        http_status: http,
        code: code.into(),
        message: message.into(),
        raw: Obj::new().s("code", code).s("message", message).done(),
        retry_after_ms: None,
    }
}

impl SimBroker {
    /// Scripted mode from script JSON text.
    ///
    /// # Errors
    /// An invalid script.
    pub fn scripted(script: &str) -> Result<Self, String> {
        let mut s = Self::new(script)?;
        s.now = s.s.clock_ts.unwrap_or(0);
        Ok(s)
    }

    /// Market mode: quotes from a DBN file or `tcp:HOST:PORT`; the script supplies the
    /// profile, account and calendar (`{}` for defaults).
    ///
    /// # Errors
    /// An invalid script or an unreadable quote source.
    pub fn market(script: &str, quotes: &str) -> Result<Self, String> {
        let mut s = Self::new(script)?;
        let mut src = QuoteSource::open(quotes)?;
        let first = src.peek_ts()?.unwrap_or(0);
        s.market = Some(Market {
            src,
            book: HashMap::new(),
            quoted: std::collections::HashSet::new(),
            day: 0,
            names: HashMap::new(),
            fed: None,
            ids: HashMap::new(),
            actions: Vec::new(),
            n: 0,
        });
        s.run_to(first)?;
        Ok(s)
    }

    /// Market mode fed in process by the session that clocks it: records arrive through
    /// the returned [`Feed`], and market time moves only with `advance`. `pin` says what
    /// feeds it (the session's sources and their hashes), for `pins`.
    ///
    /// # Errors
    /// An invalid script.
    pub fn fed(script: &str, pin: &str) -> Result<(Self, Feed), String> {
        let mut s = Self::new(script)?;
        let floor = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
        let (src, feed) = QuoteSource::feed(pin.to_owned(), floor.clone());
        s.market = Some(Market {
            src,
            book: HashMap::new(),
            quoted: std::collections::HashSet::new(),
            day: 0,
            names: HashMap::new(),
            fed: Some(floor),
            ids: HashMap::new(),
            actions: Vec::new(),
            n: 0,
        });
        Ok((s, feed))
    }

    /// When a submit or cancel received at market time `t` takes effect: after the
    /// strategy's declared latency and then the venue's.
    fn effective(&self) -> u64 {
        let ms = self.s.strategy_latency_ms.saturating_add(self.s.latency_ms);
        self.now.saturating_add(ms.saturating_mul(1_000_000))
    }

    fn new(script: &str) -> Result<Self, String> {
        let s = Script::parse(script)?;
        let mut sim = Self {
            cash: i128::from(s.cash_cents) * i128::from(CENT),
            script_sha: Some(hex(&Sha256::digest(script.as_bytes()))),
            s,
            now: 0,
            market: None,
            orders: Vec::new(),
            submits: 0,
            cancels: 0,
            execs: 0,
            pos: BTreeMap::new(),
            activities: Vec::new(),
            acts: 0,
            settle_at: None,
            updates: Vec::new(),
            streamed: 0,
            down: None,
            dropped: false,
        };
        let preload: Vec<_> = sim.s.orders.drain(..).collect();
        for p in preload {
            let leg = SimLeg {
                symbol: p.symbol,
                ratio: 1,
                buy: p.buy,
                open: true,
                filled: 0,
                cost: 0,
            };
            sim.create(
                p.client_order_id,
                false,
                vec![leg],
                p.qty,
                p.limit_cents,
                Rule::default(),
            );
            if let Some(o) = sim.orders.last_mut() {
                o.status = p.status;
            }
        }
        Ok(sim)
    }

    fn ts(&self) -> u64 {
        if self.s.clock_ts.is_some() || self.market.is_some() {
            self.now
        } else {
            wall_ns()
        }
    }

    fn days(&self, start: i64, end: i64) -> Option<Vec<SessionDay>> {
        let list = self.s.calendar.as_ref()?;
        if !self.s.weekdays {
            let inside =
                |d: &&SessionDay| parse_date(&d.date).is_some_and(|x| (start..=end).contains(&x));
            return Some(list.iter().filter(inside).cloned().collect());
        }
        let weekday = |d: i64| (1..=5).contains(&(d + 4).rem_euclid(7));
        Some(
            (start..=end.min(start + 400))
                .filter(|d| weekday(*d))
                .map(|d| SessionDay {
                    date: date_string(d),
                    open: "09:30".into(),
                    close: "16:00".into(),
                })
                .collect(),
        )
    }

    /// Open and close (ns) of the session on ET day `day`.
    pub(crate) fn session(&self, day: i64) -> Option<(u64, u64)> {
        let d = self.days(day, day)?.into_iter().next()?;
        Some((et_ns(day, hhmm(&d.open)?), et_ns(day, hhmm(&d.close)?)))
    }

    fn create(
        &mut self,
        cid: String,
        combo: bool,
        legs: Vec<SimLeg>,
        qty: u64,
        limit_cents: i64,
        rule: Rule,
    ) -> usize {
        let n = self.orders.len() + 1;
        self.orders.push(SimOrder {
            id: format!("00000000-0000-4000-8000-{n:012x}"),
            cid,
            combo,
            legs,
            qty,
            limit_cents,
            filled: 0,
            cost: 0,
            status: "new".into(),
            rule,
            t_act: self.now,
            seq: u64::try_from(n).unwrap_or(0),
            active: self.market.is_none(),
            updated: self.ts(),
        });
        n - 1
    }

    fn order_raw(o: &SimOrder) -> String {
        let side = |l: &SimLeg| if l.buy { "buy" } else { "sell" };
        let intent =
            |l: &SimLeg| format!("{}_to_{}", side(l), if l.open { "open" } else { "close" });
        let mut r = Obj::new()
            .s("id", &o.id)
            .s("client_order_id", &o.cid)
            .s("status", &o.status)
            .s("qty", &o.qty.to_string())
            .s("filled_qty", &o.filled.to_string())
            .opt_s("filled_avg_price", avg(o.cost, o.filled).as_deref())
            .s("order_class", if o.combo { "mleg" } else { "simple" })
            .s("limit_price", &fmt_cents(o.limit_cents));
        if o.combo {
            r = r.raw(
                "legs",
                &arr(o.legs.iter().map(|l| {
                    Obj::new()
                        .s("symbol", &l.symbol)
                        .s("side", side(l))
                        .s("ratio_qty", &l.ratio.to_string())
                        .s("position_intent", &intent(l))
                        .s("filled_qty", &l.filled.to_string())
                        .opt_s("filled_avg_price", avg(l.cost, l.filled).as_deref())
                        .done()
                })),
            );
        } else {
            let l = &o.legs[0];
            r = r
                .s("symbol", &l.symbol)
                .s("side", side(l))
                .s("position_intent", &intent(l))
                .raw("legs", "null");
        }
        r.n("updated_at_ns", o.updated).done()
    }

    fn broker_order(&self, i: usize) -> BrokerOrder {
        let o = &self.orders[i];
        let legs = o.combo.then(|| {
            arr(o.legs.iter().map(|l| {
                Obj::new()
                    .s("symbol", &l.symbol)
                    .n("filled_qty", l.filled)
                    .opt_s("avg_price", avg(l.cost, l.filled).as_deref())
                    .done()
            }))
        });
        BrokerOrder {
            client_order_id: o.cid.clone(),
            broker_order_id: o.id.clone(),
            status: o.status.clone(),
            filled_qty: o.filled,
            avg_price: avg(o.cost, o.filled),
            legs,
            updated_ts: o.updated,
            raw: Self::order_raw(o),
        }
    }

    /// Queues one stream update, honouring a scripted stream drop.
    fn push(
        &mut self,
        i: usize,
        event: &str,
        fill: Option<String>,
        legs: Option<String>,
        position: Option<i64>,
    ) {
        if self.down.is_some() {
            return;
        }
        let order = self.broker_order(i);
        let ts = self.ts();
        let raw = Obj::new()
            .s("event", event)
            .n("timestamp_ns", ts)
            .raw("order", &order.raw)
            .done();
        self.updates.push(Update::Order(Box::new(OrderUpdate {
            order,
            event: event.into(),
            ts_broker: ts,
            fill,
            legs,
            position_qty: position,
            reason: None,
            source: "sim",
            raw,
        })));
        self.streamed += 1;
        if let Some((n, _)) = self.s.drop_after
            && !self.dropped
            && self.streamed >= n
        {
            self.dropped = true;
            self.down = Some(Instant::now());
            self.updates.push(Update::StreamDown);
        }
    }

    fn set_status(&mut self, i: usize, status: &str) {
        status.clone_into(&mut self.orders[i].status);
        self.orders[i].updated = self.ts();
        self.push(i, status, None, None, None);
    }

    fn find(&self, t: &OrderRef) -> Option<usize> {
        self.orders.iter().position(|o| match t {
            OrderRef::Client(c) => o.cid == *c,
            OrderRef::Broker(b) => o.id == *b,
        })
    }

    fn open(&self, i: usize) -> bool {
        !terminal(&self.orders[i].status)
    }

    /// Script fill prices (nanodollars): the opposing quote, else the limit for a single.
    fn script_prices(&self, i: usize) -> Vec<i64> {
        let o = &self.orders[i];
        o.legs
            .iter()
            .map(|l| {
                let q = self.s.quotes.get(&l.symbol).copied().unwrap_or_default();
                let side = if l.buy { q.1 } else { q.0 };
                side.map_or(if o.combo { 0 } else { o.limit_cents }, |c| c) * CENT
            })
            .collect()
    }

    fn schedule(&mut self, t: u64, a: Action) {
        if let Some(m) = &mut self.market {
            m.n += 1;
            m.actions.push((t, m.n, a));
        }
    }

    fn cancel_one(&mut self, i: usize) -> CancelResult {
        self.cancels += 1;
        if !self.open(i) {
            return CancelResult::Rejected(error(422, "42210000", "order is not cancelable"));
        }
        match self.orders[i].rule.cancel {
            OnCancel::Ignore => {}
            OnCancel::Reject => {
                return CancelResult::Rejected(error(422, "42210000", "order is not cancelable"));
            }
            OnCancel::Ok if self.market.is_some() => {
                let t = self.effective();
                self.schedule(t, Action::Cancel(i));
            }
            OnCancel::Ok => self.set_status(i, "canceled"),
        }
        CancelResult::Accepted("null".into())
    }

    #[expect(
        clippy::unnecessary_wraps,
        reason = "the simulator answers every query"
    )]
    fn reply<T>(value: T, raw: String) -> Query<T> {
        Ok(Reply { value, raw })
    }
}

impl Broker for SimBroker {
    fn kind(&self) -> &'static str {
        "sim"
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            combo: true,
            max_legs: 4,
            client_order_id_max: 128,
            idempotency: "broker_client_id",
        }
    }

    fn pins(&self) -> String {
        let clock = match (&self.market, self.s.clock_ts) {
            (Some(_), _) => "client_clocked_market",
            (None, Some(_)) => "fixed",
            (None, None) => "wall",
        };
        let calendar = match (&self.s.calendar, self.s.weekdays) {
            (None, _) => "none",
            (Some(_), true) => "weekdays-0930-1600",
            (Some(_), false) => "script",
        };
        Obj::new()
            .s("model", MODEL)
            .s(
                "mode",
                if self.market.is_some() {
                    "market"
                } else {
                    "scripted"
                },
            )
            .s("clock", clock)
            .raw(
                "quotes",
                self.market.as_ref().map_or("null", |m| m.src.pin.as_str()),
            )
            .n("latency_ms", self.s.latency_ms)
            .n("strategy_latency_ms", self.s.strategy_latency_ms)
            .s("fee_per_contract", &fmt_cents(self.s.fee_cents))
            .s("fee_per_share", &fmt_nanos(i128::from(self.s.fee_share)))
            .raw("expiry", &self.expiry_pin())
            .s(
                "size_cap",
                if self.s.size_cap { "displayed" } else { "none" },
            )
            .s("calendar", calendar)
            .opt_s("script_sha256", self.script_sha.as_deref())
            .done()
    }

    fn now(&self) -> Option<u64> {
        Some(self.ts())
    }

    fn submit(&mut self, intent: &Intent) -> SubmitResult {
        self.submits += 1;
        let rule = self.s.rule_for(self.submits, &intent.client_order_id);
        if self.orders.iter().any(|o| o.cid == intent.client_order_id) {
            return SubmitResult::Rejected(error(
                422,
                "40010001",
                "client_order_id must be unique",
            ));
        }
        match &rule.submit {
            OnSubmit::Reject(http, code, msg) => {
                return SubmitResult::Rejected(error(*http, code, msg));
            }
            OnSubmit::ErrorBeforeSend => {
                return SubmitResult::Unknown {
                    reason: "connection",
                    retry_after_ms: None,
                };
            }
            OnSubmit::Http429(retry) => {
                return SubmitResult::Unknown {
                    reason: "http_429_after_send",
                    retry_after_ms: *retry,
                };
            }
            OnSubmit::Stall => loop {
                std::thread::sleep(Duration::from_hours(1));
            },
            _ => {}
        }
        if let Some(m) = &self.market {
            let today = et_day(self.now);
            if intent.legs.iter().any(|l| expired(&l.symbol, today)) {
                return SubmitResult::Rejected(error(
                    422,
                    "sim_expired_instrument",
                    "an option leg has expired",
                ));
            }
            if !intent.legs.iter().all(|l| m.ids.contains_key(&l.symbol)) {
                return SubmitResult::Rejected(error(
                    422,
                    "sim_unknown_instrument",
                    "no definition seen",
                ));
            }
            if !self
                .session(et_day(self.now))
                .is_some_and(|(o, c)| (o..c).contains(&self.now))
            {
                return SubmitResult::Rejected(error(
                    422,
                    "sim_market_closed",
                    "outside the session",
                ));
            }
        }
        let legs = intent
            .legs
            .iter()
            .map(|l| SimLeg {
                symbol: l.symbol.clone(),
                ratio: u64::from(l.ratio),
                buy: l.buy,
                open: l.open,
                filled: 0,
                cost: 0,
            })
            .collect();
        let lost = rule.submit == OnSubmit::TimeoutAfterAccept;
        let submit = rule.submit.clone();
        let i = self.create(
            intent.client_order_id.clone(),
            intent.combo,
            legs,
            u64::from(intent.qty),
            // A market order against quotes takes any price (a sell's at least
            // a cent); scripted, it fills at its reference price.
            if intent.market && self.market.is_some() {
                if !intent.combo && !intent.legs[0].buy {
                    1
                } else {
                    i64::MAX / CENT
                }
            } else {
                intent.limit_cents
            },
            rule,
        );
        let accepted = self.broker_order(i);
        if !lost {
            self.push(i, "new", None, None, None);
        }
        if self.market.is_some() {
            let t = self.effective();
            self.orders[i].t_act = t;
            self.schedule(t, Action::Activate(i));
        } else {
            match submit {
                OnSubmit::Fill => self.execute(i, u64::from(intent.qty), &self.script_prices(i)),
                OnSubmit::Partial(n) => self.execute(i, n, &self.script_prices(i)),
                _ => {}
            }
        }
        if lost {
            return SubmitResult::Unknown {
                reason: "timeout",
                retry_after_ms: None,
            };
        }
        SubmitResult::Accepted(accepted)
    }

    /// Cancels every open order, then closes each position at once: at its
    /// touch (bid to sell, ask to buy) in market mode, else at the scripted
    /// quote's mid, else at its average cost. Each close is an order with a
    /// fill, as Alpaca's liquidation produces.
    fn flatten(&mut self) -> FlattenResult {
        let _ = self.cancel_all();
        // Scripted positions join the book first so their closes net to zero.
        for (symbol, qty) in std::mem::take(&mut self.s.positions) {
            self.pos.entry(symbol).or_insert((0, 0)).0 += qty;
        }
        let held: Vec<(String, i64, i128)> = self
            .pos
            .iter()
            .filter(|(_, p)| p.0 != 0)
            .map(|(s, p)| (s.clone(), p.0, p.1))
            .collect();
        let mut closings = Vec::new();
        for (symbol, qty, cost) in held {
            let sell = qty > 0;
            let touch = self.market.as_ref().and_then(|m| {
                let q = m.book.get(m.ids.get(&symbol)?)?;
                if sell { q.bid } else { q.ask }
            });
            let mid = self.s.quotes.get(&symbol).and_then(|q| match q {
                (Some(b), Some(a)) => Some((b + a) / 2 * CENT),
                (Some(x), None) | (None, Some(x)) => Some(x * CENT),
                _ => None,
            });
            let avg = i64::try_from(cost / i128::from(qty)).unwrap_or(0).abs();
            let px = touch.or(mid).unwrap_or(avg);
            let cid = format!("flatten-{}-{symbol}", self.orders.len() + 1);
            let legs = vec![SimLeg {
                symbol: symbol.clone(),
                ratio: 1,
                buy: !sell,
                open: false,
                filled: 0,
                cost: 0,
            }];
            let i = self.create(cid, false, legs, qty.unsigned_abs(), 0, Rule::default());
            self.push(i, "new", None, None, None);
            self.execute(i, qty.unsigned_abs(), &[px]);
            closings.push(Closing {
                symbol,
                status: 200,
                raw: Self::order_raw(&self.orders[i]),
            });
        }
        let raw = arr(closings.iter().map(|c| {
            Obj::new()
                .s("symbol", &c.symbol)
                .n("status", c.status)
                .raw("body", &c.raw)
                .done()
        }));
        FlattenResult::Sent(closings, raw)
    }

    fn cancel(&mut self, broker_order_id: &str) -> CancelResult {
        match self.find(&OrderRef::Broker(broker_order_id.into())) {
            Some(i) => self.cancel_one(i),
            None => CancelResult::Rejected(error(404, "40410000", "order not found")),
        }
    }

    fn cancel_all(&mut self) -> CancelResult {
        for i in 0..self.orders.len() {
            if self.open(i) {
                let _ = self.cancel_one(i);
            }
        }
        CancelResult::Accepted("[]".into())
    }

    fn order(&mut self, target: &OrderRef) -> Query<Option<BrokerOrder>> {
        let Some(i) = self.find(target) else {
            return Self::reply(None, error(404, "40410000", "order not found").raw);
        };
        if self.orders[i].rule.fill_on_query && self.open(i) {
            let units = self.orders[i].qty - self.orders[i].filled;
            self.execute(i, units, &self.script_prices(i));
        }
        let o = self.broker_order(i);
        let raw = o.raw.clone();
        Self::reply(Some(o), raw)
    }

    fn open_orders(&mut self) -> Query<Vec<BrokerOrder>> {
        let list: Vec<_> = (0..self.orders.len())
            .filter(|i| self.open(*i))
            .map(|i| self.broker_order(i))
            .collect();
        let raw = arr(list.iter().map(|o| o.raw.clone()));
        Self::reply(list, raw)
    }

    fn orders_since(&mut self, after_ts: u64) -> Query<Vec<BrokerOrder>> {
        let list: Vec<_> = (0..self.orders.len())
            .filter(|i| self.orders[*i].updated >= after_ts)
            .map(|i| self.broker_order(i))
            .collect();
        let raw = arr(list.iter().map(|o| o.raw.clone()));
        Self::reply(list, raw)
    }

    fn account(&mut self) -> Query<AccountInfo> {
        let cash = fmt_nanos(self.cash);
        let raw = Obj::new()
            .s("status", &self.s.status)
            .n("options_trading_level", self.s.level)
            .s("cash", &cash)
            .s("buying_power", &cash)
            .b("trading_blocked", self.s.trading_blocked)
            .raw(
                "sim",
                &Obj::new()
                    .n("submits", self.submits)
                    .n("cancels", self.cancels)
                    .done(),
            )
            .done();
        let info = AccountInfo {
            status: self.s.status.clone(),
            cash: cash.clone(),
            buying_power: cash,
            options_trading_level: self.s.level,
            blocked: self.s.trading_blocked,
        };
        Self::reply(info, raw)
    }

    fn positions(&mut self) -> Query<Vec<Position>> {
        let mut list: Vec<Position> = self
            .pos
            .iter()
            .filter(|(_, p)| p.0 != 0)
            .map(|(s, p)| Position {
                symbol: s.clone(),
                qty: p.0,
                avg_entry_price: fmt_nanos((p.1 / i128::from(p.0)).abs()),
            })
            .collect();
        list.extend(self.s.positions.iter().map(|(s, q)| Position {
            symbol: s.clone(),
            qty: *q,
            avg_entry_price: "0.00".into(),
        }));
        let raw = arr(list.iter().map(|p| {
            Obj::new()
                .s("symbol", &p.symbol)
                .s("qty", &p.qty.to_string())
                .s("avg_entry_price", &p.avg_entry_price)
                .done()
        }));
        Self::reply(list, raw)
    }

    fn activities(
        &mut self,
        types: &[String],
        _after: Option<&str>,
        page_token: Option<&str>,
    ) -> Query<(Vec<String>, Option<String>)> {
        let start: usize = page_token.and_then(|t| t.parse().ok()).unwrap_or(0);
        let wanted = |a: &&String| {
            types.is_empty()
                || types
                    .iter()
                    .any(|t| a.contains(&format!("\"activity_type\":{}", quote(t))))
        };
        let all: Vec<String> = self.activities.iter().filter(wanted).cloned().collect();
        let page: Vec<String> = all.iter().skip(start).take(100).cloned().collect();
        let next = (start + page.len() < all.len()).then(|| (start + page.len()).to_string());
        let raw = arr(page.clone());
        Self::reply((page, next), raw)
    }

    fn clock(&mut self) -> Query<MarketClock> {
        let ts = self.ts();
        let today = et_day(ts);
        let sessions: Vec<(u64, u64)> = (today..today + 10)
            .filter_map(|d| self.session(d))
            .collect();
        let is_open = sessions.iter().any(|(o, c)| (*o..*c).contains(&ts));
        let next_open = sessions.iter().map(|s| s.0).find(|o| *o > ts).unwrap_or(0);
        let next_close = sessions.iter().map(|s| s.1).find(|c| *c > ts).unwrap_or(0);
        let raw = Obj::new()
            .n("timestamp_ns", ts)
            .b("is_open", is_open)
            .n("next_open_ns", next_open)
            .n("next_close_ns", next_close)
            .done();
        Self::reply(
            MarketClock {
                ts,
                is_open,
                next_open,
                next_close,
            },
            raw,
        )
    }

    fn calendar(&mut self, start: &str, end: &str) -> Query<Vec<SessionDay>> {
        let (Some(a), Some(b)) = (parse_date(start), parse_date(end)) else {
            return Err(error(422, "40010000", "invalid dates"));
        };
        let Some(days) = self.days(a, b) else {
            return Err(error(404, "sim_no_calendar", "no calendar configured"));
        };
        let raw = arr(days.iter().map(|d| {
            Obj::new()
                .s("date", &d.date)
                .s("open", &d.open)
                .s("close", &d.close)
                .done()
        }));
        Self::reply(days, raw)
    }

    fn poll_updates(&mut self, out: &mut Vec<Update>) {
        let down_for = Duration::from_millis(self.s.drop_after.map_or(0, |d| d.1));
        if self.down.is_some_and(|t| t.elapsed() >= down_for) {
            self.down = None;
            self.updates.push(Update::StreamUp);
        }
        out.append(&mut self.updates);
    }

    fn session_clocked(&self) -> bool {
        self.market.as_ref().is_some_and(|m| m.fed.is_some())
    }

    fn advance(&mut self, to_ts: u64) -> Result<u64, BrokerError> {
        if self.market.is_none() && self.s.clock_ts.is_none() {
            return Err(BrokerError::local(
                "unsupported",
                "advance needs a client-clocked simulator",
            ));
        }
        self.run_to(to_ts)
            .map_err(|e| error(500, "sim_quote_source", &e))?;
        Ok(self.now)
    }
}

impl SimBroker {
    /// Applies quote records and order actions up to `to` in the section 10 order.
    fn run_to(&mut self, to: u64) -> Result<(), String> {
        if self.settle_at.is_none() && self.market.is_some() && self.s.expiry.settle {
            // A fed simulator's clock starts at zero: settle from its first day.
            let from = if self.now == 0 { to } else { self.now };
            self.settle_at = self.next_close(et_day(from));
        }
        while let Some(m) = &mut self.market {
            // A session's close settles that day's expiries: after every record and
            // order action at or before it, before anything after it.
            if let Some(c) = self.settle_at.filter(|c| *c <= to) {
                let rec = m.src.peek_ts()?.filter(|t| *t <= c);
                let act = m.actions.iter().any(|a| a.0 <= c);
                if rec.is_none() && !act {
                    self.now = self.now.max(c);
                    self.settle_day(et_day(c), c)?;
                    self.settle_at = self.next_close(et_day(c) + 1);
                    continue;
                }
            }
            let rec = m.src.peek_ts()?.filter(|t| *t <= to);
            let act = m
                .actions
                .iter()
                .enumerate()
                .filter(|(_, a)| a.0 <= to)
                .min_by_key(|(_, a)| (a.0, a.1))
                .map(|(k, a)| (k, a.0));
            match (rec, act) {
                (_, Some((k, t))) if rec.is_none_or(|r| t < r) => {
                    let (_, _, a) = m.actions.remove(k);
                    self.now = self.now.max(t);
                    self.act(a);
                }
                (Some(_), _) => {
                    // Every record of one timestamp applies before orders are evaluated;
                    // the last quote of an instrument in that instant is its book. A
                    // combo never fills on some legs' new quotes and others' old ones,
                    // nor on a quote replaced within the same instant.
                    let Some((t, r)) = m.src.next() else { break };
                    self.now = self.now.max(t);
                    let mut touched = Vec::new();
                    let mut next = Some(r);
                    while let Some(r) = next.take() {
                        touched.extend(self.apply(r));
                        if let Some(m) = &mut self.market
                            && m.src.peek_ts()? == Some(t)
                        {
                            next = m.src.next().map(|(_, r)| r);
                        }
                    }
                    self.evaluate_touched(&touched);
                }
                _ => break,
            }
        }
        self.now = self.now.max(to);
        self.forget_expired();
        if let Some(floor) = self.market.as_ref().and_then(|m| m.fed.as_ref()) {
            floor.store(self.now, std::sync::atomic::Ordering::SeqCst);
        }
        Ok(())
    }

    /// The close of the first session on or after ET day `day`, within a fortnight.
    fn next_close(&self, day: i64) -> Option<u64> {
        (day..day + 14)
            .filter_map(|d| self.session(d))
            .map(|(_, c)| c)
            .find(|c| *c >= self.now)
    }

    /// Once a day has passed an option's expiry, forgets the contract and its book:
    /// it can't be quoted, and its id may name another contract by now.
    fn forget_expired(&mut self) {
        let today = et_day(self.now);
        let Some(m) = &mut self.market else { return };
        if m.day == today {
            return;
        }
        m.day = today;
        let gone: Vec<(String, u32)> = m
            .ids
            .iter()
            .filter(|(s, _)| expired(s, today))
            .map(|(s, i)| (s.clone(), *i))
            .collect();
        for (s, i) in gone {
            m.ids.remove(&s);
            m.names.remove(&i);
            m.book.remove(&i);
            m.quoted.remove(&i);
        }
    }

    /// The expiry settings, for `pins`.
    fn expiry_pin(&self) -> String {
        let e = &self.s.expiry;
        let styles = e
            .styles
            .iter()
            .fold(Obj::new(), |o, (k, v)| o.s(k, &format!("{v:?}")))
            .done();
        let divs = e
            .dividends_sha
            .iter()
            .fold(Obj::new(), |o, (k, v)| o.s(k, v))
            .done();
        Obj::new()
            .b("settle", e.settle)
            .s(
                "model",
                "settle at the session close: cash at intrinsic, physical at the strike",
            )
            .raw("styles", &styles)
            .opt_s("settlement_file_sha256", e.prices_sha.as_deref())
            .b("settle_from_quote", e.from_quote)
            .s("exercise_threshold", &fmt_nanos(i128::from(e.threshold)))
            .b("early_assignment", e.early)
            .raw("dividends_sha256", &divs)
            .s(
                "put_assignment_extrinsic",
                &fmt_nanos(i128::from(e.put_extrinsic)),
            )
            .done()
    }

    /// Applies one record to the book; a quote returns its instrument id.
    fn apply(&mut self, r: Rec) -> Option<u32> {
        let m = self.market.as_mut()?;
        match r {
            Rec::Def { iid, symbol } => {
                // OPRA assigns instrument ids per day: an id can name another contract
                // tomorrow, and a contract another id. Whatever an id meant before is
                // forgotten, its quote included, so no order fills against another
                // contract's book.
                if let Some(was) = m.names.insert(iid, symbol.clone()).filter(|w| *w != symbol) {
                    // The id named another contract: forget that one and its quote.
                    m.ids.remove(&was);
                    m.book.remove(&iid);
                    m.quoted.remove(&iid);
                }
                if let Some(old) = m.ids.insert(symbol, iid).filter(|old| *old != iid) {
                    // The contract had another id: that id's quote isn't its own now.
                    m.names.remove(&old);
                    m.book.remove(&old);
                    m.quoted.remove(&old);
                }
                None
            }
            Rec::Quote {
                iid,
                bid,
                ask,
                bid_sz,
                ask_sz,
                trade,
            } => {
                // A tcbbo quote is the book only where no quote record gives one: beside
                // cmbp-1 it would put back a quote a trade may have consumed.
                if trade && m.quoted.contains(&iid) {
                    return None;
                }
                if !trade {
                    m.quoted.insert(iid);
                }
                m.book.insert(
                    iid,
                    Quote {
                        bid,
                        ask,
                        bid_sz,
                        ask_sz,
                        used_bid: 0,
                        used_ask: 0,
                    },
                );
                Some(iid)
            }
        }
    }

    /// Evaluates the working orders with a leg on any of `iids`, in (`t_act`, `seq`) order.
    fn evaluate_touched(&mut self, iids: &[u32]) {
        let Some(m) = &self.market else { return };
        let ids = &m.ids;
        let mut touched: Vec<usize> = (0..self.orders.len())
            .filter(|i| {
                self.orders[*i]
                    .legs
                    .iter()
                    .any(|l| ids.get(&l.symbol).is_some_and(|id| iids.contains(id)))
            })
            .collect();
        touched.sort_by_key(|i| (self.orders[*i].t_act, self.orders[*i].seq));
        for i in touched {
            self.evaluate(i);
        }
    }

    fn act(&mut self, a: Action) {
        match a {
            Action::Activate(i) if self.open(i) => {
                self.orders[i].active = true;
                if let Some((_, close)) = self.session(et_day(self.now)) {
                    self.schedule(close.max(self.now), Action::Expire(i));
                }
                self.evaluate(i);
            }
            Action::Cancel(i) if self.open(i) => self.set_status(i, "canceled"),
            Action::Expire(i) if self.open(i) => self.set_status(i, "expired"),
            _ => {}
        }
    }
}

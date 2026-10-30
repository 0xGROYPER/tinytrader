//! Option expiry in market mode: at the session close of each day, options
//! expiring that day are settled as a broker would, as account activities
//! (`OPEXP`, `OPEXC`, `OPASN`), and, where the profile asks for it, short options
//! are assigned early.
//!
//! - **Cash-settled** roots (index options) pay their intrinsic value in cash
//!   against the settlement value: the PM close (`cash-pm`, SPXW and the like) or
//!   the AM special opening quotation (`cash-am`, SPX monthlies).
//! - **Physically settled** roots (SPY and other stocks and ETFs) exercise long and
//!   assign short options in the money by at least the exercise threshold ($0.01,
//!   as the OCC's), into the underlying at the strike, against the underlying's
//!   price at the close.
//! - A settlement price comes from the profile's settlement file (root, date, kind,
//!   value; pinned by SHA-256), or, where the profile allows, the venue's own quote
//!   of the underlying at the close (its mid). A price that can't be had fails the
//!   run: never a guess.
//! - **Early assignment** (opt-in, `early_assignment`): at the close of the day
//!   before an ex-dividend date, a short call in the money whose extrinsic value
//!   (its bid less its intrinsic value) is below the dividend is assigned; and at
//!   any close, a short put in the money whose extrinsic value is below
//!   `put_assignment_extrinsic`. Pin risk and the holder's choice not to exercise
//!   are not modelled.
use super::{SimBroker, fmt_nanos};
use crate::broker::Update;
use crate::protocol::Obj;
use crate::safety::{date_string, et_day};
use std::collections::{BTreeMap, HashMap};

/// How a root settles.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Style {
    CashPm,
    CashAm,
    Physical,
}

impl Style {
    pub(crate) fn parse(s: &str) -> Option<Self> {
        match s {
            "cash-pm" => Some(Self::CashPm),
            "cash-am" => Some(Self::CashAm),
            "physical" => Some(Self::Physical),
            _ => None,
        }
    }

    fn kind(self) -> &'static str {
        match self {
            Self::CashAm => "AM-SOQ",
            Self::CashPm | Self::Physical => "PM-close",
        }
    }
}

/// The index roots that settle in cash unless the profile says otherwise.
fn default_style(root: &str) -> Style {
    match root {
        "SPX" | "NDX" | "RUT" | "VIX" | "DJX" => Style::CashAm,
        "SPXW" | "XSP" | "NDXP" | "RUTW" | "VIXW" | "MRUT" | "XND" => Style::CashPm,
        _ => Style::Physical,
    }
}

/// The expiry settings of a profile.
#[derive(Clone, Debug, Default)]
pub(crate) struct Expiry {
    /// Whether options are settled at all (on by default).
    pub(crate) settle: bool,
    /// Per-root overrides of [`default_style`].
    pub(crate) styles: BTreeMap<String, Style>,
    /// `(root, date, kind)` → value, nanodollars.
    pub(crate) prices: HashMap<(String, String, String), i64>,
    /// The settlement file's SHA-256, for the pin.
    pub(crate) prices_sha: Option<String>,
    /// Whether a missing price may come from the venue's quote of the underlying.
    pub(crate) from_quote: bool,
    /// ITM by at least this much exercises and assigns, nanodollars.
    pub(crate) threshold: i64,
    /// Root → ex-date → dividend, nanodollars per share.
    pub(crate) dividends: BTreeMap<String, BTreeMap<String, i64>>,
    pub(crate) dividends_sha: BTreeMap<String, String>,
    pub(crate) early: bool,
    /// A short put's extrinsic value below which it is assigned early, nanodollars.
    pub(crate) put_extrinsic: i64,
}

/// Root, call or put, and strike (nanodollars) of a compact OCC symbol.
pub(crate) fn contract(symbol: &str) -> Option<(&str, bool, i64)> {
    let (root, _) = crate::intent::occ(symbol)?;
    let tail = &symbol[symbol.len() - 9..];
    let call = tail.starts_with('C');
    let strike: i64 = tail[1..].parse().ok()?;
    Some((root, call, strike * 1_000_000))
}

/// A decimal dollar amount as nanodollars: `5812.34`, `0.005`.
pub(crate) fn nanos(s: &str) -> Option<i64> {
    let s = s.trim();
    let (neg, s) = s.strip_prefix('-').map_or((false, s), |r| (true, r));
    let (whole, frac) = s.split_once('.').unwrap_or((s, ""));
    if frac.len() > 9 || whole.is_empty() && frac.is_empty() {
        return None;
    }
    let w: i64 = if whole.is_empty() {
        0
    } else {
        whole.parse().ok()?
    };
    let f: i64 = if frac.is_empty() {
        0
    } else {
        format!("{frac:0<9}").parse().ok()?
    };
    let v = w.checked_mul(1_000_000_000)?.checked_add(f)?;
    Some(if neg { -v } else { v })
}

/// The rows of a CSV with a header, as maps from column to value.
pub(crate) fn csv(text: &str) -> Vec<HashMap<String, String>> {
    let mut lines = text.lines().filter(|l| !l.trim().is_empty());
    let Some(head) = lines.next() else {
        return Vec::new();
    };
    let cols: Vec<String> = head.split(',').map(|c| c.trim().to_lowercase()).collect();
    lines
        .map(|l| {
            cols.iter()
                .cloned()
                .zip(l.split(',').map(|v| v.trim().to_owned()))
                .collect()
        })
        .collect()
}

/// What happened to one option position.
struct Settled {
    symbol: String,
    qty: i64,
    kind: &'static str,
    price: i64,
}

impl SimBroker {
    /// The settlement price of `root` on ET day `day`, by its style.
    fn settlement(&self, root: &str, day: i64, style: Style) -> Result<i64, String> {
        let e = &self.s.expiry;
        let date = date_string(day);
        if let Some(v) = e
            .prices
            .get(&(root.to_owned(), date.clone(), style.kind().to_owned()))
        {
            return Ok(*v);
        }
        if e.from_quote && style == Style::Physical {
            let m = self.market.as_ref();
            if let Some(q) = m
                .and_then(|m| m.ids.get(root))
                .and_then(|id| m.and_then(|m| m.book.get(id)))
                && let (Some(b), Some(a)) = (q.bid, q.ask)
            {
                return Ok((b + a) / 2);
            }
        }
        Err(format!(
            "no {} settlement price for {root} on {date}: add it to the settlement file{}",
            style.kind(),
            if style == Style::Physical {
                " or quote the underlying in this venue"
            } else {
                ""
            }
        ))
    }

    /// The style `root` settles by.
    fn style(&self, root: &str) -> Style {
        self.s
            .expiry
            .styles
            .get(root)
            .copied()
            .unwrap_or_else(|| default_style(root))
    }

    /// Settles every option position expiring on ET day `day` (or before, missed),
    /// then assigns early where the profile asks. `ts` is the session's close.
    pub(super) fn settle_day(&mut self, day: i64, ts: u64) -> Result<(), String> {
        let open: Vec<(String, i64)> = self
            .pos
            .iter()
            .filter(|(_, p)| p.0 != 0)
            .map(|(s, p)| (s.clone(), p.0))
            .collect();
        let mut done = Vec::new();
        for (symbol, qty) in &open {
            let Some((root, call, strike)) = contract(symbol) else {
                continue;
            };
            let Some((_, expiry)) = crate::intent::occ(symbol) else {
                continue;
            };
            if expiry > day {
                continue;
            }
            let style = self.style(root);
            let s = self.settlement(root, expiry, style)?;
            let intrinsic = if call { s - strike } else { strike - s }.max(0);
            // Cash pays any intrinsic value; delivery needs the threshold.
            let floor = if style == Style::Physical {
                self.s.expiry.threshold.max(1)
            } else {
                1
            };
            let kind = if intrinsic < floor {
                "OPEXP"
            } else if *qty > 0 {
                "OPEXC"
            } else {
                "OPASN"
            };
            done.push((
                symbol.clone(),
                *qty,
                kind,
                root.to_owned(),
                call,
                strike,
                style,
                s,
                intrinsic,
            ));
        }
        let mut out = Vec::new();
        for (symbol, qty, kind, root, call, strike, style, s, intrinsic) in done {
            self.close_option(&symbol);
            if kind != "OPEXP" {
                if style == Style::Physical {
                    self.deliver(&root, call, strike, qty);
                } else {
                    self.cash += i128::from(qty) * 100 * i128::from(intrinsic);
                }
            }
            out.push(Settled {
                symbol,
                qty,
                kind,
                price: s,
            });
        }
        if self.s.expiry.early {
            out.extend(self.assign_early(day)?);
        }
        for s in out {
            self.activity(&s, ts);
        }
        Ok(())
    }

    /// Short options assigned before expiry, at the close of ET day `day`.
    fn assign_early(&mut self, day: i64) -> Result<Vec<Settled>, String> {
        let next = (day + 1..day + 8).find(|d| self.session(*d).is_some());
        let mut hits = Vec::new();
        let shorts: Vec<(String, i64)> = self
            .pos
            .iter()
            .filter(|(_, p)| p.0 < 0)
            .map(|(s, p)| (s.clone(), p.0))
            .collect();
        for (symbol, qty) in shorts {
            let Some((root, call, strike)) =
                contract(&symbol).map(|(r, c, k)| (r.to_owned(), c, k))
            else {
                continue;
            };
            let root = root.as_str();
            let style = self.style(root);
            if style != Style::Physical {
                continue; // European: no early exercise
            }
            let Some(bid) = self
                .market
                .as_ref()
                .and_then(|m| m.ids.get(&symbol).and_then(|id| m.book.get(id)))
                .and_then(|q| q.bid)
            else {
                continue;
            };
            let s = self.settlement(root, day, style)?;
            let intrinsic = if call { s - strike } else { strike - s };
            if intrinsic <= 0 {
                continue;
            }
            let extrinsic = bid - intrinsic;
            let due = if call {
                let dividend = next.and_then(|d| {
                    self.s
                        .expiry
                        .dividends
                        .get(root)
                        .and_then(|m| m.get(&date_string(d)))
                        .copied()
                });
                dividend.is_some_and(|div| extrinsic < div)
            } else {
                extrinsic < self.s.expiry.put_extrinsic
            };
            if due {
                hits.push((symbol, qty, root.to_owned(), call, strike, s));
            }
        }
        let mut out = Vec::new();
        for (symbol, qty, root, call, strike, s) in hits {
            self.close_option(&symbol);
            self.deliver(&root, call, strike, qty);
            out.push(Settled {
                symbol,
                qty,
                kind: "OPASN",
                price: s,
            });
        }
        Ok(out)
    }

    /// Removes an option position.
    fn close_option(&mut self, symbol: &str) {
        self.pos.remove(symbol);
    }

    /// Exercise or assignment of `qty` contracts (signed: long +, short −) into
    /// 100 shares each of `root` at the strike.
    fn deliver(&mut self, root: &str, call: bool, strike: i64, qty: i64) {
        // A long call or a short put buys shares; a long put or a short call sells.
        let shares = if call { 100 * qty } else { -100 * qty };
        self.book_position(root, shares, strike);
        self.cash -= i128::from(shares) * i128::from(strike);
    }

    fn activity(&mut self, s: &Settled, ts: u64) {
        self.acts += 1;
        let raw = Obj::new()
            .s("activity_type", s.kind)
            .s("id", &format!("sim-act-{:06}", self.acts))
            .s("symbol", &s.symbol)
            .s("qty", &s.qty.to_string())
            .s("price", &fmt_nanos(i128::from(s.price)))
            .s("date", &date_string(et_day(ts)))
            .n("transaction_time_ns", ts)
            .done();
        self.activities.push(raw.clone());
        if self.down.is_none() {
            self.updates.push(Update::Activity {
                activity_type: s.kind.to_owned(),
                raw,
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{contract, csv, nanos};
    use crate::broker::{Broker, Update};
    use crate::sim::{Feed, SimBroker};
    use dbn::{CbboMsg, RecordHeader, RecordRef, SType, Schema, SymbolMappingMsg, rtype};

    const S: u64 = 1_000_000_000;
    /// 2026-10-05 10:00 New York (a Monday), and that day's 16:00 close.
    const T0: u64 = 1_791_208_800_000_000_000;
    const CLOSE: u64 = 1_791_230_400_000_000_000;
    const D: i64 = 1_000_000_000;

    fn push(feed: &Feed, iid: u32, raw: &str, bid: i64, ask: i64, ts: u64) {
        let m = SymbolMappingMsg::new(iid, ts, SType::Parent, "X", SType::RawSymbol, raw, 0, 0)
            .unwrap();
        feed.push(ts, RecordRef::from(&m)).unwrap();
        let mut q = CbboMsg {
            hd: RecordHeader::new::<CbboMsg>(rtype::CBBO_1M, 30, iid, u64::MAX),
            ts_recv: ts,
            ..CbboMsg::default_for_schema(Schema::Cbbo1M)
        };
        q.levels[0].bid_px = bid;
        q.levels[0].ask_px = ask;
        feed.push(ts, RecordRef::from(&q)).unwrap();
    }

    fn sim(script: &str) -> (SimBroker, Feed) {
        SimBroker::fed(script, "{}").unwrap()
    }

    fn kinds(sim: &mut SimBroker) -> Vec<(String, String)> {
        let mut out = Vec::new();
        sim.poll_updates(&mut out);
        out.into_iter()
            .filter_map(|u| match u {
                Update::Activity { activity_type, raw } => {
                    let v: serde_json::Value = serde_json::from_str(&raw).unwrap();
                    Some((activity_type, v["symbol"].as_str().unwrap().to_owned()))
                }
                _ => None,
            })
            .collect()
    }

    #[test]
    fn contracts_prices_and_files_parse() {
        assert_eq!(
            contract("SPY260717C00749000"),
            Some(("SPY", true, 749_000_000_000))
        );
        assert_eq!(
            contract("SPXW261030P05812500"),
            Some(("SPXW", false, 5_812_500_000_000))
        );
        assert_eq!(nanos("5812.34"), Some(5_812_340_000_000));
        assert_eq!(nanos("0.005"), Some(5_000_000));
        assert_eq!(nanos("-1.5"), Some(-1_500_000_000));
        assert_eq!(nanos("x"), None);
        let rows = csv("root,date,kind,value\nSPXW,2026-10-30,PM-close,5812.34\n");
        assert_eq!(rows[0]["value"], "5812.34");
    }

    /// Physical settlement against the venue's quote of the underlying: a long call
    /// in the money is exercised into shares at the strike, a short call is
    /// assigned, a put out of the money expires, and a contract a cent short of the
    /// threshold expires too.
    #[test]
    fn physical_options_exercise_and_assign_at_the_close() {
        let (mut sim, feed) = sim("{}");
        push(&feed, 9, "SPY", 589_990_000_000, 590_010_000_000, T0);
        sim.pos.insert("SPY261005C00580000".into(), (2, 0));
        sim.pos.insert("SPY261005C00585000".into(), (-1, 0));
        sim.pos.insert("SPY261005P00570000".into(), (-3, 0));
        sim.pos.insert("SPY261005C00590000".into(), (1, 0)); // at the money: expires
        let cash = sim.cash;
        sim.advance(CLOSE + S).unwrap();
        let mut k = kinds(&mut sim);
        k.sort();
        assert_eq!(
            k,
            [
                ("OPASN".into(), "SPY261005C00585000".into()),
                ("OPEXC".into(), "SPY261005C00580000".into()),
                ("OPEXP".into(), "SPY261005C00590000".into()),
                ("OPEXP".into(), "SPY261005P00570000".into()),
            ]
        );
        // +200 shares at 580, −100 at 585: 100 shares, 58,500 less cash.
        assert_eq!(sim.pos.get("SPY").map(|p| p.0), Some(100));
        assert_eq!(sim.cash - cash, i128::from(-200 * 580 * D + 100 * 585 * D));
        assert!(sim.pos.keys().all(|s| !s.starts_with("SPY26")));
    }

    /// Cash settlement from the settlement file: SPXW at the PM close, intrinsic in
    /// cash, no shares.
    #[test]
    fn index_options_settle_in_cash_from_the_settlement_file() {
        let dir = std::env::temp_dir().join(format!("sim-settle-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("settle.csv");
        std::fs::write(
            &file,
            "root,date,kind,value\nSPXW,2026-10-05,PM-close,5812.50\n",
        )
        .unwrap();
        let script = format!(
            r#"{{"expiry":{{"settlement_file":{:?}}}}}"#,
            file.display().to_string()
        );
        let (mut sim, _feed) = sim(&script);
        sim.pos.insert("SPXW261005C05800000".into(), (1, 0));
        sim.pos.insert("SPXW261005P05800000".into(), (-1, 0));
        let cash = sim.cash;
        sim.advance(CLOSE + S).unwrap();
        assert_eq!(sim.cash - cash, i128::from(100 * 12_500_000_000_i64)); // 12.50 × 100
        assert!(sim.pos.is_empty());
        let pins: serde_json::Value = serde_json::from_str(&sim.pins()).unwrap();
        assert_eq!(
            pins["expiry"]["settlement_file_sha256"]
                .as_str()
                .unwrap()
                .len(),
            64
        );
    }

    /// No price for a settlement fails the run, never a guess.
    #[test]
    fn a_missing_settlement_price_fails() {
        let (mut sim, _feed) = sim(r#"{"expiry":{"settle_from_quote":false}}"#);
        sim.pos.insert("SPY261005C00580000".into(), (1, 0));
        let e = sim.advance(CLOSE + S).unwrap_err();
        assert!(
            e.message
                .contains("no PM-close settlement price for SPY on 2026-10-05"),
            "{}",
            e.message
        );
    }

    /// Early assignment: a short call whose extrinsic value is below tomorrow's
    /// dividend is assigned at today's close; one with more time value isn't.
    #[test]
    fn short_calls_are_assigned_before_an_ex_dividend_date() {
        let dir = std::env::temp_dir().join(format!("sim-div-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("spy.csv");
        std::fs::write(&file, "ex_date,dividend_usd\n2026-10-06,1.80\n").unwrap();
        let script = format!(
            r#"{{"expiry":{{"early_assignment":true,"dividends":{{"SPY":{:?}}}}}}}"#,
            file.display().to_string()
        );
        let (mut sim, feed) = sim(&script);
        push(&feed, 9, "SPY", 599_990_000_000, 600_010_000_000, T0);
        // Intrinsic 20: one bid 21.00 (extrinsic 1.00 < 1.80), one bid 23.00 (3.00).
        push(
            &feed,
            1,
            "SPY261016C00580000",
            21_000_000_000,
            21_100_000_000,
            T0,
        );
        push(
            &feed,
            2,
            "SPY261120C00580000",
            23_000_000_000,
            23_100_000_000,
            T0,
        );
        sim.pos.insert("SPY261016C00580000".into(), (-1, 0));
        sim.pos.insert("SPY261120C00580000".into(), (-1, 0));
        sim.advance(CLOSE + S).unwrap();
        assert_eq!(
            kinds(&mut sim),
            [("OPASN".to_owned(), "SPY261016C00580000".to_owned())]
        );
        assert_eq!(sim.pos.get("SPY").map(|p| p.0), Some(-100));
        assert_eq!(sim.pos.get("SPY261120C00580000").map(|p| p.0), Some(-1));
    }
}

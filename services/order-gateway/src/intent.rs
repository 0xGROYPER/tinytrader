//! The v1 order intent: protocol validation, canonical form and `intent_sha256`.
use crate::protocol::{Fields, Invalid, Obj, arr, invalid};
use crate::safety::days_from_civil;
use serde_json::Value;
use sha2::{Digest, Sha256};

/// One leg; a single order is one leg with ratio 1.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Leg {
    /// Compact OCC symbol, or a stock's ticker when `equity`.
    pub symbol: String,
    /// `class: equity` (shares; single orders only); otherwise `option`.
    pub equity: bool,
    /// Ratio per parent unit (1 for a single).
    pub ratio: u32,
    /// `side: buy`.
    pub buy: bool,
    /// `effect: open`.
    pub open: bool,
}

/// A validated day intent of one allowlisted shape: a limit order, or a
/// market order carrying the client's reference price for the caps.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Intent {
    /// The only logical order key.
    pub client_order_id: String,
    /// `shape: combo`.
    pub combo: bool,
    /// One leg for a single, 2 to 4 for a combo.
    pub legs: Vec<Leg>,
    /// Contracts (parent units for a combo).
    pub qty: u32,
    /// Limit in cents; a combo is signed (+ debit, − credit). For a market
    /// order, its `ref_price`: what the client expects it to cost (the
    /// natural price when sent), which the notional cap is checked against.
    /// It doesn't bound the fill.
    pub limit_cents: i64,
    /// `type: market`: sent with no price; fills at whatever the market gives.
    pub market: bool,
    /// `time_in_force: cls`: the closing auction (limit-on-close), stock singles only.
    pub on_close: bool,
}

pub(crate) fn valid_id(id: &str) -> bool {
    (1..=128).contains(&id.len())
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b':' | b'-'))
}

/// Root and expiry (days since the epoch) of a compact OCC symbol.
pub(crate) fn occ(symbol: &str) -> Option<(&str, i64)> {
    let b = symbol.as_bytes();
    let root = b.len().checked_sub(15).filter(|n| (1..=6).contains(n))?;
    let digits = |r: std::ops::Range<usize>| b[r].iter().all(u8::is_ascii_digit);
    if !b[..root].iter().all(u8::is_ascii_uppercase)
        || !digits(root..root + 6)
        || !matches!(b[root + 6], b'C' | b'P')
        || !digits(root + 7..b.len())
    {
        return None;
    }
    let num = |i: usize| i64::from(b[i] - b'0') * 10 + i64::from(b[i + 1] - b'0');
    let (y, m, d) = (2000 + num(root), num(root + 2), num(root + 4));
    let days = days_from_civil(y, u32::try_from(m).ok()?, u32::try_from(d).ok()?);
    let back = crate::safety::civil_from_days(days);
    (back == (y, u32::try_from(m).ok()?, u32::try_from(d).ok()?)).then_some((&symbol[..root], days))
}

/// Parses a decimal price with at most two decimal places into cents.
pub(crate) fn parse_cents(s: &str) -> Option<i64> {
    let (neg, body) = s.strip_prefix('-').map_or((false, s), |b| (true, b));
    let (int, frac) = body.split_once('.').unwrap_or((body, "00"));
    let ok = |p: &str| p.bytes().all(|b| b.is_ascii_digit());
    if int.is_empty() || int.len() > 9 || frac.is_empty() || frac.len() > 2 || !ok(int) || !ok(frac)
    {
        return None;
    }
    let cents = int.parse::<i64>().ok()? * 100
        + frac.parse::<i64>().ok()? * if frac.len() == 1 { 10 } else { 1 };
    Some(if neg { -cents } else { cents })
}

/// Formats cents with exactly two decimal places.
pub(crate) fn fmt_cents(c: i64) -> String {
    let sign = if c < 0 { "-" } else { "" };
    let a = c.unsigned_abs();
    format!("{sign}{}.{:02}", a / 100, a % 100)
}

fn gcd(a: u32, b: u32) -> u32 {
    if b == 0 { a } else { gcd(b, a % b) }
}

/// A stock ticker: 1 to 5 capitals, with an optional class suffix (`BRK.B`).
pub(crate) fn ticker(symbol: &str) -> bool {
    let (base, class) = symbol.split_once('.').unwrap_or((symbol, "A"));
    (1..=5).contains(&base.len())
        && base.bytes().all(|b| b.is_ascii_uppercase())
        && class.len() == 1
        && class.bytes().all(|b| b.is_ascii_uppercase())
}

/// `(symbol, equity)`.
fn instrument(f: &mut Fields) -> Result<(String, bool), Invalid> {
    let mut i = Fields::new(f.req("instrument")?, &f.at("instrument"))?;
    let equity = match i.string("class")?.as_str() {
        "option" => false,
        "equity" => true,
        _ => return Err(invalid(&i.at("class"), "option or equity")),
    };
    let symbol = i.string("symbol")?;
    if equity && !ticker(&symbol) {
        return Err(invalid(
            &i.at("symbol"),
            "a stock ticker such as AAPL or BRK.B",
        ));
    }
    if !equity && occ(&symbol).is_none() {
        return Err(invalid(
            &i.at("symbol"),
            "compact OCC symbol with a valid date",
        ));
    }
    i.finish()?;
    Ok((symbol, equity))
}

fn side_effect(f: &mut Fields) -> Result<(bool, bool), Invalid> {
    let buy = match f.string("side")?.as_str() {
        "buy" => true,
        "sell" => false,
        _ => return Err(invalid(&f.at("side"), "buy or sell")),
    };
    let open = match f.string("effect")?.as_str() {
        "open" => true,
        "close" => false,
        _ => return Err(invalid(&f.at("effect"), "open or close")),
    };
    Ok((buy, open))
}

impl Intent {
    /// Protocol validation (section 6); the path prefix is `intent`.
    pub(crate) fn parse(v: Value) -> Result<Self, Invalid> {
        let mut f = Fields::new(v, "intent")?;
        let client_order_id = f.string("client_order_id")?;
        if !valid_id(&client_order_id) {
            return Err(invalid(
                &f.at("client_order_id"),
                "1 to 128 of [A-Za-z0-9._:-]",
            ));
        }
        let combo = match f.string("shape")?.as_str() {
            "single" => false,
            "combo" => true,
            _ => return Err(invalid(&f.at("shape"), "single or combo")),
        };
        let legs = if combo {
            let path = f.at("legs");
            let raw = f.array("legs")?;
            if !(2..=4).contains(&raw.len()) {
                return Err(invalid(&path, "2 to 4 legs"));
            }
            let mut legs = Vec::new();
            for (n, v) in raw.into_iter().enumerate() {
                let mut l = Fields::new(v, &format!("{path}[{n}]"))?;
                let (symbol, equity) = instrument(&mut l)?;
                if equity {
                    return Err(invalid(&format!("{path}[{n}]"), "combos are options only"));
                }
                let ratio = u32::try_from(l.int("ratio", 1, 10_000)?).unwrap_or(1);
                let (buy, open) = side_effect(&mut l)?;
                l.finish()?;
                if legs.iter().any(|x: &Leg| x.symbol == symbol) {
                    return Err(invalid(&path, "leg symbols must be distinct"));
                }
                legs.push(Leg {
                    symbol,
                    equity,
                    ratio,
                    buy,
                    open,
                });
            }
            if legs.iter().fold(0, |g, l| gcd(g, l.ratio)) != 1 {
                return Err(invalid(&path, "leg ratios must have gcd 1"));
            }
            legs
        } else {
            let (symbol, equity) = instrument(&mut f)?;
            let (buy, open) = side_effect(&mut f)?;
            vec![Leg {
                symbol,
                equity,
                ratio: 1,
                buy,
                open,
            }]
        };
        let qty = u32::try_from(f.int("qty", 1, 1_000_000)?).unwrap_or(1);
        let market = match f.string("type")?.as_str() {
            "limit" => false,
            "market" => true,
            _ => return Err(invalid(&f.at("type"), "limit or market")),
        };
        // A market order names no limit, but a reference price for the caps.
        let price_field = if market { "ref_price" } else { "limit_price" };
        let price_path = f.at(price_field);
        let limit_cents = parse_cents(&f.string(price_field)?)
            .filter(|c| if combo { *c != 0 } else { *c > 0 })
            .ok_or_else(|| {
                invalid(
                    &price_path,
                    "decimal, at most 2 places; single > 0, combo != 0",
                )
            })?;
        let on_close = match f.string("time_in_force")?.as_str() {
            "day" => false,
            "cls" if !combo && legs[0].equity && !market => true,
            _ => {
                return Err(invalid(
                    &f.at("time_in_force"),
                    "day, or cls for a stock single limit",
                ));
            }
        };
        f.finish()?;
        Ok(Self {
            client_order_id,
            combo,
            legs,
            qty,
            limit_cents,
            market,
            on_close,
        })
    }

    /// Whether this is a stock order (a single equity leg).
    #[must_use]
    pub fn is_equity(&self) -> bool {
        !self.combo && self.legs[0].equity
    }

    /// Dollars per unit of price: 100 for an option contract, 1 for a share.
    #[must_use]
    pub fn multiplier(&self) -> u64 {
        if self.is_equity() { 1 } else { 100 }
    }

    /// Whether any leg opens a position.
    #[must_use]
    pub fn has_open(&self) -> bool {
        self.legs.iter().any(|l| l.open)
    }

    /// The canonical intent: fixed field order, no whitespace, two-place prices.
    #[must_use]
    pub fn canonical(&self) -> String {
        let instrument = |l: &Leg| {
            Obj::new()
                .s("class", if l.equity { "equity" } else { "option" })
                .s("symbol", &l.symbol)
                .done()
        };
        let side = |l: &Leg| if l.buy { "buy" } else { "sell" };
        let effect = |l: &Leg| if l.open { "open" } else { "close" };
        let mut o = Obj::new().s("client_order_id", &self.client_order_id);
        if self.combo {
            let legs = arr(self.legs.iter().map(|l| {
                Obj::new()
                    .raw("instrument", &instrument(l))
                    .n("ratio", l.ratio)
                    .s("side", side(l))
                    .s("effect", effect(l))
                    .done()
            }));
            o = o.s("shape", "combo").raw("legs", &legs);
        } else {
            let l = &self.legs[0];
            o = o
                .s("shape", "single")
                .raw("instrument", &instrument(l))
                .s("side", side(l))
                .s("effect", effect(l));
        }
        o.n("qty", self.qty)
            .s("type", if self.market { "market" } else { "limit" })
            .s(
                if self.market {
                    "ref_price"
                } else {
                    "limit_price"
                },
                &fmt_cents(self.limit_cents),
            )
            .s("time_in_force", if self.on_close { "cls" } else { "day" })
            .done()
    }

    /// Lowercase hex SHA-256 of [`Intent::canonical`].
    #[must_use]
    pub fn sha256(&self) -> String {
        hex(&Sha256::digest(self.canonical().as_bytes()))
    }
}

pub(crate) fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    bytes.iter().fold(String::new(), |mut s, b| {
        let _ = write!(s, "{b:02x}");
        s
    })
}

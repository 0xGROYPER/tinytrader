// An options position's payoff and its prices, for the Spread builder's
// summary and chart (widgets/spreads.js) and Candles' overlay. Pure: the
// quotes, the underlying's price and the volatilities come in as arguments.
//
// What it is worth at expiry needs only each leg's strike, type, quantity and
// what was paid (a held leg's entry price, else its price now). What it is
// worth today needs a price for every leg and the underlying's: a leg without
// one is said, and only the parts that need it are left out, so a contract
// with no bid or ask never blanks the rest. Nothing returned is NaN.

/** A finite number, else null (null, '', NaN and ±Infinity are no number). */
export const num = x => {
  if (x == null || x === '') return null;
  const v = +x;
  return Number.isFinite(v) ? v : null;
};

/**
 * A quote's mid; none when one-sided, crossed or too wide to say what the
 * contract is worth (alpaca-data's rule: apart by more than half the mid and a dime).
 */
export const midOf = q => {
  const bid = num(q?.bid), ask = num(q?.ask);
  if (bid == null || ask == null || !(bid > 0) || !(ask >= bid)) return null;
  const m = (bid + ask) / 2, w = ask - bid;
  return w <= 0.10 || w <= 0.5 * m ? m : null;
};

export const N = x => 0.5 * (1 + erf(x / Math.SQRT2));
function erf(x) {
  const s = x < 0 ? -1 : 1, t = 1 / (1 + 0.3275911 * Math.abs(x));
  return s * (1 - ((((1.061405429 * t - 1.453152027) * t + 1.421413741) * t - 0.284496736) * t + 0.254829592) * t * Math.exp(-x * x));
}
/** Black-Scholes value (zero rates). */
export function bs(type, S, K, t, v) {
  if (t <= 0 || !(v > 0)) return Math.max(type === 'call' ? S - K : K - S, 0);
  const d1 = (Math.log(S / K) + v * v * t / 2) / (v * Math.sqrt(t)), d2 = d1 - v * Math.sqrt(t);
  const call = S * N(d1) - K * N(d2);
  return type === 'call' ? call : call - S + K;
}
/** The volatility that prices a contract at `px` (bisection), or null when no volatility does. */
export function impliedVol(type, S, K, t, px) {
  if (!(S > 0) || !(K > 0) || !(t > 0) || !(px > 0)) return null;
  let lo = 1e-4, hi = 10;
  if (px <= bs(type, S, K, t, lo) || px >= bs(type, S, K, t, hi)) return null;
  for (let i = 0; i < 80; i++) { const m = (lo + hi) / 2; if (bs(type, S, K, t, m) < px) lo = m; else hi = m; }
  return (lo + hi) / 2;
}

/**
 * A leg's price now, by the first of: the quote's mid; else from the
 * volatility of the strikes around it (`ivAt`, with the underlying's price);
 * else its last trade; else the broker's mark for a position held
 * (`mark`, its current price); else none.
 * @param {{ type: 'call' | 'put', strike: number, mark?: number, q?: any }} l
 * @param {{ spot?: number | null, t: number, ivAt?: (k: number) => number | null }} at
 * @returns {{ px: number | null, from: 'mid' | 'model' | 'last' | 'mark' | null }}
 */
export function priceLeg(l, { spot, t, ivAt }) {
  const m = midOf(l.q);
  if (m != null) return { px: m, from: 'mid' };
  const v = num(ivAt?.(l.strike)), s = num(spot);
  if (v > 0 && s > 0) { const px = bs(l.type, s, l.strike, t, v); if (Number.isFinite(px)) return { px, from: 'model' }; }
  const last = num(l.q?.last);
  if (last > 0) return { px: last, from: 'last' };
  const mark = num(l.mark);
  if (mark != null && mark >= 0) return { px: mark, from: 'mark' };
  return { px: null, from: null };
}

/** At expiry, per share, before what was paid. */
const intrinsic = (ls, S) => ls.reduce((s, l) => s + l.side * l.qty * Math.max(l.type === 'call' ? S - l.strike : l.strike - S, 0), 0);

/**
 * A position's payoff and prices.
 * @param {{ legs: { type: 'call' | 'put', strike: number, side: 1 | -1, qty: number, cost?: number, mark?: number, q?: any }[],
 *   spot?: number | null, t: number, ivAt?: (k: number) => number | null, atmIV?: number | null, dist?: any }} o
 */
export function analyse({ legs, spot: spot0, t, ivAt, atmIV, dist }) {
  const spot = num(spot0) > 0 ? num(spot0) : null;
  if (!legs?.length) return null;
  const ls = legs.map(l => {
    const { px, from } = priceLeg(l, { spot, t, ivAt });
    const cost = num(l.cost);
    // Its volatility for today's value: the quote's (when it prices), else the
    // strikes around it, else the one that its last or mark implies.
    const qiv = from === 'mid' ? num(l.q?.iv) : null;
    const v = (qiv > 0 && qiv) || (num(ivAt?.(l.strike)) > 0 ? num(ivAt(l.strike)) : null) || (spot && px != null ? impliedVol(l.type, spot, l.strike, t, px) : null);
    return { ...l, cost, px, from, iv: v };
  });
  // Signed per-share prices: + paid, - received; a held leg at what was paid for it.
  const unpriced = ls.filter(l => l.px == null), unpaid = ls.filter(l => l.cost == null && l.px == null);
  const net = unpaid.length ? null : ls.reduce((s, l) => s + l.side * l.qty * (l.cost ?? l.px), 0);
  const worth = unpriced.length ? null : ls.reduce((s, l) => s + l.side * l.qty * l.px, 0);
  const held = ls.some(l => l.cost != null);
  const cross = ls.reduce((s, l) => s + l.side * l.qty * ((l.side > 0 ? num(l.q?.ask) : num(l.q?.bid)) ?? l.px ?? 0), 0);
  // What it pays at expiry less what it cost (without the cost when that is unknown).
  const paid = net ?? 0;
  const pay = S => intrinsic(ls, S) - paid;
  // Straight between the strikes: the best and worst are at a strike, at
  // zero, or without bound past the highest strike.
  const ks = [...new Set(ls.map(l => l.strike).filter(k => Number.isFinite(k)))].sort((a, b) => a - b);
  const top = ks[ks.length - 1] ?? 0, slopeHi = pay(top + 1) - pay(top);
  const at = [0, ...ks].map(pay);
  const maxP = slopeHi > 1e-9 ? Infinity : Math.max(...at), maxL = slopeHi < -1e-9 ? -Infinity : Math.min(...at);
  const be = [], kinks = [0, ...ks.filter(k => k > 0)];
  for (let i = 1; i < kinks.length; i++) {
    const a = pay(kinks[i - 1]), b = pay(kinks[i]);
    if ((a < 0) !== (b < 0) && b !== a) be.push(kinks[i - 1] + (kinks[i] - kinks[i - 1]) * (-a) / (b - a));
  }
  const last = pay(top);
  if (Math.abs(slopeHi) > 1e-9 && (last < 0) !== (slopeHi < 0) && last !== 0) be.push(top - last / slopeHi);
  // Chance of profit at expiry: from the implied distribution when its fit
  // is usable (linear between its CDF points), else lognormal at the
  // at-the-money implied volatility; none without the underlying's price or the cost.
  const pts = dist && dist.quality !== 'poor' && Array.isArray(dist.cdf) && dist.cdf.length > 2 && dist.cdf.every(p => Number.isFinite(p?.[0]) && Number.isFinite(p?.[1])) ? dist.cdf : null;
  const sd = (num(atmIV) > 0 ? num(atmIV) : 0.3) * Math.sqrt(Math.max(t, 0));
  let pop = null;
  if (net != null && (pts || (spot && sd > 0))) {
    const cdf = pts ? x => {
      if (x <= pts[0][0]) return pts[0][1] * Math.max(0, x / pts[0][0]);
      const end = pts[pts.length - 1];
      if (x >= end[0]) return 1 - (1 - end[1]) * Math.exp(-(x - end[0]) / Math.max(end[0] * 0.05, 1e-9));
      let i = 1; while (pts[i][0] < x) i++;
      const [k0, p0] = pts[i - 1], [k1, p1] = pts[i];
      return k1 > k0 ? p0 + (p1 - p0) * (x - k0) / (k1 - k0) : p1;
    } : x => x <= 0 ? 0 : N((Math.log(x / spot) + sd * sd / 2) / sd);
    const hi = Math.max((spot || 0) * 3, top * 1.5), n = 2400;
    pop = 0;
    for (let i = 1; i <= n; i++) { const x0 = hi * (i - 1) / n, x1 = hi * i / n; if (pay((x0 + x1) / 2) > 0) pop += cdf(x1) - cdf(x0); }
    if (!Number.isFinite(pop)) pop = null;
  }
  // Worth today at an underlying price: each leg at its own implied
  // volatility, less what was paid. None when a leg has no price or there is
  // no underlying price to value it from.
  const todayOk = spot != null && net != null && ls.every(l => l.iv > 0);
  const today = todayOk ? S => ls.reduce((v, l) => v + l.side * l.qty * bs(l.type, S, l.strike, t, l.iv), 0) - net : null;
  return {
    legs: ls, net, worth, held, cross, pay, today, spot, sd, strikes: ks, maxP, maxL, be, pop,
    popFrom: pts ? `implied, ${dist.quality} fit` : 'lognormal',
    // What isn't priced from a quote's mid, by why.
    unpriced, unpaid, missing: ls.filter(l => !l.q && l.from === 'model'),
    wide: ls.filter(l => l.q && l.cost == null && l.from !== 'mid'),
    stale: ls.filter(l => l.from === 'last' || l.from === 'mark'),
  };
}

/** The lowest and highest finite value across these series, with zero. */
export function extent(...series) {
  let lo = 0, hi = 0;
  for (const s of series) for (const v of s || []) if (Number.isFinite(v)) { if (v < lo) lo = v; if (v > hi) hi = v; }
  return [lo, hi];
}
/** An SVG path through the finite points only, broken where a point isn't. */
export function linePath(xs, ys, X, Y) {
  let d = '', pen = false;
  for (let i = 0; i < xs.length; i++) {
    const x = X(xs[i]), y = ys ? Y(ys[i]) : NaN;
    if (!Number.isFinite(x) || !Number.isFinite(y)) { pen = false; continue; }
    d += `${pen ? 'L' : 'M'}${x.toFixed(1)},${y.toFixed(1)}`;
    pen = true;
  }
  return d;
}

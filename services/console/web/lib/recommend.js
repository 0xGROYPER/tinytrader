// A suggested options position for a stock picked in the screener, from its
// option chain and the reason it was listed. Volatility priced rich against
// recent moves (or a high-IV view) suggests selling premium: an iron condor
// with its short strikes at the 16th and 84th percentiles of the expiry's
// implied distribution (about one standard deviation each side) and its
// wings at the 4th and 96th. Volatility priced cheap suggests buying an
// at-the-money straddle. A steep put skew suggests selling the rich side as
// a credit spread. The expiry is the one nearest 30 days out, between 14 and
// 60, with a usable implied distribution where there is one; a report before
// it is pointed out. Strikes sold have a bid; strikes bought, an ask. It's a
// starting point to adjust, not advice.

/**
 * @typedef {{ type: 'call' | 'put', strike: number, side: 1 | -1, qty: number }} Leg
 * @typedef {{ expiry: string, label: string, legs: Leg[], why: string }} Pick
 */

const DAY = 86400e3;
const daysTo = (/** @type {string} */ d, /** @type {number} */ now) => (Date.parse(`${d}T20:00:00Z`) - now) / DAY;

/** The price at cumulative probability p, from the distribution's CDF points, or lognormal. */
function quantile(e, spot, now, p) {
  const pts = e.distribution && e.distribution.quality !== 'poor' && Array.isArray(e.distribution.cdf) ? e.distribution.cdf : null;
  if (pts && pts.length > 2 && p >= pts[0][1] && p <= pts[pts.length - 1][1]) {
    let i = 1; while (i < pts.length - 1 && pts[i][1] < p) i++;
    const [k0, p0] = pts[i - 1], [k1, p1] = pts[i];
    return p1 === p0 ? k0 : k0 + (k1 - k0) * (p - p0) / (p1 - p0);
  }
  const iv = e.atm_iv, t = Math.max(daysTo(e.expiry, now), 0.5) / 365;
  if (!(iv > 0)) return null;
  const sd = iv * Math.sqrt(t), z = inverseNormal(p);
  return (e.distribution?.forward || spot) * Math.exp(-sd * sd / 2 + z * sd);
}

// Acklam's rational approximation.
function inverseNormal(p) {
  const a = [-39.69683028665376, 220.9460984245205, -275.9285104469687, 138.357751867269, -30.66479806614716, 2.506628277459239];
  const b = [-54.47609879822406, 161.5858368580409, -155.6989798598866, 66.80131188771972, -13.28068155288572];
  const c = [-0.007784894002430293, -0.3223964580411365, -2.400758277161838, -2.549732539343734, 4.374664141464968, 2.938163982698783];
  const d = [0.007784695709041462, 0.3224671290700398, 2.445134137142996, 3.754408661907416];
  const lo = 0.02425, q = p < lo ? Math.sqrt(-2 * Math.log(p)) : p > 1 - lo ? Math.sqrt(-2 * Math.log(1 - p)) : 0;
  if (p < lo) return (((((c[0] * q + c[1]) * q + c[2]) * q + c[3]) * q + c[4]) * q + c[5]) / ((((d[0] * q + d[1]) * q + d[2]) * q + d[3]) * q + 1);
  if (p > 1 - lo) return -(((((c[0] * q + c[1]) * q + c[2]) * q + c[3]) * q + c[4]) * q + c[5]) / ((((d[0] * q + d[1]) * q + d[2]) * q + d[3]) * q + 1);
  const r = (p - 0.5) ** 2, x = p - 0.5;
  return (((((a[0] * r + a[1]) * r + a[2]) * r + a[3]) * r + a[4]) * r + a[5]) * x / (((((b[0] * r + b[1]) * r + b[2]) * r + b[3]) * r + b[4]) * r + 1);
}

/**
 * @param {{ chain: any, view?: string, row?: any, earnings?: string | null, now?: number }} ask
 * @returns {Pick | { none: string }}
 */
export function recommend({ chain, view = '', row = null, earnings = null, now = Date.now() }) {
  const spot = chain?.spot;
  if (!spot) return { none: 'no price for the underlying' };
  const quoted = c => c.bid > 0 && c.ask >= c.bid;
  const exps = (chain.expiries || []).map(e => ({ e, d: daysTo(e.expiry, now) }))
    .filter(({ e, d }) => d >= 14 && d <= 60 && (chain.contracts || []).some(c => c.expiry === e.expiry && quoted(c)));
  if (!exps.length) return { none: 'no expiry 14 to 60 days out with quotes' };
  const good = exps.filter(x => x.e.distribution && x.e.distribution.quality !== 'poor');
  const { e, d } = (good.length ? good : exps).reduce((b, x) => Math.abs(x.d - 30) < Math.abs(b.d - 30) ? x : b);
  const of = (type, ok) => (chain.contracts || []).filter(c => c.expiry === e.expiry && c.type === type && ok(c)).map(c => c.strike).sort((a, b) => a - b);
  // Strikes to sell (a bid) and to buy (an ask).
  const calls = of('call', quoted), puts = of('put', quoted);
  const offered = c => c.ask > 0, buyCalls = of('call', offered), buyPuts = of('put', offered);
  const near = (ks, x) => ks.reduce((b, k) => Math.abs(k - x) < Math.abs(b - x) ? k : b, ks[0]);
  // A listed strike near `x`, and a wing strictly beyond `from` (up for calls, down for puts).
  const at = (type, p) => { const x = quantile(e, spot, now, p), ks = type === 'call' ? calls : puts; return x == null || !ks.length ? null : near(ks, x); };
  const wing = (type, p, from) => {
    const ks = type === 'call' ? buyCalls : buyPuts, x = quantile(e, spot, now, p);
    const beyond = ks.filter(k => type === 'call' ? k > from : k < from);
    if (!beyond.length) return null;
    return x == null ? (type === 'call' ? beyond[0] : beyond[beyond.length - 1]) : near(beyond, x);
  };

  const ivrv = row?.iv_rv, skew = row?.skew, iv = row?.atm_iv_30d ?? e.atm_iv;
  const pct = x => `${(x * 100).toFixed(0)}%`;
  const reasons = [];
  let mode = 'condor';
  if (view === 'Put skew' && skew != null && Math.abs(skew) >= 0.02) {
    mode = skew > 0 ? 'put credit' : 'call credit';
    reasons.push(`${skew > 0 ? 'Puts' : 'Calls'} are priced ${(Math.abs(skew) * 100).toFixed(1)} vol points richer than the other side at 25 delta, so sell that side.`);
  } else if (ivrv != null && ivrv <= 0.85) {
    mode = 'straddle';
    reasons.push(`Implied volatility${iv ? ` ${pct(iv)}` : ''} is ${ivrv.toFixed(2)}× the 20-day realized${row?.rv_20d ? ` (${pct(row.rv_20d)})` : ''}: options look cheap against recent moves, so buy them.`);
  } else if (ivrv != null && ivrv >= 1.15) {
    reasons.push(`Implied volatility${iv ? ` ${pct(iv)}` : ''} is ${ivrv.toFixed(2)}× the 20-day realized${row?.rv_20d ? ` (${pct(row.rv_20d)})` : ''}: options look rich against recent moves, so sell premium.`);
  } else if (['Highest IV', 'Expected move', 'Expected $ move'].includes(view)) {
    reasons.push(`Listed for ${view.toLowerCase()}: rich premium to sell, if the stock stays in range.`);
  } else {
    reasons.push('No strong volatility signal; a neutral range trade to start from.');
  }

  /** @type {Leg[]} */
  let legs = [], label = '';
  if (mode === 'straddle') {
    const k = near(calls.filter(x => puts.includes(x)), e.distribution?.forward || spot);
    if (k == null) return { none: 'no strike quoted both ways' };
    legs = [{ type: 'call', strike: k, side: 1, qty: 1 }, { type: 'put', strike: k, side: 1, qty: 1 }];
    label = 'Long straddle';
    reasons.push(`At the money (${k}): profits on a move bigger than the premium either way.`);
  } else if (mode === 'put credit') {
    const s = at('put', 0.25), l = s == null ? null : wing('put', 0.08, s);
    if (s == null || l == null) return { none: 'not enough quoted puts' };
    legs = [{ type: 'put', strike: s, side: -1, qty: 1 }, { type: 'put', strike: l, side: 1, qty: 1 }];
    label = 'Put credit spread';
    reasons.push(`Short ${s} put near the 25th percentile of the implied distribution, long ${l} near the 8th.`);
  } else if (mode === 'call credit') {
    const s = at('call', 0.75), l = s == null ? null : wing('call', 0.92, s);
    if (s == null || l == null) return { none: 'not enough quoted calls' };
    legs = [{ type: 'call', strike: s, side: -1, qty: 1 }, { type: 'call', strike: l, side: 1, qty: 1 }];
    label = 'Call credit spread';
    reasons.push(`Short ${s} call near the 75th percentile of the implied distribution, long ${l} near the 92nd.`);
  } else {
    const sp = at('put', 0.16), sc = at('call', 0.84);
    const lp = sp == null ? null : wing('put', 0.04, sp), lc = sc == null ? null : wing('call', 0.96, sc);
    if ([sp, sc, lp, lc].includes(null) || sp >= sc) return { none: 'not enough quoted strikes for a condor' };
    legs = [{ type: 'call', strike: sc, side: -1, qty: 1 }, { type: 'put', strike: sp, side: -1, qty: 1 },
      { type: 'call', strike: lc, side: 1, qty: 1 }, { type: 'put', strike: lp, side: 1, qty: 1 }];
    label = 'Iron condor';
    reasons.push(`Shorts ${sp} / ${sc} at about one standard deviation (16th / 84th percentiles of the implied distribution), wings ${lp} / ${lc} near the 4th / 96th.`);
  }
  reasons.push(`${Math.round(d)} days to ${e.expiry}.`);
  if (earnings && earnings >= new Date(now).toISOString().slice(0, 10) && earnings <= e.expiry) {
    reasons.push(`It spans the earnings report on ${earnings}${mode === 'straddle' ? '' : ': the move there can run past the short strikes'}.`);
  }
  return { expiry: e.expiry, label, legs, why: reasons.join(' ') };
}

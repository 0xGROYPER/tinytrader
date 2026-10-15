// Trend: daily closes for any mix of symbols (stocks and ETFs, crypto pairs
// such as BTC/USD, and Cboe's indexes: VIX and its terms, Treasury yields),
// from a service serving `daily` (alpaca-data), kept here and shared by the
// widgets that read them (Cross-asset, Observations); and what they say:
// changes over a day, week and month, the 20-, 50- and 200-day averages and
// which side of each the price is on (and since when), 20-day and 52-week
// highs and lows, the streak of up or down days, and how unusual the day's
// move is against the last 20.

import './prefs.js'; // settings shared between devices, read first

/** @typedef {{ s: string, label?: string }} Row */
/** @typedef {{ name: string, rows: Row[] }} Group */

/** Names for the symbols on the board, and any others worth naming. */
export const NAMES = {
  IRX: '13-week yield', FVX: '5-year yield', TNX: '10-year yield', TYX: '30-year yield',
  TLT: '20+ year Treasuries', IEF: '7–10 year Treasuries', SHY: '1–3 year Treasuries', HYG: 'High yield', LQD: 'Investment grade', TIP: 'TIPS',
  UUP: 'US dollar', FXE: 'Euro', FXY: 'Yen', FXB: 'Pound', FXF: 'Swiss franc', CYB: 'Yuan',
  USO: 'Crude oil', BNO: 'Brent', UNG: 'Natural gas', GLD: 'Gold', SLV: 'Silver', CPER: 'Copper', DBA: 'Agriculture', URA: 'Uranium',
  VIX: 'VIX', VIX9D: 'VIX 9-day', VIX3M: 'VIX 3-month', VIX1D: 'VIX 1-day',
  EWJ: 'Japan', FXI: 'China large caps', KWEB: 'China internet', EWG: 'Germany', EWU: 'UK', EEM: 'Emerging markets', INDA: 'India', EWZ: 'Brazil', EWY: 'Korea', EWT: 'Taiwan',
  'BTC/USD': 'Bitcoin', 'ETH/USD': 'Ether', 'SOL/USD': 'Solana',
  SPY: 'S&P 500', QQQ: 'Nasdaq 100', IWM: 'Russell 2000', RSP: 'S&P 500 equal weight', DIA: 'Dow', SMH: 'Semiconductors', XLE: 'Energy', XLF: 'Financials', XLK: 'Technology',
};

/** The board to start from. @type {Group[]} */
export const BOARD = [
  { name: 'Rates', rows: ['IRX', 'FVX', 'TNX', 'TYX', 'TLT', 'HYG', 'LQD'].map(s => ({ s })) },
  { name: 'Dollar and FX', rows: ['UUP', 'FXE', 'FXY'].map(s => ({ s })) },
  { name: 'Commodities', rows: ['USO', 'UNG', 'GLD', 'SLV', 'CPER'].map(s => ({ s })) },
  { name: 'Volatility', rows: ['VIX9D', 'VIX', 'VIX3M'].map(s => ({ s })) },
  { name: 'World', rows: ['EWJ', 'FXI', 'EWG', 'EWU', 'EEM', 'INDA'].map(s => ({ s })) },
  { name: 'Crypto', rows: ['BTC/USD', 'ETH/USD', 'SOL/USD'].map(s => ({ s })) },
  { name: 'US', rows: ['SPY', 'QQQ', 'IWM', 'RSP'].map(s => ({ s })) },
];

const KEY = 'tinytrader-board';
/** The board as edited in this browser, else the one to start from. @returns {Group[]} */
export function board() {
  try { const v = JSON.parse(localStorage.getItem(KEY) || 'null'); if (Array.isArray(v) && v.length) return v; } catch (_) {}
  return BOARD;
}
/** @param {Group[] | null} groups null: back to the one to start from */
export function setBoard(groups) {
  try { if (groups) localStorage.setItem(KEY, JSON.stringify(groups)); else localStorage.removeItem(KEY); } catch (_) {}
  document.dispatchEvent(new CustomEvent('tt-board'));
}

// Yields (Cboe's ten-times-the-percentage indexes) move in basis points.
export const YIELDS = new Set(['IRX', 'FVX', 'TNX', 'TYX']);
export const isYield = s => YIELDS.has(s);
export const isCrypto = s => s.includes('/');
export const label = s => NAMES[s] || s;

/** Closes by symbol, as fetched: { closes: [t, c][], at, error, failed }. */
const kept = new Map();
const STALE = 10 * 60e3;
/** How soon a symbol that failed upstream (not one with no bars) is asked for again. */
export const RETRY = 30e3;
const old = x => Date.now() - x.at > (x.failed ? RETRY : STALE);
/** @type {Map<string, Promise<void>>} */
const asking = new Map();
const service = fleet => (fleet.topo?.services || []).find(s => s.daily)?.id || null;

/**
 * Daily closes for the symbols, fetching those missing or ten minutes old,
 * or 30 seconds old if they failed (one request for them all); the rest from
 * what's kept. `wait` is how soon to ask again: RETRY if any failed.
 * @param {any} fleet @param {string[]} symbols
 * @returns {Promise<{ series: Map<string, any>, error: string, wait: number }>}
 */
export async function closes(fleet, symbols) {
  const svc = service(fleet);
  // Before the topology has loaded there's no telling yet: asked again in a second.
  if (!svc) return fleet.topo ? { series: new Map(), error: 'No service serves daily closes (alpaca-data).', wait: STALE } : { series: new Map(), error: '', wait: 1000 };
  const want = [...new Set(symbols)].filter(s => !kept.has(s) || old(kept.get(s)));
  let error = '';
  for (let i = 0; i < want.length; i += 200) {
    const part = want.slice(i, i + 200), key = part.join(',');
    if (!asking.has(key)) {
      asking.set(key, (async () => {
        const r = await fetch(`/api/stream/${encodeURIComponent(svc)}/daily?symbols=${encodeURIComponent(key)}&days=400`, { cache: 'no-store' });
        const v = await r.json().catch(() => ({}));
        if (!r.ok) throw new Error(v.error || `${r.status}`);
        // An error other than "no bars" is the upstream failing (an outage, a limit): it's retried soon.
        for (const [s, x] of Object.entries(v.series || {})) {
          const error = x.error || '';
          kept.set(s, { closes: x.closes || [], error, failed: !!error && error !== 'no bars', at: Date.now() });
        }
      })().finally(() => asking.delete(key)));
    }
    try { await asking.get(key); } catch (e) { error = `Daily closes couldn't be read: ${e.message}`; }
  }
  /** @type {Map<string, any>} */
  const series = new Map();
  for (const s of symbols) if (kept.has(s)) series.set(s, kept.get(s));
  const wait = error || [...series.values()].some(x => x.failed) ? RETRY : STALE;
  return { series, error, wait };
}

const mean = xs => xs.reduce((a, b) => a + b, 0) / xs.length;
const day = t => new Date(t * 1000).toLocaleDateString('en-CA', { timeZone: 'America/New_York' });

/**
 * What a symbol's closes say, or null with too few.
 * @param {string} s @param {[number, number][]} rows oldest first
 */
export function analyse(s, rows) {
  if (!rows || rows.length < 3) return null;
  const c = rows.map(r => r[1]), t = rows.map(r => r[0]), n = c.length, last = c[n - 1];
  const crypto = isCrypto(s), y = isYield(s);
  // A change: percent, or basis points for a yield (its index is ten times the percentage).
  const change = (a, b) => b == null ? null : y ? (a - b) * 10 : a / b - 1;
  const back = k => n > k ? c[n - 1 - k] : null;
  const [wk, mo, yr] = crypto ? [7, 30, 365] : [5, 21, 252];
  const smaAt = (k, i) => i + 1 >= k ? mean(c.slice(i + 1 - k, i + 1)) : null;
  // Each average, its side, and when the price was last on this side before the run that's on now.
  const avg = {};
  for (const k of [20, 50, 200]) {
    const v = smaAt(k, n - 1);
    if (v == null) { avg[k] = null; continue; }
    const side = last >= v;
    let i = n - 1;
    while (i >= k - 1 && (c[i] >= smaAt(k, i)) === side) i--;
    const run = n - 1 - i; // sessions on this side, today included
    let j = i;
    while (j >= k - 1 && (c[j] >= smaAt(k, j)) !== side) j--;
    avg[k] = { v, above: side, run, crossed: run === 1 && i >= k - 1, since: j >= k - 1 ? day(t[j]) : null };
  }
  const window = k => c.slice(Math.max(0, n - k));
  const hi = k => Math.max(...window(k)), lo = k => Math.min(...window(k));
  const prior = k => c.slice(Math.max(0, n - 1 - k), n - 1);
  const newHi = k => n > k && last > Math.max(...prior(k)), newLo = k => n > k && last < Math.min(...prior(k));
  let streak = 0;
  for (let i = n - 1; i > 0 && Math.sign(c[i] - c[i - 1]) === Math.sign(c[n - 1] - c[n - 2]) && c[i] !== c[i - 1]; i--) streak++;
  streak *= Math.sign(c[n - 1] - c[n - 2]) || 0;
  // The day's move against the last 20: returns, or point changes for a yield.
  const moves = []; for (let i = Math.max(1, n - 21); i < n - 1; i++) moves.push(y ? c[i] - c[i - 1] : Math.log(c[i] / c[i - 1]));
  const sd = moves.length > 5 ? Math.sqrt(mean(moves.map(m => m * m)) - mean(moves) ** 2) : null;
  const today = y ? c[n - 1] - c[n - 2] : Math.log(c[n - 1] / c[n - 2]);
  return {
    s, last, value: y ? last / 10 : last, date: day(t[n - 1]), yield: y, crypto,
    d1: change(last, back(1)), w1: change(last, back(wk)), m1: change(last, back(mo)),
    avg, hi20: hi(20), lo20: lo(20), hi52: hi(yr), lo52: lo(yr),
    newHi20: newHi(20), newLo20: newLo(20), newHi52: n > yr / 2 && newHi(Math.min(yr, n - 1)), newLo52: n > yr / 2 && newLo(Math.min(yr, n - 1)),
    streak, streakChange: streak ? change(last, back(Math.abs(streak))) : null,
    z: sd ? today / sd : null,
  };
}

/** A change as text: +1.23%, or +6bp for a yield. */
export function fmtChange(v, y) {
  if (v == null) return '—';
  // Signed as shown: a change that rounds to nothing has no sign.
  const t = y ? Math.abs(v).toFixed(0) : Math.abs(v * 100).toFixed(2), zero = +t === 0;
  return `${zero ? '' : v > 0 ? '+' : '−'}${t}${y ? 'bp' : '%'}`;
}
/** A level: 4.25% for a yield, else a price. */
export const fmtLevel = a => a.yield ? `${a.value.toFixed(2)}%` : a.last >= 1000 ? a.last.toLocaleString(undefined, { maximumFractionDigits: 0 }) : a.last.toFixed(2);

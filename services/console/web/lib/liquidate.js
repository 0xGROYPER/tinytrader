// Liquidate: option positions in an account closed at once, as few orders
// as the broker allows. Every position is offered with a box to tick (all
// ticked to start); the ticked ones are grouped by underlying, and each
// group is one combo order of up to four legs, its legs reversed, ratios in
// lowest terms. A bigger group is split into runs of four by expiry, type
// and strike, so its spreads close together and no order leaves a short leg
// uncovered on its own. A pick that keeps a short while closing a long that
// may cover it is named (uncovered), and sending waits for an explicit yes.
// Each is priced at the marketable limit (the natural
// and 5%, at least $0.05, past it), or sent at market, from the chain's
// quotes read just before sending. Stock positions and contracts already
// expired are left out, and said so.
import { fleet } from './fleet.js';
import { get, stream } from './cache.js';
import { notSentText } from './orders.js';
import { ourPrefix } from './ticket.js';

const OCC = /^([A-Z]{1,6})(\d{2})(\d{2})(\d{2})([CP])(\d{8})$/;
const r2 = x => Math.round(x * 100) / 100;
const band = n => Math.max(0.05, r2(Math.abs(n) * 0.05));
const gcd = (a, b) => { while (b) [a, b] = [b, a % b]; return a; };
const ny = () => {
  const d = new Date();
  return { date: d.toLocaleDateString('en-CA', { timeZone: 'America/New_York' }), hhmm: +d.toLocaleTimeString('en-GB', { timeZone: 'America/New_York', hour: '2-digit', minute: '2-digit' }).replace(':', '') };
};
// A contract can't be traded after its last day's close (a quarter past for index options).
const expired = expiry => { const n = ny(); return expiry < n.date || (expiry === n.date && n.hhmm >= 1615); };

/**
 * @typedef {{ symbol: string, side: 'buy' | 'sell', ratio: number }} Leg
 * @typedef {{ title: string, root: string, legs: Leg[], qty: number }} Close
 * @typedef {{ symbol: string, root: string, expiry: string, type: string, strike: number, qty: number }} Held
 */

/** The option positions that can be closed, and what's left out (with why). */
export function closable(positions) {
  /** @type {Held[]} */
  const held = [];
  const left = [];
  for (const p of positions || []) {
    const q = +p.qty, m = OCC.exec(p.symbol || '');
    if (!q) continue;
    if (!m) { left.push({ symbol: p.symbol, why: 'not an option (Flatten, or the broker, closes it)' }); continue; }
    const expiry = `20${m[2]}-${m[3]}-${m[4]}`;
    if (expired(expiry)) { left.push({ symbol: p.symbol, why: 'expired: it settles on its own' }); continue; }
    held.push({ symbol: p.symbol, root: m[1], expiry, type: m[5], strike: +m[6] / 1000, qty: q });
  }
  held.sort((a, b) => a.root.localeCompare(b.root) || a.expiry.localeCompare(b.expiry) || a.type.localeCompare(b.type) || a.strike - b.strike);
  return { held, left };
}

const day = e => `${+e.slice(5, 7)}/${+e.slice(8)}`;

/**
 * The closing orders for an account's positions (only those whose symbols
 * are in `pick`, when given), and what they leave out.
 * @param {any[]} positions @param {Set<string>} [pick]
 */
export function plan(positions, pick) {
  const { held, left } = closable(positions);
  /** @type {Map<string, Held[]>} */
  const groups = new Map();
  for (const h of held) {
    if (pick && !pick.has(h.symbol)) continue;
    if (!groups.has(h.root)) groups.set(h.root, []);
    groups.get(h.root).push(h);
  }
  /** @type {Close[]} */
  const orders = [];
  for (const [root, hs] of groups) {
    for (let i = 0; i < hs.length; i += 4) {
      const run = hs.slice(i, i + 4), unit = run.reduce((x, l) => gcd(x, Math.abs(l.qty)), 0) || 1;
      orders.push({
        title: `${root} ${[...new Set(run.map(l => day(l.expiry)))].join(', ')}`, root, qty: unit,
        legs: run.map(l => ({ symbol: l.symbol, side: l.qty > 0 ? 'sell' : 'buy', ratio: Math.abs(l.qty) / unit })),
      });
    }
  }
  return { orders, left };
}

/**
 * The shorts a pick would leave uncovered: kept short contracts of an
 * underlying and type (call or put) when the pick closes a long of the same
 * underlying and type that could be covering them. Closing everything, or
 * closing a short and keeping its long, leaves nothing uncovered.
 * @param {Held[]} held @param {Set<string>} pick @returns {Held[]}
 */
export function uncovered(held, pick) {
  const key = h => `${h.root}|${h.type}`;
  const closingLong = new Set(held.filter(h => h.qty > 0 && pick.has(h.symbol)).map(key));
  return held.filter(h => h.qty < 0 && !pick.has(h.symbol) && closingLong.has(key(h)));
}

/** Quotes for these contracts by symbol, one chain read per underlying and expiry. */
export async function quotes(symbols) {
  const svc = (fleet.topo?.services || []).find(s => s.chain)?.id;
  const out = new Map();
  if (!svc) return out;
  const want = new Map();
  for (const sym of symbols) {
    const m = OCC.exec(sym || '');
    if (m) { const e = `20${m[2]}-${m[3]}-${m[4]}`; want.set(`${m[1]}|${e}`, [m[1], e]); }
  }
  await Promise.all([...want.values()].map(async ([root, expiry]) => {
    try {
      const v = await get(stream(svc, 'chain', { underlying: root, from: expiry, to: expiry }), { fresh: 1500 });
      for (const c of v.contracts || []) if (c.symbol) out.set(c.symbol, c);
    } catch (_) { /* priced as unknown */ }
  }));
  return out;
}

/**
 * What an order would send: its natural (net per unit, + pays) and the price
 * field, or null without a quote on every leg.
 * @param {Close} o @param {Map<string, any>} q @param {boolean} market
 */
export function priced(o, q, market) {
  let natural = 0;
  for (const l of o.legs) {
    const c = q.get(l.symbol), px = l.side === 'buy' ? c?.ask : c?.bid;
    if (px == null || (l.side === 'buy' && !px)) return null;
    natural += (l.side === 'buy' ? 1 : -1) * l.ratio * px;
  }
  natural = r2(natural);
  const single = o.legs.length === 1;
  // A single leg's price is positive; a combo's signed (+ pays, − receives).
  const sent = n => single ? Math.max(0.01, r2(Math.abs(n))) : (n === 0 ? 0.01 : r2(n));
  if (market) return { natural, type: 'market', price: sent(natural) };
  let n = r2(natural + band(natural));
  if (single && o.legs[0].side === 'sell') n = Math.min(n, -0.01);
  return { natural, type: 'limit', price: sent(n) };
}

/** Send one closing order as the console's trade client; its outcome in a few words, and whether it's working. */
export async function send(account, live, o, p) {
  const order = { legs: o.legs, qty: o.qty, effect: 'close', type: p.type, [p.type === 'market' ? 'ref_price' : 'limit_price']: p.price };
  try {
    const r = await fetch(`/api/orders/${encodeURIComponent(account)}`, { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ confirm: account, live, order }) });
    const v = await r.json().catch(() => ({}));
    if (!r.ok || v.error) return { ok: false, text: `not sent: ${v.error || r.status}` };
    if (v.outcome === 'accepted') return { ok: true, text: `sent (${v.status || 'new'})` };
    if (v.outcome === 'rejected') return { ok: false, text: `rejected: ${v.message || v.code || ''}` };
    if (v.outcome === 'not_sent') return { ok: false, text: `not sent: ${notSentText(v.reason || '', v.detail)}` };
    if (v.outcome === 'unknown') return { ok: false, text: 'unknown: it may have reached the broker; check the open orders before sending again' };
    return { ok: false, text: String(v.outcome || 'no answer') };
  } catch (e) { return { ok: false, text: `not known whether it was sent (${e.message}); check the open orders` }; }
}

/** Cancel the console's own open orders on an account (ids tt-): how many were asked to cancel. */
export async function cancelOurs(account, orders) {
  const ours = (orders || []).filter(o => (o.client_order_id || '').startsWith(ourPrefix()));
  await Promise.all(ours.map(o => fetch(`/api/orders/${encodeURIComponent(account)}/${encodeURIComponent(o.client_order_id)}/cancel`, { method: 'POST', headers: { 'content-type': 'application/json' }, body: '{}' }).catch(() => null)));
  return ours.length;
}

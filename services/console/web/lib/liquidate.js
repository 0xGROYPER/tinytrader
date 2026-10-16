// Liquidate: every option position in an account closed at once, as few
// orders as the broker allows. Positions are grouped as Close… groups them
// (underlying and expiry) and each group is one combo order of up to four
// legs, its legs reversed, ratios in lowest terms; a bigger group is split
// into runs of four by type and strike, so its spreads close together and no
// order leaves a short leg uncovered on its own. Each is priced at the
// marketable limit (the natural and 5%, at least $0.05, past it), or sent at
// market, from the chain's quotes read just before sending. Stock positions
// and contracts already expired are left out, and said so. Each order is
// checked against a fresh read of the account just before it is sent
// (reread, fitted), so a leg already closed, or covered by a working order,
// is left out.
import { fleet } from './fleet.js';
import { get, stream } from './cache.js';
import { notSentText } from './orders.js';

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
 * @typedef {{ title: string, root: string, expiry: string, legs: Leg[], qty: number }} Close
 */

/** The closing orders for an account's positions, and what they leave out. */
export function plan(positions) {
  /** @type {Map<string, { root: string, expiry: string, held: { symbol: string, type: string, strike: number, qty: number }[] }>} */
  const groups = new Map();
  const left = [];
  for (const p of positions || []) {
    const q = +p.qty, m = OCC.exec(p.symbol || '');
    if (!q) continue;
    if (!m) { left.push({ symbol: p.symbol, why: 'not an option (Flatten, or the broker, closes it)' }); continue; }
    const expiry = `20${m[2]}-${m[3]}-${m[4]}`;
    if (expired(expiry)) { left.push({ symbol: p.symbol, why: 'expired: it settles on its own' }); continue; }
    const key = `${m[1]}|${expiry}`;
    if (!groups.has(key)) groups.set(key, { root: m[1], expiry, held: [] });
    groups.get(key).held.push({ symbol: p.symbol, type: m[5], strike: +m[6] / 1000, qty: q });
  }
  /** @type {Close[]} */
  const orders = [];
  for (const g of groups.values()) {
    const held = g.held.sort((a, b) => a.type.localeCompare(b.type) || a.strike - b.strike);
    for (let i = 0; i < held.length; i += 4) {
      const run = held.slice(i, i + 4), unit = run.reduce((x, l) => gcd(x, Math.abs(l.qty)), 0) || 1;
      orders.push({
        title: `${g.root} ${+g.expiry.slice(5, 7)}/${+g.expiry.slice(8)}`, root: g.root, expiry: g.expiry, qty: unit,
        legs: run.map(l => ({ symbol: l.symbol, side: l.qty > 0 ? 'sell' : 'buy', ratio: Math.abs(l.qty) / unit })),
      });
    }
  }
  return { orders, left };
}

/** Quotes for the orders' contracts by symbol, one chain read per underlying and expiry. */
export async function quotes(orders) {
  const svc = (fleet.topo?.services || []).find(s => s.chain)?.id;
  const out = new Map();
  if (!svc) return out;
  const want = new Map(orders.map(o => [`${o.root}|${o.expiry}`, o]));
  await Promise.all([...want.values()].map(async o => {
    try {
      const v = await get(stream(svc, 'chain', { underlying: o.root, from: o.expiry, to: o.expiry }), { fresh: 1500 });
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

const UNSURE = 'unknown: it may have reached the broker; check the open orders before sending again';

/**
 * Send one closing order as the console's trade client: its outcome in a few
 * words, whether it's working, and `maybe` when it may have reached the broker
 * (the account then waits for a re-read, see holdOf). `id` is the order's client
 * id when the answer names it. Only accepted, rejected and not_sent are known.
 */
export async function send(account, live, o, p) {
  const order = { legs: o.legs, qty: o.qty, effect: 'close', type: p.type, [p.type === 'market' ? 'ref_price' : 'limit_price']: p.price };
  try {
    const r = await fetch(`/api/orders/${encodeURIComponent(account)}`, { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ confirm: account, live, order }) });
    const v = await r.json().catch(() => ({}));
    const id = v.client_order_id || v.current?.client_order_id;
    if (v.outcome === 'rejected') return { ok: false, text: `rejected: ${v.message || v.code || ''}`, id };
    if (v.outcome === 'not_sent') return { ok: false, text: `not sent: ${notSentText(v.reason || '', v.detail)}`, id };
    if (!r.ok || v.error) return { ok: false, text: `${UNSURE} (the console answered ${v.error || r.status})`, maybe: true, id };
    if (v.outcome === 'accepted') return { ok: true, text: `sent (${v.status || 'new'})`, id };
    if (v.outcome === 'unknown') return { ok: false, text: UNSURE, maybe: true, id };
    if (v.outcome === 'duplicate') return { ok: false, text: 'duplicate: this order was already sent; check the open orders before sending again', maybe: true, id };
    return { ok: false, text: `${v.outcome || 'no answer'}: ${UNSURE}`, maybe: true, id };
  } catch (e) { return { ok: false, text: `not known whether it was sent (${e.message}); check the open orders`, maybe: true }; }
}

/** Cancel the console's own open orders on an account (ids tt-): how many were asked to cancel. */
export async function cancelOurs(account, orders) {
  const ours = (orders || []).filter(o => (o.client_order_id || '').startsWith('tt-'));
  await Promise.all(ours.map(o => fetch(`/api/orders/${encodeURIComponent(account)}/${encodeURIComponent(o.client_order_id)}/cancel`, { method: 'POST', headers: { 'content-type': 'application/json' }, body: '{}' }).catch(() => null)));
  return ours.length;
}

/**
 * The account as the account service has it now, read the way the widget
 * refreshes (GET /api/state): its positions and open orders, or null when either
 * can't be read. Never the snapshot the page already holds.
 */
export async function reread(account) {
  try {
    const r = await fetch('/api/state', { cache: 'no-store' });
    if (!r.ok) return null;
    const v = await r.json();
    for (const s of Object.values(v.services || {})) {
      const a = (s?.accounts?.accounts || []).find(x => x?.name === account);
      if (a) return Array.isArray(a.positions) && Array.isArray(a.orders) ? a : null;
    }
    return null;
  } catch (_) { return null; }
}

/**
 * `o` as the fresh account `a` allows: its units cut to what each leg still
 * holds on the side it closes, so no leg can flip. No order when a leg is flat
 * or covered by a working tt- order; `left` gives each such leg's reason.
 * @param {Close} o @param {{ positions: any[], orders: any[] }} a
 * @returns {{ order: Close | null, left: { symbol: string, why: string }[] }}
 */
export function fitted(o, a) {
  const working = new Set(a.orders.filter(x => (x.client_order_id || '').startsWith('tt-')).flatMap(x => (x.legs?.length ? x.legs : [x]).map(l => l.symbol)));
  const covered = o.legs.filter(l => working.has(l.symbol));
  if (covered.length) return { order: null, left: covered.map(l => ({ symbol: l.symbol, why: 'covered by a working order' })) };
  // A sell closes a long, a buy closes a short: the contracts held on that side, in this order's ratio.
  const fits = o.legs.map(l => {
    const q = +(a.positions.find(p => p.symbol === l.symbol)?.qty || 0);
    return Math.floor((l.side === 'sell' ? Math.max(0, q) : Math.max(0, -q)) / l.ratio);
  });
  const flat = o.legs.filter((l, i) => !fits[i]);
  if (flat.length) return { order: null, left: flat.map(l => ({ symbol: l.symbol, why: 'already flat' })) };
  const units = Math.min(o.qty, ...fits);
  const short = o.legs.filter((l, i) => fits[i] < o.qty);
  return { order: units < o.qty ? { ...o, qty: units } : o, left: short.map(l => ({ symbol: l.symbol, why: `smaller than planned: ${units} of ${o.qty} sent` })) };
}

// An account whose last run had an order that may be live (see send's maybe)
// is held: no run is sent to it until its open orders are read again after the
// run, and each such order is in them or has had 30 s to show. Kept in the
// browser, so a reload does not clear it; cleared only by that re-read.
const HELD = 'tt-liq-held', SETTLE_MS = 30e3;
let held = {};
try { const v = JSON.parse(localStorage.getItem(HELD) || '{}'); if (v && typeof v === 'object') held = v; } catch (_) { /* kept in memory only */ }
const keep = () => { try { localStorage.setItem(HELD, JSON.stringify(held)); } catch (_) { /* kept in memory only */ } };

/** Hold `account` after a run: `ids` are the maybe-live orders' client ids (null where none was named), `base` the account's updated_at when the run ended. */
export function hold(account, base, ids) {
  if (!ids.length) return;
  held[account] = { at: Date.now(), base: base ?? null, ids };
  keep();
}

/** Why `account` may not be sent to yet, or '' when it may; `a` is the account as the snapshot has it now. */
export function holdOf(account, a) {
  const h = held[account];
  if (!h) return '';
  // Re-read: the account service's updated_at moved past the run's, with open orders in that read.
  const read = Array.isArray(a?.orders) && typeof a.updated_at === 'number' && (h.base == null || a.updated_at > h.base);
  if (!read) return 'The account has not been read since then.';
  const open = new Set(a.orders.map(o => o.client_order_id));
  const settled = h.ids.every(id => (id && open.has(id)) || Date.now() - h.at >= SETTLE_MS);
  if (!settled) return 'The unknown order is not in the open orders yet; it stays held for 30 seconds after the run.';
  delete held[account];
  keep();
  return '';
}

// The order ticket: one screen for an options order, opening or closing, from
// the Spread builder (Trade) or an account's positions (Close…). The legs come
// from where it was opened; here are the account, the quantity and the price,
// and Send. Market sends a market order (the gateway checks its caps against
// the natural, sent as its reference price); Limit starts at the marketable
// price, the natural and a little more (5%, at least $0.05) so it fills like a
// market order unless the quotes jump, and follows the quotes until it's
// changed. Each leg's bid and ask come from the chain service every few
// seconds. A live account is never picked for you, and its Send asks once
// more before it sends.
import { fleet, svcState } from './fleet.js';
import { get, stream } from './cache.js';
import { esc } from './format.js';
import { notSentText } from './orders.js';
import { orderFetch } from './api.js';

const OCC = /^([A-Z]{1,6})(\d{2})(\d{2})(\d{2})([CP])(\d{8})$/;
const r2 = x => Math.round(x * 100) / 100;
const usd = v => `$${Math.abs(v).toFixed(2)}`;
/** SPY261015C00765000 as SPY 765C 10/15. */
export const occLabel = s => { const m = OCC.exec(s || ''); return m ? `${m[1]} ${+m[6] / 1000}${m[5]} ${+m[3]}/${+m[4]}` : s; };
/** The protection past the natural for the marketable limit: 5%, at least a nickel. */
const band = n => Math.max(0.05, r2(Math.abs(n) * 0.05));

/** Order routing as the console has it: {trading, accounts: [{account, environment, error, state}]}, or {unavailable}. */
let routing = /** @type {any} */ (null), routingAt = 0, asking = /** @type {Promise<void> | null} */ (null);
/** Whether an account can be traded from the console. @param {string} name */
export const tradable = name => !!name && !!routing?.trading && (routing.accounts || []).some(x => x.account === name && !x.error);
/** How many of an account's orders the gateway says have not resolved (0 when it doesn't say, or isn't a number). @param {string} name */
export const unresolvedOf = name => {
  const n = name ? (routing?.accounts || []).find(x => x.account === name)?.unresolved : 0;
  return typeof n === 'number' && n > 0 ? n : 0;
};
/** Routing, read if it's 15 seconds old; then `done` (every caller waiting on the read). */
export function routed(done) {
  if (asking) { asking.then(done); return; }
  if (Date.now() - routingAt < 15000) return;
  routingAt = Date.now();
  asking = (async () => {
    try {
      const r = await fetch('/api/orders', { cache: 'no-store' });
      routing = r.ok ? await r.json() : { unavailable: r.status === 404 ? 'this console has no order routing yet (it needs redeploying).' : `the console answered ${r.status}.` };
    } catch (e) { routing = { unavailable: `the console can't be reached (${e.message}).` }; }
  })().finally(() => { asking = null; });
  asking.then(done);
}

/** An account's open orders, as its account service has them (null if unknown, undefined if not read). */
function openOrdersOf(name) {
  for (const s of (fleet.topo?.services || []).filter(x => x.accounts)) for (const a of svcState(s.id).accounts?.accounts || []) if (a.name === name) return a.orders;
  return undefined;
}

function outcome(v, ok) {
  if (!ok || v.error) return { ok: false, again: true, html: `<span class="sb-bad">Not sent: ${esc(v.error || 'error')}.</span>` };
  const id = esc(v.client_order_id || v.current?.client_order_id || '');
  if (v.outcome === 'accepted') return { ok: true, again: false, html: `<span class="sb-good">Accepted by the broker (${esc(v.status || 'new')}), ${id}. Not a fill yet: it's with the account's open orders until it fills or is cancelled.</span>` };
  if (v.outcome === 'rejected') return { ok: false, again: true, html: `<span class="sb-bad">Rejected by the broker: ${esc(v.message || v.code || '')}.</span>` };
  if (v.outcome === 'not_sent') return { ok: false, again: true, html: `<span class="sb-bad">Not sent: ${esc(notSentText(v.reason || '', v.detail))}.</span>` };
  if (v.outcome === 'unknown') return { ok: false, again: false, html: `<span class="sb-bad">Unknown (${esc(v.reason || '')}): it may have reached the broker. Don't send it again; the gateway is checking, and it will show with the account's open orders if it did.</span>` };
  return { ok: false, again: false, html: esc(JSON.stringify(v).slice(0, 200)) };
}

/**
 * A ticket drawn into `box`. `open(order)` shows one; the widget calls
 * `update()` when it redraws, and `dispose()` when it goes.
 * @param {HTMLElement} box
 * @param {{ onSent?: (v: any) => void, onAccount?: (name: string) => void }} [hooks]
 */
export function createTicket(box, hooks = {}) {
  /** @type {null | { title: string, acct: string, from: string, legs: { symbol: string, side: 'buy' | 'sell', ratio: number }[], qty: number, effect: 'open' | 'close', market: boolean, limit: number | null, quotes: Map<string, any>, qErr: string, confirm: number, sending: boolean, result: string, done: boolean, risk?: (net: number, qty: number) => string }} */
  let t = null, timer = 0;

  // Bid and ask for every leg, one chain read per underlying and expiry.
  async function readQuotes() {
    const me = t; if (!me) return;
    const svc = (fleet.topo?.services || []).find(s => s.chain)?.id;
    if (!svc) { me.qErr = 'No service serves option chains (alpaca-data), so there are no quotes.'; draw(); return; }
    const want = new Map();
    for (const l of me.legs) { const m = OCC.exec(l.symbol); if (m) { const e = `20${m[2]}-${m[3]}-${m[4]}`; want.set(`${m[1]}|${e}`, [m[1], e]); } }
    const errs = [];
    await Promise.all([...want.values()].map(async ([root, expiry]) => {
      try {
        // The same read as the Spread builder's for that expiry: shared.
        const v = await get(stream(svc, 'chain', { underlying: root, from: expiry, to: expiry }), { fresh: 1500 });
        for (const c of v.contracts || []) if (c.symbol) me.quotes.set(c.symbol, c);
      } catch (e) { errs.push(`${root} ${expiry}: ${e.message}`); }
    }));
    if (t !== me) return;
    me.qErr = errs.length ? `Quotes couldn't be read (${errs.join('; ')}).` : '';
    draw();
  }
  const poll = () => { clearTimeout(timer); if (t) timer = setTimeout(() => { readQuotes().finally(poll); }, 4000); };

  /** Per leg, and the net mid and natural per unit (signed: + pays), or null where a quote is missing. */
  function pricing() {
    const legs = t.legs.map(l => { const q = t.quotes.get(l.symbol), bid = q?.bid ?? null, ask = q?.ask || null; return { ...l, bid, ask, mid: bid != null && ask ? (bid + ask) / 2 : null }; });
    const sum = f => legs.every(l => f(l) != null) ? r2(legs.reduce((s, l) => s + (l.side === 'buy' ? 1 : -1) * l.ratio * f(l), 0)) : null;
    return { legs, mid: sum(l => l.mid), natural: sum(l => l.side === 'buy' ? l.ask : l.bid) };
  }
  const single = () => t.legs.length === 1;
  // A net per unit as sent: a single leg's is its positive price, a combo's signed.
  const asSent = net => single() ? r2(Math.abs(net)) : r2(net);
  const asNet = sent => single() && t.legs[0].side === 'sell' ? -sent : sent;
  /** The marketable limit: the natural and the band, toward paying (net). */
  const marketable = p => {
    if (p.natural == null) return null;
    let n = r2(p.natural + band(p.natural));
    if (single() && t.legs[0].side === 'sell') n = Math.min(n, -0.01); // a sell's price stays at least a cent
    return n === 0 ? 0.01 : n;
  };
  /** The limit as a net, as shown and sent. */
  const limitNet = p => t.limit ?? marketable(p);

  function view() {
    if (!t) return '';
    const p = pricing(), accts = routing?.accounts || [], isLive = x => x?.environment === 'live';
    if (routing && !routing.unavailable && !accts.some(x => x.account === t.acct && !x.error)) t.acct = accts.find(x => !x.error && !isLive(x))?.account || '';
    const live = isLive(accts.find(x => x.account === t.acct));
    // Nothing picked shows as such, never as the first account listed.
    const pick = accts.length ? `<select class="tk-acct" aria-label="Account">${t.acct ? '' : '<option value="" selected disabled>Pick an account</option>'}${accts.map(x => `<option value="${esc(x.account)}"${x.error ? ' disabled' : ''}${x.account === t.acct ? ' selected' : ''}>${esc(x.account)}${isLive(x) ? ' · LIVE' : ''}${x.error ? ' (unreachable)' : x.state && x.state !== 'ready' ? ` (${esc(x.state)})` : ''}</option>`).join('')}</select>` : '';
    const head = `<div class="tk-head"><b>${esc(t.title)}</b>${pick}${live ? '<span class="tk-live-tag">LIVE · real money</span>' : ''}<button type="button" class="tile-btn tk-x" data-tk="close" aria-label="Close the ticket" title="Close">×</button></div>`;
    const result = t.result ? `<p class="tk-res">${t.result}</p>` : '';
    const say = text => `<div class="tk">${head}<p class="muted tk-note">${text}</p>${result}</div>`;
    if (!routing) return say('Loading the accounts…');
    if (routing.unavailable) return say(`Orders: ${esc(routing.unavailable)}`);
    if (!routing.trading) return say('Sending orders needs the console\'s trade token (operator: scripts/gateway-token GATEWAY_CONSOLE_TOKEN, then restart the gateways and the console).');
    const q = x => x == null ? '—' : x.toFixed(2);
    const legs = `<div class="tk-legs mono">${p.legs.map(l => `<span class="tk-leg"><b class="${l.side === 'buy' ? 'pos' : 'neg'}">${l.side === 'buy' ? '+' : '−'}${l.ratio * t.qty}</b> ${esc(occLabel(l.symbol))} <small>${q(l.bid)}×${q(l.ask)}</small></span>`).join('')}</div>`;
    const net = limitNet(p), shown = net == null ? '' : asSent(net).toFixed(2);
    const priceRow = `<div class="tk-row">
      <label class="tk-qty">Qty <input class="tk-q mono" type="number" min="1" max="100" value="${t.qty}"></label>
      <span class="tk-seg" role="group" aria-label="Order type"><button type="button" data-tk-type="market" aria-pressed="${t.market}">Market</button><button type="button" data-tk-type="limit" aria-pressed="${!t.market}">Limit</button></span>
      ${t.market ? '' : `<span class="tk-px"><button type="button" class="tile-btn" data-tk-step="-1" aria-label="A cent less" title="A cent less (shift: a nickel)">−</button><input class="tk-limit mono" type="number" step="0.01" value="${shown}" aria-label="Limit" title="${single() ? 'The price per contract' : 'Net per spread: positive pays (debit), negative receives (credit)'}"><button type="button" class="tile-btn" data-tk-step="1" aria-label="A cent more" title="A cent more (shift: a nickel)">+</button></span>
      ${t.limit != null ? '<button type="button" class="tk-link" data-tk="reset" title="Back to the marketable price, following the quotes">marketable</button>' : ''}${p.mid != null ? `<button type="button" class="tk-link" data-tk="mid" title="Limit at the mid">mid ${asSent(p.mid).toFixed(2)}</button>` : ''}`}
    </div>`;
    // What it does, in a line.
    const verb = t.effect === 'close' ? 'Closing' : 'Opening', what = single() ? 'order' : 'combo order';
    let sum = '';
    if (t.market) sum = p.natural == null ? 'Market needs a quote on every leg.' : `${verb} ${what} at market: fills at what the market gives. About ${p.natural < 0 ? 'receiving' : 'paying'} $${(Math.abs(p.natural) * 100 * t.qty).toFixed(0)} at the quotes now (natural ${q(asSent(p.natural))}).`;
    else if (net != null) {
      const total = Math.abs(net) * 100 * t.qty, toNat = p.natural == null ? null : r2(net - p.natural);
      sum = `${verb} ${what}, limit, day: ${net < 0 ? `receive at least $${total.toFixed(0)}` : `pay at most $${total.toFixed(0)}`}${toNat == null ? '' : toNat >= 0 ? ' · marketable' : ` · ${usd(toNat)} short of the natural ${q(asSent(p.natural))}: it waits`}.`;
    } else sum = t.qErr ? esc(t.qErr) : 'Waiting for quotes…';
    const risk = t.risk && !t.market && net != null ? ` ${esc(t.risk(net, t.qty))}` : '';
    // What's already working on the account, however it was placed.
    const mine = openOrdersOf(t.acct), desc = x => x.legs?.length ? x.legs.map(l => `${l.side === 'sell' ? '−' : '+'}${(l.ratio_qty || 1) * (x.qty || 1)} ${occLabel(l.symbol)}`).join(' ') : `${x.side === 'sell' ? '−' : '+'}${x.qty} ${occLabel(x.symbol)}`;
    const warn = mine === null ? `<p class="sb-bad tk-warn">${esc(t.acct)}'s open orders couldn't be read: check the broker first.</p>`
      : mine?.length ? `<p class="sb-bad tk-warn" title="${esc(mine.map(x => `${desc(x)} @ ${x.limit_price ?? x.type} ${x.status || ''}`).join('\n'))}">${esc(t.acct)} already has ${mine.length} open order${mine.length > 1 ? 's' : ''}: ${esc(desc(mine[0]))}${mine.length > 1 ? ' …' : ''}</p>` : '';
    const other = t.from && t.acct !== t.from ? `<p class="sb-bad tk-warn">Not the account these legs came from (${esc(t.from)}).</p>` : '';
    const ok = t.acct && (t.market ? p.natural != null : net != null && net !== 0);
    const confirming = t.confirm > Date.now();
    const label = t.sending ? 'Sending…' : confirming ? `Confirm: send to ${t.acct} for real` : live ? 'Send live order' : 'Send order';
    const acts = t.done ? `<button type="button" class="btn" data-tk="close">Done</button>`
      : `<button type="button" class="btn primary${confirming ? ' danger' : ''}" data-tk="send"${!ok || t.sending ? ' disabled' : ''}>${esc(label)}</button>${confirming ? '<button type="button" class="btn" data-tk="unconfirm">Not yet</button>' : ''}`;
    return `<div class="tk${live ? ' tk-live' : ''}">${head}${legs}${priceRow}<p class="tk-sum">${sum}${risk}</p>${other}${warn}<div class="tk-acts">${acts}</div>${result}</div>`;
  }

  // Not while one of its fields has focus: a redraw would take what's being typed.
  function draw(force = false) {
    const f = document.activeElement;
    if (!force && f && box.contains(f) && f.matches('input, select')) return;
    paint(view());
  }
  const paint = html => { if (box.innerHTML !== html) box.innerHTML = html; };

  async function send() {
    if (!t || t.sending || t.done) return;
    const p = pricing(), live = (routing?.accounts || []).find(x => x.account === t.acct)?.environment === 'live';
    // A live order asks once more; the ask lapses after a few seconds.
    if (live && t.confirm <= Date.now()) { t.confirm = Date.now() + 6000; draw(true); setTimeout(() => draw(true), 6100); return; }
    const net = t.market ? p.natural : limitNet(p);
    if (net == null || (!t.market && !net)) { t.result = '<span class="sb-bad">Not sent: no price yet.</span>'; draw(true); return; }
    const order = { legs: t.legs.map(l => ({ symbol: l.symbol, side: l.side, ratio: l.ratio })), qty: t.qty, effect: t.effect,
      ...(t.market ? { type: 'market', ref_price: asSent(net) } : { type: 'limit', limit_price: asSent(net) }) };
    t.sending = true; t.confirm = 0; draw(true);
    const me = t;
    try {
      const r = await orderFetch(`/api/orders/${encodeURIComponent(me.acct)}`, { confirm: me.acct, live, order }, 'POST', {}, live);
      const v = await r.json().catch(() => ({})), o = outcome(v, r.ok);
      me.result = o.html; me.done = !o.again;
      hooks.onSent?.(v);
    } catch (e) { me.result = `<span class="sb-bad">Not known whether it was sent: ${esc(e.message)}. Check the account's open orders before sending again.</span>`; me.done = true; }
    me.sending = false;
    if (t === me) draw(true);
  }

  const click = e => {
    const b = /** @type {HTMLElement} */ (e.target).closest('button'); if (!b || !t) return;
    const d = b.dataset, p = pricing();
    if (d.tk === 'close') { close(); return; }
    if (d.tk === 'send') { send(); return; }
    if (d.tk === 'unconfirm') t.confirm = 0;
    else if (d.tk === 'reset') t.limit = null;
    else if (d.tk === 'mid' && p.mid != null) t.limit = p.mid;
    else if (d.tkType) t.market = d.tkType === 'market';
    else if (d.tkStep) { const n = limitNet(p); if (n != null) t.limit = r2(asNet(r2(asSent(n) + +d.tkStep * (e.shiftKey ? 0.05 : 0.01)))); }
    else return;
    t.confirm = 0; t.result = t.done ? t.result : '';
    draw(true);
  };
  // Fields are kept as typed, and drawn again once they're left (or Enter).
  const keep = (e, redraw) => {
    const f = /** @type {HTMLInputElement} */ (e.target); if (!t || !box.contains(f)) return;
    if (f.classList.contains('tk-q')) t.qty = Math.max(1, Math.min(100, Math.round(+f.value) || 1));
    else if (f.classList.contains('tk-limit')) { const v = r2(+f.value); if (f.value !== '' && isFinite(v) && v) t.limit = asNet(single() ? Math.abs(v) : v); }
    else if (f.classList.contains('tk-acct')) { t.acct = f.value; hooks.onAccount?.(f.value); }
    else return;
    t.confirm = 0;
    if (redraw) { f.blur(); draw(true); }
  };
  const input = e => keep(e, false), change = e => keep(e, true);
  const key = e => { if (e.key === 'Enter' && /** @type {HTMLElement} */ (e.target).matches?.('.tk input')) { e.preventDefault(); /** @type {HTMLElement} */ (e.target).blur(); } };
  box.addEventListener('click', click); box.addEventListener('input', input); box.addEventListener('change', change); box.addEventListener('keydown', key);

  function close() { t = null; clearTimeout(timer); draw(true); }
  return {
    /**
     * Show a ticket for these legs (ratios in lowest terms; `qty` the units).
     * @param {{ title: string, acct: string, legs: { symbol: string, side: 'buy' | 'sell', ratio: number }[], qty: number, effect: 'open' | 'close', risk?: (net: number, qty: number) => string }} o
     */
    open(o) {
      t = { ...o, from: o.acct, market: false, limit: null, quotes: new Map(), qErr: '', confirm: 0, sending: false, result: '', done: false };
      draw(true); routed(() => draw()); readQuotes().finally(poll);
      box.scrollIntoView({ block: 'nearest' });
    },
    close,
    get isOpen() { return !!t; },
    update() { routed(() => draw()); draw(); },
    dispose() { clearTimeout(timer); box.removeEventListener('click', click); box.removeEventListener('input', input); box.removeEventListener('change', change); box.removeEventListener('keydown', key); },
  };
}

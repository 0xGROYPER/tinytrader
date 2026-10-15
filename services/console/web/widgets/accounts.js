// Accounts: a card per brokerage account (live first) with equity, the day's
// change, balances, an equity chart, the largest positions and every open
// order however it was placed (the console's can be cancelled here). A paper
// account with an order gateway also gets Flatten (close every position at
// market, through the gateway, after typing the account's name) and, while
// its gateway halts opens, Resume opening. An account with an order gateway
// gets an order ticket (Close… on each option group, New order on the card):
// up to four option legs as one order, priced from each leg's bid and ask,
// starting at the marketable (natural) price for a close, reviewed and sent
// as the console's trade client (POST /api/orders/{account}). A new order
// is built in the Spread builder: Spread builder on a card opens it with
// that account picked for its Trade review.
//
// Account (one account): the same for one account, picked at its top (the
// live account until another is picked, kept with the panel), with room for
// every position (average price, last price, P/L) and the standing orders.
import { defineWidget, paint } from '../lib/widgets.js';
import { fleet, svcState } from '../lib/fleet.js';
import { esc, duration } from '../lib/format.js';
import { askHeld } from '../lib/position.js';
import { notSentText } from '../lib/orders.js';

/** An account's option positions by underlying and expiry, for the Spread builder's payoff. */
const heldGroups = new Map();
function optionGroups(account, pos) {
  const groups = new Map();
  for (const p of pos) {
    const m = /^([A-Z]{1,6})(\d{2})(\d{2})(\d{2})([CP])(\d{8})$/.exec(p.symbol || '');
    const qty = +p.qty, cost = +p.avg_entry_price;
    if (!m || !qty || !isFinite(cost)) continue;
    const [, root, yy, mm, dd, cp, k] = m, expiry = `20${yy}-${mm}-${dd}`, key = `${account}|${root}|${expiry}`;
    if (!groups.has(key)) groups.set(key, { key, sym: root, expiry, account, legs: [] });
    groups.get(key).legs.push({ type: cp === 'C' ? 'call' : 'put', strike: +k / 1000, side: qty > 0 ? 1 : -1, qty: Math.abs(qty), cost });
  }
  for (const g of groups.values()) heldGroups.set(g.key, g);
  return [...groups.values()];
}
const dayLabel = d => new Date(d + 'T12:00:00Z').toLocaleDateString(undefined, { timeZone: 'UTC', month: 'short', day: 'numeric' });

// Order gateways by account name ({state} or {error}), read every 15 s while shown.
let gateways = {};
let gwAt = 0;
async function readGateways(update) {
  if (Date.now() - gwAt < 15000) return;
  gwAt = Date.now();
  try {
    const r = await fetch('/api/accounts/gateways', { cache: 'no-store' });
    const v = await r.json();
    gateways = Object.fromEntries((v.gateways || []).map(g => [g.account, g]));
  } catch (_) { gateways = {}; }
  update();
}
// Flatten in progress per account: {stage: 'confirm' | 'busy' | 'done', typed, result, error}.
const flat = {};
async function post(path, body) {
  const r = await fetch(path, { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify(body) });
  let v = null; try { v = await r.json(); } catch (_) {}
  if (!r.ok) throw new Error(v?.error || `${r.status} ${r.statusText}`);
  return v;
}

function flattenPanel(a) {
  const name = a?.name, g = name && gateways[name];
  if (a?.environment === 'live' || g?.environment === 'live') return '';
  if (!g) return '';
  if (g.error) return `<p class="muted acct-note" title="${esc(g.error)}">Order gateway unreachable.</p>`;
  const f = flat[name] || {};
  const halted = g.state === 'killed';
  const buttons = `<div class="acct-actions">
    <button type="button" class="btn small danger" data-flat-open="${esc(name)}"${f.stage === 'busy' ? ' disabled' : ''}>Flatten</button>
    ${halted ? `<button type="button" class="btn small" data-flat-resume="${esc(name)}">Resume opening</button><span class="pill p-warn">opens halted</span>` : ''}</div>`;
  if (f.stage === 'confirm') {
    const pos = a.positions || [];
    return `<div class="acct-flat">
      <b>Close every position in ${esc(a.label || name)} at market?</b>
      <p class="muted" style="margin:0;font-size:12px">${pos.length ? pos.map(p => `${esc(p.symbol)} ${esc(String(p.qty))}`).join(', ') : 'No positions now; open orders are cancelled.'}</p>
      <p class="muted" style="margin:0;font-size:12px">Open orders are cancelled first. Options close at whatever the spread is. New opening orders stay refused until Resume opening.</p>
      <label style="font-size:12px">Type <b>${esc(name)}</b> to confirm <input class="acct-flat-input" data-flat-typed="${esc(name)}" value="${esc(f.typed || '')}" autocomplete="off" spellcheck="false"></label>
      <div class="acct-actions"><button type="button" class="btn small danger" data-flat-go="${esc(name)}"${f.typed === name ? '' : ' disabled'}>Flatten now</button>
      <button type="button" class="btn small" data-flat-cancel="${esc(name)}">Cancel</button></div></div>`;
  }
  if (f.stage === 'busy') return `<div class="acct-flat"><b>Flattening ${esc(name)}…</b></div>`;
  if (f.stage === 'done') {
    const rows = (f.result?.positions || []).map(p => `<tr><td style="color:var(--fg)">${esc(p.symbol)}</td><td>${p.status === 200 ? 'closing order placed' : esc(String(p.broker_raw?.message || p.status))}</td></tr>`).join('');
    const head = f.error ? `<b class="neg">Flatten failed: ${esc(f.error)}</b>`
      : f.result?.outcome === 'ok' ? `<b>Flatten sent${rows ? '' : ': nothing to close'}.</b>`
      : `<b class="neg">Flatten ${esc(f.result?.outcome || '')}: ${esc(f.result?.reason || f.result?.message || '')}</b> <span class="muted">Check positions before trying again.</span>`;
    return `<div class="acct-flat">${head}${rows ? `<table class="pos-tbl"><tbody>${rows}</tbody></table>` : ''}
      <div class="acct-actions"><button type="button" class="btn small" data-flat-cancel="${esc(name)}">Close</button></div></div>${buttons}`;
  }
  return buttons;
}

// Open orders, from the account itself: the console's (ids tt-) with Cancel,
// others marked as placed elsewhere (another client, Alpaca's app).
const cancelling = {};
function ordersPanel(a) {
  const orders = a.orders, name = a.name;
  if (orders === undefined) return '';
  if (orders === null) return '<p class="neg" style="margin:0;font-size:12px">Open orders couldn\'t be read: check the broker before sending more.</p>';
  if (!orders.length) return '';
  const age = t => { const s = (Date.now() - Date.parse(t)) / 1000; return isFinite(s) ? duration(s) : ''; };
  const legsOf = o => o.legs?.length
    ? o.legs.map(l => `${l.side === 'sell' ? '−' : '+'}${(l.ratio_qty || 1) * (o.qty || 1)} ${esc(l.symbol)}`).join('<br>')
    : `${o.side === 'sell' ? '−' : '+'}${o.qty ?? '?'} ${esc(o.symbol)}`;
  const rows = orders.map(o => {
    const id = o.client_order_id || '', ours = id.startsWith('tt-'), c = cancelling[`${name}|${id}`];
    return `<tr><td style="color:var(--fg);text-align:left">${legsOf(o)}</td>
      <td>${o.limit_price != null ? (o.limit_price < 0 ? `cr ${(-o.limit_price).toFixed(2)}` : o.limit_price.toFixed(2)) : esc(o.type || '')}</td>
      <td>${o.filled_qty || 0}/${o.qty ?? '?'}</td><td>${esc(o.status || '')}</td><td>${esc(age(o.created_at))}</td>
      <td>${ours ? `<button type="button" class="btn small" data-cancel-order="${esc(name)}|${esc(id)}"${c ? ' disabled' : ''}>${c ? esc(c) : 'Cancel'}</button>` : '<span class="muted" title="Not placed by the console: change or cancel it where it was placed">elsewhere</span>'}</td></tr>`;
  }).join('');
  return `<div class="scroll"><table class="pos-tbl acct-orders"><thead><tr><th>Open order</th><th>Limit</th><th>Filled</th><th>Status</th><th>Age</th><th></th></tr></thead><tbody>${rows}</tbody></table></div>`;
}

// Every service that serves an `account` stream gets a card: live accounts first.
function accountsView(withHeading = true) {
  // Every account of every service that serves an `accounts` stream.
  const accts = (fleet.topo?.services || []).filter(s => s.accounts)
    .flatMap(s => (svcState(s.id).accounts?.accounts || []).map(a => ({ svc: s, a })))
    .sort((x, y) => (x.a?.environment === 'live' ? 0 : 1) - (y.a?.environment === 'live' ? 0 : 1) || (x.a?.label || x.svc.id).localeCompare(y.a?.label || y.svc.id));
  if (!accts.length) return '';
  const grid = `<div class="cards acct-cards">${accts.map(({ svc, a }) => accountCard(svc, a)).join('')}</div>`;
  return withHeading ? `<section><div class="sec-head"><h2>Accounts</h2><span class="muted" style="font-size:13px">Balances and positions</span></div>${grid}</section>` : grid;
}
function money(v, cur) {
  if (v == null || !isFinite(v)) return '—';
  const s = Math.abs(v).toLocaleString(undefined, { minimumFractionDigits: 2, maximumFractionDigits: 2 });
  return (v < 0 ? '−' : '') + (cur === 'USD' || !cur ? '$' : cur + ' ') + s;
}
function signed(v, cur) { return v == null ? '—' : (v > 0 ? '+' : '') + money(v, cur); }
function pct(v) { return v == null || !isFinite(v) ? '—' : (v > 0 ? '+' : v < 0 ? '−' : '') + Math.abs(v * 100).toFixed(2) + '%'; }
const tone = v => v > 0 ? 'pos' : v < 0 ? 'neg' : '';
/** An option's OCC symbol as `SPY 775C 10/16`; anything else as it is. */
function shortSym(sym) {
  const m = /^([A-Z]{1,6})(\d{2})(\d{2})(\d{2})([CP])(\d{8})$/.exec(sym || '');
  return m ? `${m[1]} ${+m[6] / 1000}${m[5]} ${+m[3]}/${+m[4]}` : sym;
}
/** @param {string} [pick] One account's view: its picker, every position, more columns. */
function accountCard(svc, a, pick) {
  const acc = a?.account || {}, cur = acc.currency, env = a?.environment || 'paper';
  const day = acc.equity != null && acc.last_equity != null ? acc.equity - acc.last_equity : null;
  // One line: the account, then its equity and the day's change.
  const head = `<div class="acct-head"><h3>${pick ?? esc(a?.label || svc.label || svc.id)}<span class="env env-${env === 'live' ? 'live' : 'paper'}">${esc(env)}</span>
    ${acc.status && acc.status !== 'ACTIVE' ? `<span class="pill p-warn">${esc(acc.status.toLowerCase())}</span>` : ''}
    ${acc.trading_blocked || acc.account_blocked ? '<span class="pill p-crit">blocked</span>' : ''}</h3>
    ${a?.updated_at ? `<span class="acct-eq"><b>${money(acc.equity, cur)}</b>${day != null ? `<span class="${tone(day)}">${signed(day, cur)} (${pct(acc.last_equity ? day / acc.last_equity : null)})</span>` : ''}</span>` : ''}</div>`;
  if (!a?.updated_at) {
    const err = svcState(svc.id).health?.body?.last_error;
    return `<article class="${pick ? 'acct acct-one' : 'card acct'}">${head}<p class="muted" style="margin:0">${esc(err ? 'Cannot read the account: ' + err : 'Waiting for the first reading…')}</p></article>`;
  }
  const stats = [['Cash', money(acc.cash, cur)], ['Buying power', money(acc.buying_power, cur)]];
  if (acc.long_market_value) stats.push(['Long', money(acc.long_market_value, cur)]);
  if (acc.short_market_value) stats.push(['Short', money(acc.short_market_value, cur)]);
  const pos = a.positions || [];
  const shown = pick ? pos.length : 8;
  const px = v => v == null || v === '' || !isFinite(+v) ? '—' : (+v).toFixed(2);
  const rows = pos.slice().sort((x, y) => Math.abs(y.market_value || 0) - Math.abs(x.market_value || 0)).slice(0, shown).map(p => `<tr>
    <td style="color:var(--fg)" title="${esc(p.symbol)}">${esc(shortSym(p.symbol))}</td><td>${esc(String(p.qty ?? '—'))}</td>${pick ? `<td>${px(p.avg_entry_price)}</td><td>${px(p.current_price)}</td>` : ''}<td>${money(p.market_value, cur)}</td>
    <td class="${tone(p.unrealized_pl)}" title="${pct(p.unrealized_plpc)}">${signed(p.unrealized_pl, cur)}</td>
    <td class="${tone(p.change_today)} acct-today">${pct(p.change_today)}</td></tr>`).join('');
  return `<article class="${pick ? 'acct acct-one' : 'card acct'}">${head}
    <div class="acct-stats">${stats.map(([k, v]) => `<span>${k} <b>${v}</b></span>`).join('')}</div>
    <div class="chart" data-chart="${esc(svc.id)}|recent|${esc(a.label || 'equity')}" data-decimals="2" data-height="${pick ? 90 : 56}"></div>
    ${pos.length ? `<div class="scroll"><table class="pos-tbl"><thead><tr><th>Position</th><th>Qty</th>${pick ? '<th>Avg</th><th>Last</th>' : ''}<th>Value</th><th>P/L</th><th class="acct-today">Today</th></tr></thead><tbody>${rows}</tbody></table></div>` : ''}
    ${(() => { const g = optionGroups(a.name || a.label || svc.id, pos), can = tradable(a.name); return g.length || can ? `<div class="acct-payoffs">${g.map(x => `<button type="button" class="chip-btn" data-held="${esc(x.key)}" title="Load these legs into the Spread builder at their entry prices">Payoff: ${esc(x.sym)} ${esc(dayLabel(x.expiry))} · ${x.legs.length} leg${x.legs.length > 1 ? 's' : ''}</button>${can ? `<button type="button" class="chip-btn" data-ot-close="${esc(x.key)}" title="An order closing these legs, priced to fill (natural)">Close…</button>` : ''}`).join('')}${can ? `<button type="button" class="chip-btn" data-sb-open="${esc(a.name)}" title="Build an order in the Spread builder, with this account picked for it">Spread builder</button>` : ''}</div>` : ''; })()}
    ${ordersPanel(a)}
    <div class="foot">${pos.length > shown ? `${pos.length} positions, largest ${shown} shown · ` : ''}${!pos.length && Array.isArray(a.orders) && !a.orders.length ? 'No positions or open orders · ' : !pos.length ? 'No positions · ' : ''}updated ${duration(Date.now() / 1000 - a.updated_at)} ago</div>
    ${flattenPanel(a)}
  </article>`;
}

// The order ticket: one at a time, above the cards. Up to four option legs
// (OCC symbols, side, ratio) as one order, opening or closing; each leg's bid
// and ask from the chain service, every few seconds while it's open. The net
// is per unit (the quantity multiplies it): + pays (debit), − receives
// (credit); a single leg's limit is its own price. Natural pays the ask on
// buys and takes the bid on sells, so it's the marketable price, and a close
// starts there; an open starts at the mid. Review shows the account (a live
// one never picked for you), every contract and what it can cost, then Send.
/** Order routing as the console has it: {trading, accounts: [{account, environment, error, state}]}, or {unavailable}. */
let routing = /** @type {any} */ (null), routingAt = 0;
async function readRouting(update) {
  if (Date.now() - routingAt < 15000) return;
  routingAt = Date.now();
  try {
    const r = await fetch('/api/orders', { cache: 'no-store' });
    routing = r.ok ? await r.json() : { unavailable: r.status === 404 ? 'this console has no order routing yet (it needs redeploying).' : `the console answered ${r.status}.` };
  } catch (e) { routing = { unavailable: `the console can't be reached (${e.message}).` }; }
  update();
}
const tradable = name => !!name && !!routing?.trading && (routing.accounts || []).some(x => x.account === name && !x.error);
const OCC = /^([A-Z]{1,6})(\d{2})(\d{2})(\d{2})([CP])(\d{8})$/;
const gcd = (a, b) => { while (b) [a, b] = [b, a % b]; return a; };
const r2 = x => Math.round(x * 100) / 100;
/** @type {null | { acct: string, from: string, legs: { symbol: string, side: 'buy' | 'sell', ratio: number }[], qty: number, effect: 'open' | 'close', limit: number | null, quotes: Map<string, any>, qErr: string, qAt: number, review: boolean, typed: string, sending: boolean, result: string, title: string }} */
let tk = null;
const blankLeg = () => ({ symbol: '', side: /** @type {'buy' | 'sell'} */ ('buy'), ratio: 1 });
function openTicket(acct, legs, qty, effect, title) {
  tk = { acct, from: acct, legs, qty, effect, limit: null, quotes: new Map(), qErr: '', qAt: 0, review: false, typed: '', sending: false, result: '', title };
}
// A held group, closed: each leg reversed, ratios in lowest terms, the quantity the rest.
function closeTicket(g) {
  const g0 = g.legs.reduce((x, l) => gcd(x, l.qty), 0) || 1;
  const legs = g.legs.map(l => ({ symbol: occOf(g.sym, g.expiry, l.type, l.strike), side: /** @type {'buy' | 'sell'} */ (l.side > 0 ? 'sell' : 'buy'), ratio: l.qty / g0 }));
  openTicket(g.account, legs, g0, 'close', `Close ${g.sym} ${dayLabel(g.expiry)}`);
}
const occOf = (root, expiry, type, strike) => `${root}${expiry.slice(2, 4)}${expiry.slice(5, 7)}${expiry.slice(8, 10)}${type === 'call' ? 'C' : 'P'}${String(Math.round(strike * 1000)).padStart(8, '0')}`;
// Bid and ask for every leg, one chain read per underlying and expiry.
async function readQuotes(update) {
  if (!tk) return;
  const svc = (fleet.topo?.services || []).find(s => s.chain)?.id;
  if (!svc) { tk.qErr = 'No service serves option chains (alpaca-data), so there are no quotes.'; update(); return; }
  const me = tk, want = new Map();
  for (const l of me.legs) {
    const m = OCC.exec(l.symbol); if (!m) continue;
    const expiry = `20${m[2]}-${m[3]}-${m[4]}`; want.set(`${m[1]}|${expiry}`, [m[1], expiry]);
  }
  me.qAt = Date.now();
  const errs = [];
  await Promise.all([...want.values()].map(async ([root, expiry]) => {
    try {
      const r = await fetch(`/api/stream/${encodeURIComponent(svc)}/chain?underlying=${encodeURIComponent(root)}&from=${expiry}&to=${expiry}`, { cache: 'no-store' });
      const v = await r.json().catch(() => ({}));
      if (!r.ok) throw new Error(v.error || `${r.status}`);
      for (const c of v.contracts || []) if (c.symbol) me.quotes.set(c.symbol, c);
    } catch (e) { errs.push(`${root} ${expiry}: ${e.message}`); }
  }));
  if (tk !== me) return;
  me.qErr = errs.length ? `Quotes couldn't be read (${errs.join('; ')}).` : '';
  update();
}
/** The prices: per leg, and the net mid and natural (per unit, signed: + debit), or null where a quote is missing. */
function pricing(t) {
  const legs = t.legs.map(l => { const q = t.quotes.get(l.symbol); const bid = q?.bid ?? null, ask = q?.ask || null; return { ...l, bid, ask, mid: bid != null && ask ? (bid + ask) / 2 : null }; });
  const sum = f => legs.every(l => f(l) != null) ? legs.reduce((s, l) => s + (l.side === 'buy' ? 1 : -1) * l.ratio * f(l), 0) : null;
  return { legs, mid: sum(l => l.mid), natural: sum(l => l.side === 'buy' ? l.ask : l.bid) };
}
const single = t => t.legs.length === 1;
// The limit as sent: a single leg's is its positive price; else the signed net.
const sentLimit = (t, net) => net == null ? null : single(t) ? r2(Math.abs(net)) : r2(net);
// As a signed net, whatever the shape (a single sell is a credit).
const asNet = (t, limit) => single(t) && t.legs[0].side === 'sell' ? -limit : limit;
function limitOf(t, p) { return t.limit ?? sentLimit(t, t.effect === 'close' ? p.natural : p.mid); }
const legsOk = t => t.legs.length >= 1 && t.legs.length <= 4 && t.legs.every(l => OCC.test(l.symbol) && l.ratio >= 1 && l.ratio <= 10) && new Set(t.legs.map(l => l.symbol)).size === t.legs.length && t.qty >= 1 && t.qty <= 100;
// The account's open orders, as its account service has them (null if unknown, undefined if not read).
function openOrdersOf(name) {
  for (const s of (fleet.topo?.services || []).filter(x => x.accounts)) for (const a of svcState(s.id).accounts?.accounts || []) if (a.name === name) return a.orders;
  return undefined;
}
const usd = v => `$${Math.abs(v).toFixed(2)}`;
function ticketView() {
  const t = tk; if (!t) return '';
  const p = pricing(t), limit = limitOf(t, p), money = x => x == null ? '—' : x.toFixed(2);
  const netNote = x => x == null ? '—' : single(t) ? usd(x) : `${x < 0 ? 'cr ' : ''}${Math.abs(x).toFixed(2)}`;
  const head = `<div class="ot-head"><b>${esc(t.title)}</b><span class="muted">${esc(t.acct)} · ${t.effect === 'close' ? 'closing' : 'opening'}</span><button type="button" class="btn small" data-ot="close" aria-label="Close the ticket">×</button></div>`;
  const result = t.result ? `<p class="ot-result">${t.result}</p>` : '';
  if (!routing) return `<div class="ot">${head}<p class="muted">Loading the accounts…</p></div>`;
  if (routing.unavailable) return `<div class="ot">${head}<p class="muted">Orders: ${esc(routing.unavailable)}</p></div>`;
  if (!routing.trading) return `<div class="ot">${head}<p class="muted">Sending orders needs the console's trade token (operator: scripts/gateway-token GATEWAY_CONSOLE_TOKEN, then restart the gateways and the console).</p></div>`;
  const rows = p.legs.map((l, i) => `<tr data-ot-i="${i}">
      <td>${t.review ? `<span class="mono">${esc(l.symbol)}</span>` : `<input class="ot-sym mono" value="${esc(l.symbol)}" placeholder="SPY261120C00600000" spellcheck="false" autocomplete="off" aria-label="Leg ${i + 1} OCC symbol">`}</td>
      <td>${t.review ? (l.side === 'buy' ? 'Buy' : 'Sell') : `<select class="ot-side" aria-label="Leg ${i + 1} side"><option value="buy"${l.side === 'buy' ? ' selected' : ''}>Buy</option><option value="sell"${l.side === 'sell' ? ' selected' : ''}>Sell</option></select>`}</td>
      <td class="num">${t.review ? `${l.ratio * t.qty}` : `<input class="ot-ratio mono" type="number" min="1" max="10" value="${l.ratio}" aria-label="Leg ${i + 1} ratio">`}</td>
      <td class="num mono">${money(l.bid)} × ${money(l.ask)}</td><td class="num mono">${money(l.mid)}</td>
      <td>${t.review || t.legs.length < 2 ? '' : '<button type="button" class="sb-x" data-ot="remove" aria-label="Remove this leg" title="Remove">×</button>'}</td></tr>`).join('');
  const table = `<div class="scroll"><table class="pos-tbl ot-legs"><thead><tr><th>Contract</th><th>Side</th><th class="num">${t.review ? 'Contracts' : 'Ratio'}</th><th class="num">Bid × ask</th><th class="num">Mid</th><th></th></tr></thead><tbody>${rows}</tbody>
    ${t.review || t.legs.length >= 4 ? '' : '<tfoot><tr><td colspan="6"><button type="button" class="chip-btn" data-ot="add">Add leg</button></td></tr></tfoot>'}</table></div>`;
  // How far the limit is from the natural (marketable at or past it) and the mid.
  let where = '';
  if (limit != null && p.natural != null) {
    const n = asNet(t, limit), off = r2(n - p.natural), fromMid = p.mid == null ? null : r2(n - p.mid);
    where = `${off >= 0 ? '<b class="sb-good">Marketable</b>' : `<b>${usd(off)} short of natural</b>`}${fromMid != null ? ` · ${fromMid === 0 ? 'at the mid' : `${usd(fromMid)} ${fromMid > 0 ? 'past' : 'inside'} the mid, toward ${fromMid > 0 ? 'filling' : 'waiting'}`}` : ''}`;
  }
  const price = `<div class="ot-price">
      <span>Natural <b class="mono">${netNote(p.natural)}</b></span><span>Mid <b class="mono">${netNote(p.mid)}</b></span>
      <label>Limit <input class="ot-limit mono" type="number" step="0.01" value="${limit == null ? '' : limit.toFixed(2)}" title="${single(t) ? 'The price per contract' : 'Net per unit: positive pays (debit), negative receives (credit)'}"></label>
      ${t.review ? '' : `<span class="ot-steps"><button type="button" class="chip-btn" data-ot="natural">Natural</button><button type="button" class="chip-btn" data-ot="mid">Mid</button>${[-0.05, -0.01, 0.01, 0.05].map(d => `<button type="button" class="chip-btn mono" data-ot-step="${d}">${d > 0 ? '+' : '−'}${Math.abs(d).toFixed(2)}</button>`).join('')}</span>`}
      ${t.review ? '' : `<label>Qty <input class="ot-qty mono" type="number" min="1" max="100" value="${t.qty}"></label><label>Effect <select class="ot-effect"><option value="close"${t.effect === 'close' ? ' selected' : ''}>Close</option><option value="open"${t.effect === 'open' ? ' selected' : ''}>Open</option></select></label>`}
    </div><p class="ot-where muted">${where || (t.qErr ? esc(t.qErr) : p.legs.some(l => OCC.test(l.symbol)) ? 'Waiting for quotes…' : 'Type each leg as an OCC symbol.')}</p>`;
  if (!t.review) {
    const ok = legsOk(t) && limit != null && limit !== 0 && !(single(t) && limit < 0);
    return `<div class="ot">${head}${table}${price}<p class="ot-acts"><button type="button" class="btn primary" data-ot="review"${ok ? '' : ' disabled'}>Review</button><button type="button" class="btn" data-ot="close">Cancel</button>${ok ? '' : `<span class="muted">${!legsOk(t) ? 'One to four distinct OCC contracts, ratios 1 to 10, quantity 1 to 100.' : 'Set a limit.'}</span>`}</p>${result}</div>`;
  }
  // The review: the account, what's working on it, every contract, and what it can cost.
  const accts = routing.accounts || [], isLive = x => x?.environment === 'live';
  if (!accts.some(x => x.account === t.acct && !x.error)) t.acct = accts.find(x => !x.error && !isLive(x))?.account || '';
  const live = isLive(accts.find(x => x.account === t.acct));
  const net = asNet(t, limit), credit = net < 0, total = Math.abs(net) * 100 * t.qty;
  const mine = openOrdersOf(t.acct), desc = x => x.legs?.length ? x.legs.map(l => `${l.side === 'sell' ? '−' : '+'}${(l.ratio_qty || 1) * (x.qty || 1)} ${l.symbol}`).join(' ') : `${x.side === 'sell' ? '−' : '+'}${x.qty} ${x.symbol}`;
  const already = mine === null ? `<p class="sb-bad">${esc(t.acct)}'s open orders couldn't be read: check the broker first.</p>`
    : mine?.length ? `<p class="sb-bad"><b>${esc(t.acct)} already has ${mine.length} open order${mine.length > 1 ? 's' : ''}:</b></p><ul class="mono">${mine.map(x => `<li>${esc(desc(x))} @ ${x.limit_price ?? x.type} <span class="muted">${esc(x.status || '')}${(x.client_order_id || '').startsWith('tt-') ? '' : ', placed outside the console'}</span></li>`).join('')}</ul>` : '';
  const other = t.acct !== t.from ? `<p class="sb-bad">Not the account these legs came from (${esc(t.from)}).</p>` : '';
  return `<div class="ot sb-review${live ? ' sb-live' : ''}">${head}${live ? '<p class="sb-bad"><b>LIVE ACCOUNT: real money.</b></p>' : ''}
    <p class="sb-tk"><label>Account <select class="ot-acct">${accts.map(x => `<option value="${esc(x.account)}"${x.error ? ' disabled' : ''}${x.account === t.acct ? ' selected' : ''}>${esc(x.account)}${isLive(x) ? ' (LIVE: real money)' : ''}${x.error ? ' (unreachable)' : x.state && x.state !== 'ready' ? ` (${esc(x.state)})` : ''}</option>`).join('')}</select></label></p>
    ${other}${already}${table}${price}
    <p>${credit ? `Receive at least $${total.toFixed(0)}` : `Pay at most $${total.toFixed(0)}`}${t.qty > 1 ? ` (${t.qty} × ${usd(Math.abs(net))})` : ''}. ${t.legs.length === 1 ? 'One order' : 'One combo order'}, limit, day, ${t.effect === 'close' ? 'closing' : 'opening'}.</p>
    ${live ? `<p><label>Type <b class="mono">${esc(t.acct)}</b> to send: <input class="ot-confirm mono" autocomplete="off" spellcheck="false" value="${esc(t.typed)}"></label></p>` : ''}
    <p class="ot-acts"><button type="button" class="btn primary" data-ot="send"${!t.acct || t.sending || (live && t.typed.trim() !== t.acct) ? ' disabled' : ''}>${t.sending ? 'Sending…' : live ? 'Send live order' : 'Send order'}</button><button type="button" class="btn" data-ot="back"${t.sending ? ' disabled' : ''}>Back</button></p></div>${result}`;
}
function outcome(v, ok) {
  if (!ok || v.error) return `<span class="sb-bad">Not sent: ${esc(v.error || 'error')}.</span>`;
  const id = esc(v.client_order_id || v.current?.client_order_id || '');
  if (v.outcome === 'accepted') return `<span class="sb-good">Accepted by the broker (${esc(v.status || 'new')}), ${id}. Not a fill yet: it's listed with the account's open orders until it fills or is cancelled.</span>`;
  if (v.outcome === 'rejected') return `<span class="sb-bad">Rejected by the broker: ${esc(v.message || v.code || '')}.</span>`;
  if (v.outcome === 'not_sent') return `<span class="sb-bad">Not sent: ${esc(notSentText(v.reason || '', v.detail))}.</span>`;
  if (v.outcome === 'unknown') return `<span class="sb-bad">Unknown (${esc(v.reason || '')}): it may have reached the broker. Don't send it again; the gateway is checking and it will show with the account's open orders if it did.</span>`;
  return esc(JSON.stringify(v).slice(0, 200));
}
async function sendTicket(update) {
  const t = tk; if (!t || t.sending) return;
  const live = (routing?.accounts || []).find(x => x.account === t.acct)?.environment === 'live';
  if (live && t.typed.trim() !== t.acct) { t.result = `<span class="sb-bad">Type ${esc(t.acct)} to send a live order.</span>`; update(true); return; }
  const limit = limitOf(t, pricing(t));
  if (!legsOk(t) || limit == null || !limit) { t.result = '<span class="sb-bad">Not sent: the legs or the limit aren\'t complete.</span>'; t.review = false; update(true); return; }
  t.sending = true; update(true);
  try {
    const r = await fetch(`/api/orders/${encodeURIComponent(t.acct)}`, { method: 'POST', headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ confirm: t.acct, live, order: { legs: t.legs.map(l => ({ symbol: l.symbol, side: l.side, ratio: l.ratio })), qty: t.qty, limit_price: limit, effect: t.effect } }) });
    const v = await r.json().catch(() => ({}));
    t.result = outcome(v, r.ok);
  } catch (e) { t.result = `<span class="sb-bad">Not known whether it was sent: ${esc(e.message)}. Check the account's open orders before sending again.</span>`; }
  t.sending = false; t.review = false; t.typed = '';
  update(true);
}

const draw = () => accountsView(false) || '<p class="muted">No account service is running.</p>';

/** Every account of every service serving them: [{ svc, a }], live first. */
const allAccounts = () => (fleet.topo?.services || []).filter(s => s.accounts)
  .flatMap(s => (svcState(s.id).accounts?.accounts || []).map(a => ({ svc: s, a })))
  .sort((x, y) => (x.a?.environment === 'live' ? 0 : 1) - (y.a?.environment === 'live' ? 0 : 1));
/** One account: `want` (a name), else the live one, else the first. */
function drawOne(want) {
  const list = allAccounts();
  const one = list.find(x => (x.a?.name || x.a?.label) === want) || list.find(x => x.a?.environment === 'live') || list[0];
  if (!one) return '<p class="muted">No account service is running.</p>';
  const name = x => x.a?.name || x.a?.label || x.svc.id;
  const pick = `<select class="acct-pick" aria-label="Account">${list.map(x => `<option value="${esc(name(x))}"${x === one ? ' selected' : ''}>${esc(x.a?.label || name(x))}</option>`).join('')}</select>`;
  return accountCard(one.svc, one.a, pick);
}

// Redraws on every update, keeping the scroll position, but not while the
// operator is typing a confirmation. The ticket has its own box, redrawn
// with its quotes, and not while one of its fields has focus.
function mount(el, single = false) {
  // One account's view: which, kept with the panel.
  let want = '';
  if (single) { try { want = JSON.parse(el.dataset.args || '{}').account || ''; } catch (_) {} }
  el.innerHTML = '<div class="ot-box"></div><div class="acct-main"></div>';
  const box = /** @type {HTMLElement} */ (el.querySelector('.ot-box')), main = /** @type {HTMLElement} */ (el.querySelector('.acct-main'));
  let qTimer = 0;
  const ticket = (force = false) => {
    const f = document.activeElement;
    if (!force && f && box.contains(f) && f.matches('input, select')) return;
    paint(box, ticketView());
    clearTimeout(qTimer);
    if (tk) qTimer = setTimeout(() => readQuotes(ticket), 5000);
  };
  const update = (force = false) => {
    if (!(el.contains(document.activeElement) && document.activeElement.matches('.acct-flat-input, .acct-pick'))) paint(main, single ? drawOne(want) : draw());
    ticket(force === true);
    readGateways(update); readRouting(update);
  };
  const show = () => { ticket(true); readQuotes(ticket); box.scrollIntoView({ block: 'nearest' }); };
  const click = async e => {
    const t = e.target.closest('button'); if (!t) return;
    const d = t.dataset;
    if (d.sbOpen) {
      // The Spread builder, with this account picked for its Trade review.
      try { localStorage.setItem('tt-sb-account', d.sbOpen); } catch (_) {}
      document.dispatchEvent(new CustomEvent('tt-sb-account', { detail: d.sbOpen }));
      document.dispatchEvent(new CustomEvent('tt-show-view', { detail: 'spreads' }));
      return;
    }
    if (d.otClose) {
      const g = heldGroups.get(d.otClose); if (!g) return;
      if (g.legs.length > 4) { openTicket(g.account, [], 1, 'close', `Close ${g.sym} ${dayLabel(g.expiry)}`); tk.result = '<span class="sb-bad">More than four legs: close them as two orders (New order).</span>'; ticket(true); return; }
      closeTicket(g); show(); return;
    }
    if (d.otStep && tk) { const l = limitOf(tk, pricing(tk)); if (l != null) tk.limit = r2(l + +d.otStep); ticket(true); return; }
    if (d.ot && tk) {
      const i = +(/** @type {HTMLElement | null} */ (t.closest('[data-ot-i]'))?.dataset.otI ?? -1), p = pricing(tk);
      if (d.ot === 'close') { tk = null; clearTimeout(qTimer); ticket(true); return; }
      if (d.ot === 'natural') tk.limit = sentLimit(tk, p.natural);
      else if (d.ot === 'mid') tk.limit = sentLimit(tk, p.mid);
      else if (d.ot === 'add' && tk.legs.length < 4) tk.legs.push(blankLeg());
      else if (d.ot === 'remove' && i >= 0) { tk.legs.splice(i, 1); tk.limit = null; }
      else if (d.ot === 'review') { tk.review = true; tk.result = ''; tk.limit = limitOf(tk, p); }
      else if (d.ot === 'back') tk.review = false;
      else if (d.ot === 'send') { sendTicket(ticket); return; }
      ticket(true); return;
    }
    if (d.held) {
      const g = heldGroups.get(d.held); if (!g) return;
      askHeld(g); document.dispatchEvent(new CustomEvent('tt-show-view', { detail: 'spreads' }));
      return;
    }
    if (d.cancelOrder) {
      const [name, id] = d.cancelOrder.split('|');
      cancelling[d.cancelOrder] = 'Cancelling…'; update();
      try { const v = await post(`/api/orders/${encodeURIComponent(name)}/${encodeURIComponent(id)}/cancel`, {}); cancelling[d.cancelOrder] = v.outcome === 'accepted' ? 'Cancel sent' : `Cancel ${v.outcome || 'failed'}`; }
      catch (err) { cancelling[d.cancelOrder] = 'Cancel failed'; console.warn(err); }
      update(); return;
    }
    if (d.flatOpen) { flat[d.flatOpen] = { stage: 'confirm', typed: '' }; update(); el.querySelector(`[data-flat-typed="${CSS.escape(d.flatOpen)}"]`)?.focus(); }
    else if (d.flatCancel) { delete flat[d.flatCancel]; update(); }
    else if (d.flatGo) {
      const name = d.flatGo;
      flat[name] = { stage: 'busy' }; update();
      try { flat[name] = { stage: 'done', result: await post(`/api/accounts/${encodeURIComponent(name)}/flatten`, { confirm: name, reason: 'flatten from the console' }) }; }
      catch (err) { flat[name] = { stage: 'done', error: err.message }; }
      gwAt = 0; update();
    } else if (d.flatResume) {
      try { await post(`/api/accounts/${encodeURIComponent(d.flatResume)}/resume`, { reason: 'resumed from the console' }); }
      catch (err) { flat[d.flatResume] = { stage: 'done', error: err.message }; }
      gwAt = 0; update();
    }
  };
  const input = e => {
    if (tk && e.target.classList?.contains('ot-confirm')) {
      tk.typed = e.target.value;
      const send = /** @type {HTMLButtonElement | null} */ (box.querySelector('[data-ot="send"]'));
      if (send) send.disabled = tk.sending || tk.typed.trim() !== tk.acct;
      return;
    }
    if (tk && box.contains(e.target)) { keep(e, false); return; }
    const name = e.target.dataset?.flatTyped; if (!name || !flat[name]) return;
    flat[name].typed = e.target.value;
    const go = el.querySelector(`[data-flat-go="${CSS.escape(name)}"]`);
    if (go) go.disabled = e.target.value !== name;
  };
  // The ticket's fields are kept as typed and redrawn a moment after they
  // change (Enter or leaving them), so a click on a button isn't lost to the redraw.
  const keep = (e, redraw) => {
    const f = e.target; if (!tk || !box.contains(f)) return;
    const i = +(f.closest('[data-ot-i]')?.dataset.otI ?? -1), leg = tk.legs[i];
    if (f.classList.contains('ot-sym') && leg) { leg.symbol = f.value.trim().toUpperCase().replace(/\s+/g, ''); tk.limit = null; }
    else if (f.classList.contains('ot-side') && leg) { leg.side = f.value === 'sell' ? 'sell' : 'buy'; tk.limit = null; }
    else if (f.classList.contains('ot-ratio') && leg) leg.ratio = Math.max(1, Math.min(10, Math.round(+f.value) || 1));
    else if (f.classList.contains('ot-qty')) tk.qty = Math.max(1, Math.min(100, Math.round(+f.value) || 1));
    else if (f.classList.contains('ot-effect')) { tk.effect = f.value === 'open' ? 'open' : 'close'; tk.limit = null; }
    else if (f.classList.contains('ot-limit')) { const v = r2(+f.value); if (f.value === '') tk.limit = null; else if (isFinite(v) && v) tk.limit = v; }
    else if (f.classList.contains('ot-acct')) { tk.acct = f.value; tk.typed = ''; }
    else return;
    if (redraw) setTimeout(() => ticket(true), 250);
  };
  const change = e => {
    if (e.target.classList?.contains('acct-pick')) {
      want = e.target.value; e.target.blur();
      el.dispatchEvent(new CustomEvent('tt-tab-args', { bubbles: true, detail: { args: { account: want } } }));
      update(); return;
    }
    keep(e, true); if (tk && e.target.classList?.contains('ot-sym')) readQuotes(ticket); };
  const key = e => { if (e.key === 'Enter' && e.target.matches?.('.ot input')) { e.preventDefault(); e.target.blur(); } };
  el.addEventListener('click', click); el.addEventListener('input', input); el.addEventListener('change', change); el.addEventListener('keydown', key);
  update();
  return { update, dispose: () => { clearTimeout(qTimer); el.removeEventListener('click', click); el.removeEventListener('input', input); el.removeEventListener('change', change); el.removeEventListener('keydown', key); } };
}

defineWidget('accounts', { title: 'Accounts and equity', mount: el => mount(el) });
defineWidget('account', { title: 'Account', mount: el => mount(el, true) });

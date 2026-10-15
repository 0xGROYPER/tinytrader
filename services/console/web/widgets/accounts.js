// Accounts: a card per brokerage account (live first) with equity, the day's
// change, balances, an equity chart, the largest positions and every open
// order however it was placed (the console's can be cancelled here). A paper
// account with an order gateway also gets Flatten (close every position at
// market, through the gateway, after typing the account's name) and, while
// its gateway halts opens, Resume opening. An account with an order gateway
// gets Close… on each option group: the order ticket (lib/ticket.js), at
// market or a limit, sent as the console's trade client (POST
// /api/orders/{account}). A new order is built in the Spread builder:
// Spread builder on a card opens it with that account picked for its ticket.
//
// Account (one account): the same for one account, picked at its top (the
// live account until another is picked, kept with the panel), with room for
// every position (average price, last price, P/L) and the standing orders.
import { defineWidget, paint } from '../lib/widgets.js';
import { fleet, svcState } from '../lib/fleet.js';
import { esc, duration } from '../lib/format.js';
import { askHeld } from '../lib/position.js';
import { createTicket, tradable, routed } from '../lib/ticket.js';

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
    ${(() => { const g = optionGroups(a.name || a.label || svc.id, pos), can = tradable(a.name); return g.length || can ? `<div class="acct-payoffs">${g.map(x => `<button type="button" class="chip-btn" data-held="${esc(x.key)}" title="Load these legs into the Spread builder at their entry prices">Payoff: ${esc(x.sym)} ${esc(dayLabel(x.expiry))} · ${x.legs.length} leg${x.legs.length > 1 ? 's' : ''}</button>${can ? `<button type="button" class="chip-btn" data-ot-close="${esc(x.key)}" title="An order closing these legs, at market or a limit">Close…</button>` : ''}`).join('')}${can ? `<button type="button" class="chip-btn" data-sb-open="${esc(a.name)}" title="Build an order in the Spread builder, with this account picked for it">Spread builder</button>` : ''}</div>` : ''; })()}
    ${ordersPanel(a)}
    <div class="foot">${pos.length > shown ? `${pos.length} positions, largest ${shown} shown · ` : ''}${!pos.length && Array.isArray(a.orders) && !a.orders.length ? 'No positions or open orders · ' : !pos.length ? 'No positions · ' : ''}updated ${duration(Date.now() / 1000 - a.updated_at)} ago</div>
    ${flattenPanel(a)}
  </article>`;
}

// A held group, closed: each leg reversed, ratios in lowest terms, the quantity the rest.
const gcd = (a, b) => { while (b) [a, b] = [b, a % b]; return a; };
const occOf = (root, expiry, type, strike) => `${root}${expiry.slice(2, 4)}${expiry.slice(5, 7)}${expiry.slice(8, 10)}${type === 'call' ? 'C' : 'P'}${String(Math.round(strike * 1000)).padStart(8, '0')}`;
function closing(g) {
  const g0 = g.legs.reduce((x, l) => gcd(x, l.qty), 0) || 1;
  const legs = g.legs.map(l => ({ symbol: occOf(g.sym, g.expiry, l.type, l.strike), side: /** @type {'buy' | 'sell'} */ (l.side > 0 ? 'sell' : 'buy'), ratio: l.qty / g0 }));
  return { title: `Close ${g.sym} ${dayLabel(g.expiry)}`, acct: g.account, legs, qty: g0, effect: /** @type {'close'} */ ('close') };
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
// operator is typing a confirmation. The ticket has its own box above.
function mount(el, single = false) {
  // One account's view: which, kept with the panel.
  let want = '';
  if (single) { try { want = JSON.parse(el.dataset.args || '{}').account || ''; } catch (_) {} }
  el.innerHTML = '<div class="ot-box"></div><div class="acct-main"></div>';
  const box = /** @type {HTMLElement} */ (el.querySelector('.ot-box')), main = /** @type {HTMLElement} */ (el.querySelector('.acct-main'));
  const tk = createTicket(box);
  const update = () => {
    if (!(el.contains(document.activeElement) && document.activeElement.matches('.acct-flat-input, .acct-pick'))) paint(main, single ? drawOne(want) : draw());
    tk.update();
    readGateways(update); routed(update);
  };
  const click = async e => {
    const t = e.target.closest('button'); if (!t) return;
    const d = t.dataset;
    if (d.sbOpen) {
      // The Spread builder, with this account picked for its ticket.
      try { localStorage.setItem('tt-sb-account', d.sbOpen); } catch (_) {}
      document.dispatchEvent(new CustomEvent('tt-sb-account', { detail: d.sbOpen }));
      document.dispatchEvent(new CustomEvent('tt-show-view', { detail: 'spreads' }));
      return;
    }
    if (d.otClose) {
      const g = heldGroups.get(d.otClose); if (!g) return;
      // A group of more than four legs is two orders: built in the Spread builder.
      if (g.legs.length > 4) { askHeld(g); document.dispatchEvent(new CustomEvent('tt-show-view', { detail: 'spreads' })); return; }
      tk.open(closing(g)); return;
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
    if (box.contains(e.target)) return;
    const name = e.target.dataset?.flatTyped; if (!name || !flat[name]) return;
    flat[name].typed = e.target.value;
    const go = el.querySelector(`[data-flat-go="${CSS.escape(name)}"]`);
    if (go) go.disabled = e.target.value !== name;
  };
  const change = e => {
    if (e.target.classList?.contains('acct-pick')) {
      want = e.target.value; e.target.blur();
      el.dispatchEvent(new CustomEvent('tt-tab-args', { bubbles: true, detail: { args: { account: want } } }));
      update(); return;
    }
  };
  el.addEventListener('click', click); el.addEventListener('input', input); el.addEventListener('change', change);
  update();
  return { update, dispose: () => { tk.dispose(); el.removeEventListener('click', click); el.removeEventListener('input', input); el.removeEventListener('change', change); } };
}

defineWidget('accounts', { title: 'Accounts and equity', mount: el => mount(el) });
defineWidget('account', { title: 'Account', mount: el => mount(el, true) });

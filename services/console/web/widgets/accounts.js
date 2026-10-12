// Accounts: a card per brokerage account (live first) with equity, the day's
// change, balances, an equity chart and the largest positions. A paper
// account with an order gateway also gets Flatten (close every position at
// market, through the gateway, after typing the account's name) and, while
// its gateway halts opens, Resume opening.
import { defineWidget, paint } from '../lib/widgets.js';
import { fleet, svcState } from '../lib/fleet.js';
import { esc, duration } from '../lib/format.js';

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
  if (a?.environment === 'live' || g?.environment === 'live') return '<p class="muted" style="margin:0;font-size:12px">Live account: flatten isn\'t offered from the console.</p>';
  if (!g) return '';
  if (g.error) return `<p class="muted" style="margin:0;font-size:12px">Gateway unreachable: ${esc(g.error)}</p>`;
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
function accountCard(svc, a) {
  const acc = a?.account || {}, cur = acc.currency, env = a?.environment || 'paper';
  const head = `<h3>${esc(a?.label || svc.label || svc.id)}<span class="env env-${env === 'live' ? 'live' : 'paper'}">${esc(env)}</span>
    ${acc.status && acc.status !== 'ACTIVE' ? `<span class="pill p-warn">${esc(acc.status.toLowerCase())}</span>` : ''}
    ${acc.trading_blocked || acc.account_blocked ? '<span class="pill p-crit">blocked</span>' : ''}</h3>`;
  if (!a?.updated_at) {
    const err = svcState(svc.id).health?.body?.last_error;
    return `<article class="card acct">${head}<p class="muted" style="margin:0">${esc(err ? 'Cannot read the account: ' + err : 'Waiting for the first reading…')}</p></article>`;
  }
  const day = acc.equity != null && acc.last_equity != null ? acc.equity - acc.last_equity : null;
  const stats = [['Cash', money(acc.cash, cur)], ['Buying power', money(acc.buying_power, cur)], ['Long', money(acc.long_market_value, cur)]];
  if (acc.short_market_value) stats.push(['Short', money(acc.short_market_value, cur)]);
  const pos = a.positions || [];
  const rows = pos.slice().sort((x, y) => Math.abs(y.market_value || 0) - Math.abs(x.market_value || 0)).slice(0, 8).map(p => `<tr>
    <td style="color:var(--fg)">${esc(p.symbol)}</td><td>${esc(String(p.qty ?? '—'))}</td><td>${money(p.market_value, cur)}</td>
    <td class="${tone(p.unrealized_pl)}">${signed(p.unrealized_pl, cur)} <span class="muted">${pct(p.unrealized_plpc)}</span></td>
    <td class="${tone(p.change_today)}">${pct(p.change_today)}</td></tr>`).join('');
  return `<article class="card acct">${head}
    <div class="val"><span class="big">${money(acc.equity, cur)}</span>${day != null ? `<span class="chg ${tone(day)}">${signed(day, cur)} (${pct(acc.last_equity ? day / acc.last_equity : null)}) today</span>` : ''}</div>
    <div class="acct-stats">${stats.map(([k, v]) => `<div><span>${k}</span><b>${v}</b></div>`).join('')}</div>
    <div class="chart" data-chart="${esc(svc.id)}|recent|${esc(a.label || 'equity')}" data-decimals="2"></div>
    ${pos.length ? `<div class="scroll"><table class="pos-tbl"><thead><tr><th>Symbol</th><th>Qty</th><th>Value</th><th>Unrealized</th><th>Today</th></tr></thead><tbody>${rows}</tbody></table></div>` : '<p class="muted" style="margin:0;font-size:13px">No open positions.</p>'}
    <div class="foot">${pos.length > 8 ? `${pos.length} positions, largest 8 shown · ` : ''}updated ${duration(Date.now() / 1000 - a.updated_at)} ago</div>
    ${flattenPanel(a)}
  </article>`;
}

const draw = () => `<div><h1>Accounts</h1><p class="muted" style="margin:0">Balances and positions of every brokerage account this deployment reads. Paper accounts with an order gateway can be flattened.</p></div>` + (accountsView(false) || '<p class="muted">No account service is running.</p>');

// Redraws on every update, keeping the scroll position, but not while the
// operator is typing a confirmation.
function mount(el) {
  const update = () => {
    if (el.contains(document.activeElement) && document.activeElement.matches('.acct-flat-input')) return;
    paint(el, draw());
    readGateways(update);
  };
  const click = async e => {
    const t = e.target.closest('button'); if (!t) return;
    const d = t.dataset;
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
    const name = e.target.dataset?.flatTyped; if (!name || !flat[name]) return;
    flat[name].typed = e.target.value;
    const go = el.querySelector(`[data-flat-go="${CSS.escape(name)}"]`);
    if (go) go.disabled = e.target.value !== name;
  };
  el.addEventListener('click', click); el.addEventListener('input', input);
  update();
  return { update, dispose: () => { el.removeEventListener('click', click); el.removeEventListener('input', input); } };
}

defineWidget('accounts', { title: 'Accounts and equity', mount });

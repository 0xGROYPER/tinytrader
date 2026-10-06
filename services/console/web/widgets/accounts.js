// Accounts: a card per brokerage account (live first) with equity, the day's
// change, balances, an equity chart and the largest positions. Read only.
import { defineWidget, redrawing } from '../lib/widgets.js';
import { fleet, svcState } from '../lib/fleet.js';
import { esc, duration } from '../lib/format.js';

// Every service that serves an `account` stream gets a card: live accounts first.
function accountsView(withHeading = true) {
  // Every account of every service that serves an `accounts` stream.
  const accts = (fleet.topo?.services || []).filter(s => s.accounts)
    .flatMap(s => (svcState(s.id).accounts?.accounts || []).map(a => ({ svc: s, a })))
    .sort((x, y) => (x.a?.environment === 'live' ? 0 : 1) - (y.a?.environment === 'live' ? 0 : 1) || (x.a?.label || x.svc.id).localeCompare(y.a?.label || y.svc.id));
  if (!accts.length) return '';
  const grid = `<div class="cards acct-cards">${accts.map(({ svc, a }) => accountCard(svc, a)).join('')}</div>`;
  return withHeading ? `<section><div class="sec-head"><h2>Accounts</h2><span class="muted" style="font-size:13px">Balances and positions, read only</span></div>${grid}</section>` : grid;
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
  </article>`;
}

const draw = () => `<div><h1>Accounts</h1><p class="muted" style="margin:0">Balances and positions of every brokerage account this deployment reads. Read only.</p></div>` + (accountsView(false) || '<p class="muted">No account service is running.</p>');

defineWidget('accounts', { title: 'Accounts and equity', mount: redrawing(draw) });

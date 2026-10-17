// Performance: one account's trading P/L (the live one until another is
// picked, kept with the panel). A day's trades as a heatmap, a cell each,
// green or red in four steps by how much it made or lost, as the Data view
// shades days by size; then a month calendar of each day's P/L and trades
// (this month, paged back with ‹). Clicking a day shows its trades.
// Today's P/L, not in the history until the day is over, is the account's
// equity against yesterday's close, as its account service reads it.
// A trade is a round trip, flat to flat in one underlying and expiry (or one
// stock), rebuilt from the account's fills by its account service; its P/L
// is before fees. A day's P/L is the change in the account's equity
// (Alpaca's portfolio history) less deposits and withdrawals, so it includes
// fees and positions still open.
import { defineWidget, paint } from '../lib/widgets.js';
import { svcState, serviceFor, servicesFor } from '../lib/fleet.js';
import { esc } from '../lib/format.js';
import { occLabel } from '../lib/ticket.js';
import { get } from '../lib/cache.js';

const service = () => serviceFor('performance') || null;
const nyToday = () => new Date().toLocaleDateString('en-CA', { timeZone: 'America/New_York' });
const money = v => v == null ? '—' : `${v < 0 ? '−' : v > 0 ? '+' : ''}$${Math.abs(v).toLocaleString(undefined, { maximumFractionDigits: Math.abs(v) >= 1000 ? 0 : 2 })}`;
const short = d => new Date(d + 'T12:00:00Z').toLocaleDateString(undefined, { timeZone: 'UTC', month: 'short', day: 'numeric' });
const hm = t => new Date(t).toLocaleTimeString('en-GB', { timeZone: 'America/New_York', hour: '2-digit', minute: '2-digit' });
const tone = v => v > 0 ? 'up' : v < 0 ? 'down' : 'flat';
/** Four steps of strength, from a value's share of the largest. */
const level = (v, max) => !v || !max ? 0 : 1 + Math.min(3, Math.floor(Math.abs(v) / max * 4 - 1e-9));

function accounts() {
  for (const s of servicesFor('accounts')) {
    const list = svcState(s.id).accounts?.accounts || [];
    if (list.length) return list.map(a => ({ name: a.name, label: a.label || a.name, live: a.environment === 'live' }));
  }
  return [];
}

/** The account's days, with today's so far (equity against yesterday's close). */
function dayList(my) {
  const list = my.perf?.days || [], today = nyToday();
  if (list.some(d => d.date === today)) return list;
  for (const s of servicesFor('accounts')) {
    const a = (svcState(s.id).accounts?.accounts || []).find(x => x.name === my.acct)?.account;
    if (a?.equity && a?.last_equity) return [...list, { date: today, equity: a.equity, pnl: a.equity - a.last_equity, pnl_pct: (a.equity / a.last_equity - 1) * 100 }];
  }
  return list;
}

// The day's trades: a cell each, in the order they were opened.
function tradesView(my) {
  const t = my.trades;
  if (!t) return `<p class="muted">${my.tErr ? esc(my.tErr) : 'Loading the trades…'}</p>`;
  const list = t.trades || [];
  const closed = list.filter(x => !x.open), total = closed.reduce((s, x) => s + x.pnl, 0), wins = closed.filter(x => x.pnl > 0).length;
  const max = Math.max(0, ...closed.map(x => Math.abs(x.pnl)));
  const cells = list.map((x, i) => {
    const what = [...new Set(x.contracts.map(occLabel))].join(', ');
    const tip = `${x.key} · ${hm(x.start)}–${hm(x.end)} · ${x.open ? 'open (or closing a position from before)' : money(x.pnl)} · ${x.fills} fills\n${what}`;
    return `<button type="button" class="pf-cell ${x.open ? 'open' : `${tone(x.pnl)} l${level(x.pnl, max)}`}${my.pick === i ? ' picked' : ''}" data-pf-trade="${i}" title="${esc(tip)}"><span>${x.open ? 'open' : esc(money(Math.round(x.pnl)))}</span></button>`;
  }).join('');
  const p = list[my.pick];
  const detail = p ? `<p class="pf-detail"><b>${esc(p.key)}</b> ${hm(p.start)}–${hm(p.end)} · ${p.open ? 'open' : `<b class="${tone(p.pnl)}">${esc(money(p.pnl))}</b>`} · ${p.fills} fills · <span class="mono">${esc(p.contracts.map(occLabel).join(', '))}</span></p>` : '';
  const head = `<div class="pf-head"><b>Trades · ${esc(short(t.date))}${t.date === nyToday() ? ' (today)' : ''}</b>
    <span class="muted">${list.length ? `${closed.length} closed${list.length > closed.length ? `, ${list.length - closed.length} open` : ''} · ${wins} won · <b class="${tone(total)}">${esc(money(total))}</b> realized, before fees` : 'No trades'}</span></div>`;
  return `${head}${list.length ? `<div class="pf-cells">${cells}</div>
    <div class="cal-legend"><span>loss</span>${[4, 3, 2, 1].map(l => `<i class="pf-key down l${l}"></i>`).join('')}${[1, 2, 3, 4].map(l => `<i class="pf-key up l${l}"></i>`).join('')}<span>gain</span><span><i class="pf-key open"></i>open</span></div>${detail}` : ''}`;
}

// A month of days: each day's P/L and trades, tinted by its P/L.
function monthView(my) {
  const v = my.perf; if (!v) return `<p class="muted">${my.pErr ? esc(my.pErr) : 'Loading the days…'}</p>`;
  const byDay = new Map(dayList(my).map(d => [d.date, d])), counts = v.trades || {};
  const [y, m] = my.month.split('-').map(Number);
  const first = new Date(Date.UTC(y, m - 1, 1)), days = new Date(Date.UTC(y, m, 0)).getUTCDate();
  const inMonth = [...byDay.values()].filter(d => d.date.startsWith(my.month) && d.pnl != null);
  const max = Math.max(0, ...inMonth.map(d => Math.abs(d.pnl)));
  const sum = inMonth.reduce((s, d) => s + d.pnl, 0);
  const tc = Object.entries(counts).filter(([d]) => d.startsWith(my.month)).reduce((a, [, c]) => ({ closed: a.closed + c.closed, wins: a.wins + c.wins }), { closed: 0, wins: 0 });
  const today = nyToday();
  let cells = '';
  for (let i = 0; i < (first.getUTCDay() + 6) % 7; i++) cells += '<div class="pf-day blank"></div>';
  for (let d = 1; d <= days; d++) {
    const k = `${my.month}-${String(d).padStart(2, '0')}`, x = byDay.get(k), c = counts[k], wd = new Date(Date.UTC(y, m - 1, d)).getUTCDay();
    const cls = ['pf-day', wd === 0 || wd === 6 ? 'weekend' : '', x?.pnl ? `${tone(x.pnl)} l${level(x.pnl, max)}` : '', k === my.day ? 'picked' : '', k === today ? 'today' : ''].filter(Boolean).join(' ');
    cells += `<button type="button" class="${cls}" data-pf-day="${k}"${x || c ? '' : ' disabled'}><span class="n">${d}</span>${x?.pnl != null ? `<b>${esc(money(Math.round(x.pnl)))}</b>` : ''}${c?.trades ? `<small>${c.trades} trade${c.trades > 1 ? 's' : ''}${c.closed ? ` · ${c.wins}/${c.closed} won` : ''}</small>` : ''}${x?.transfers ? `<small>${x.transfers > 0 ? 'deposit' : 'withdrawal'} $${Math.abs(x.transfers).toLocaleString()}</small>` : ''}</button>`;
  }
  const name = first.toLocaleDateString(undefined, { timeZone: 'UTC', month: 'long', year: 'numeric' });
  return `<div class="pf-head"><span class="pf-nav"><button type="button" class="tile-btn" data-pf-month="-1" aria-label="Previous month">‹</button><b>${esc(name)}</b><button type="button" class="tile-btn" data-pf-month="1" aria-label="Next month">›</button></span>
      <span class="muted">${inMonth.length ? `<b class="${tone(sum)}">${esc(money(sum))}</b> over ${inMonth.length} day${inMonth.length > 1 ? 's' : ''}${tc.closed ? ` · ${tc.closed} trades, ${Math.round(tc.wins / tc.closed * 100)}% won` : ''}` : 'Nothing this month'}</span></div>
    <div class="pf-week">${['Mon', 'Tue', 'Wed', 'Thu', 'Fri', 'Sat', 'Sun'].map(d => `<span>${d}</span>`).join('')}</div>
    <div class="pf-month">${cells}</div>`;
}

defineWidget('performance', {
  title: 'Performance',
  mount(el) {
    const my = { acct: '', day: '', month: nyToday().slice(0, 7), perf: /** @type {any} */ (null), trades: /** @type {any} */ (null), pErr: '', tErr: '', pick: -1, pAt: 0, tAt: 0, alive: true };
    try { my.acct = JSON.parse(el.dataset.args || '{}').account || ''; } catch (_) {}
    el.innerHTML = '<div class="pf"><div class="pf-top"></div><section class="pf-sec pf-trades"></section><section class="pf-sec pf-cal"></section></div>';
    const $ = s => /** @type {HTMLElement} */ (el.querySelector(s));
    const ask = (what, q) => get(`/api/stream/${encodeURIComponent(service() || 'accounts')}/${what}?${q}`, { fresh: 5000 });
    const draw = () => {
      const list = accounts();
      if (!list.some(a => a.name === my.acct)) my.acct = (list.find(a => a.live) || list[0])?.name || '';
      paint($('.pf-top'), list.length ? `<select class="acct-pick pf-acct" aria-label="Account">${list.map(a => `<option value="${esc(a.name)}"${a.name === my.acct ? ' selected' : ''}>${esc(a.label)}</option>`).join('')}</select>` : '<p class="muted">No account service is running.</p>');
      if (!service()) { paint($('.pf-trades'), '<p class="muted">The account service needs redeploying for performance.</p>'); return; }
      paint($('.pf-trades'), tradesView(my)); paint($('.pf-cal'), monthView(my));
    };
    async function loadPerf() {
      if (!my.acct || !service()) return;
      const acct = my.acct; my.pAt = Date.now();
      try { const v = await ask('performance', `account=${encodeURIComponent(acct)}`); if (acct !== my.acct) return; my.perf = v; my.pErr = ''; }
      catch (e) { my.pErr = `Couldn't read the account's history: ${e.message}`; }
      // The day to show first: today, else the latest with trades.
      if (!my.day) { const t = Object.keys(my.perf?.trades || {}).sort(); my.day = t.includes(nyToday()) || !t.length ? nyToday() : t[t.length - 1]; my.month = my.day.slice(0, 7); loadTrades(); }
      if (my.alive) draw();
    }
    async function loadTrades() {
      if (!my.acct || !my.day || !service()) return;
      const acct = my.acct, day = my.day; my.tAt = Date.now();
      try { const v = await ask('trades', `account=${encodeURIComponent(acct)}&date=${day}`); if (acct !== my.acct || day !== my.day) return; my.trades = v; my.tErr = ''; }
      catch (e) { my.tErr = `Couldn't read the trades: ${e.message}`; }
      if (my.alive) draw();
    }
    const showDay = d => { my.day = d; my.month = d.slice(0, 7); my.trades = null; my.pick = -1; draw(); loadTrades(); };
    const click = e => {
      const b = /** @type {HTMLElement} */ (e.target).closest('[data-pf-day], [data-pf-trade], [data-pf-month]'); if (!b) return;
      const d = /** @type {HTMLElement} */ (b).dataset;
      if (d.pfDay) showDay(d.pfDay);
      else if (d.pfTrade != null) { my.pick = my.pick === +d.pfTrade ? -1 : +d.pfTrade; draw(); }
      else if (d.pfMonth) { const [y, m] = my.month.split('-').map(Number), t = new Date(Date.UTC(y, m - 1 + +d.pfMonth, 1)); my.month = t.toISOString().slice(0, 7); draw(); }
    };
    const change = e => {
      const t = /** @type {HTMLSelectElement} */ (e.target); if (!t.classList.contains('pf-acct')) return;
      my.acct = t.value; my.perf = null; my.trades = null; my.day = ''; my.pick = -1; t.blur();
      el.dispatchEvent(new CustomEvent('tt-tab-args', { bubbles: true, detail: { args: { account: my.acct } } }));
      draw(); loadPerf();
    };
    el.addEventListener('click', click); el.addEventListener('change', change);
    draw(); loadPerf();
    return {
      // Today's trades every half minute; the days every five minutes.
      update() {
        if (!my.perf && !my.pErr && Date.now() - my.pAt > 3000) loadPerf();
        else if (Date.now() - my.pAt > 300e3) loadPerf();
        if (my.day === nyToday() && Date.now() - my.tAt > 30e3) loadTrades();
        draw();
      },
      dispose() { my.alive = false; el.removeEventListener('click', click); el.removeEventListener('change', change); },
    };
  },
});

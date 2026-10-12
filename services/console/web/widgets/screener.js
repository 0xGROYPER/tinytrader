// Options screener: stocks ranked by what their options expect. The universe
// is every US listing at or above a market capitalisation (Nasdaq's
// screener); the largest of them, the day's movers and most traded, and
// Alpaca's most-active list are scanned for the at-the-money IV nearest 30
// days, the next expiry's straddle (the implied move), realized volatility
// and volume against its average. Views rank by IV, by the expected move in
// percent or in dollars of market value, by IV over realized, by the day's
// move and by volume, and by skew (the 30-day 25-delta risk reversal from the
// chain's fitted smile). Click a row to point the workspace at the stock.
import { defineWidget } from '../lib/widgets.js';
import { fleet } from '../lib/fleet.js';
import { esc } from '../lib/format.js';
import { setFocus } from '../lib/theme.js';
import { askRecommendation } from '../lib/position.js';

const service = () => (fleet.topo?.services || []).find(s => s.screener)?.id || null;
const SHOW = 50;

// The implied move to the expiry, scaled to a week (five sessions), so names
// with only monthly expiries compare with those listing weeklies.
const weekMove = r => r.implied_move != null && r.move_sessions > 0 ? r.implied_move * Math.sqrt(5 / r.move_sessions) : null;
const dollarMove = r => weekMove(r) != null && r.market_cap ? weekMove(r) * r.market_cap : null;

const VIEWS = {
  'Highest IV': { key: r => r.atm_iv_30d, note: 'At-the-money implied volatility of the expiry nearest 30 days.' },
  'Expected move': { key: weekMove, note: 'The at-the-money straddle over the price, for the first expiry at least three sessions away, scaled to a week.' },
  'Expected $ move': { key: dollarMove, note: 'The expected weekly move times market capitalisation: where the most value is expected to change hands in price.' },
  'Put skew': { key: r => r.skew, note: '25-delta risk reversal at the expiry nearest 30 days, from the fitted smile: put less call implied volatility, in points. High: downside priced richer; negative: upside richer. Blank when the chain is too thin to fit.' },
  'IV over realized': { key: r => r.iv_rv, note: '30-day implied volatility over 20-day realized: options priced rich (high) or cheap (low) against recent moves.' },
  'Top movers': { key: r => r.change_pct == null ? null : Math.abs(r.change_pct), note: 'The day\'s biggest moves either way, with what options expect next.' },
  'Unusual volume': { key: r => r.rel_volume, note: 'Shares traded against the 20-session average.' },
  'Most active': { key: r => r.dollar_volume, note: 'Dollar volume traded.' },
};
const CAPS = [[2e9, '≥ $2B'], [10e9, '≥ $10B'], [50e9, '≥ $50B'], [200e9, '≥ $200B']];

const pct = (x, d = 1) => x == null ? '' : `${(x * 100).toFixed(d)}%`;
const signed = x => x == null ? '' : `${x > 0 ? '+' : ''}${(x * 100).toFixed(2)}%`;
const money = v => v == null ? '' : v >= 1e12 ? `$${(v / 1e12).toFixed(2)}T` : v >= 1e9 ? `$${(v / 1e9).toFixed(v >= 1e10 ? 0 : 1)}B` : v >= 1e6 ? `$${(v / 1e6).toFixed(0)}M` : `$${Math.round(v / 1e3)}K`;
const num = (x, d = 2) => x == null ? '' : x.toFixed(d);
const short = d => d ? new Date(d + 'T12:00:00Z').toLocaleDateString(undefined, { month: 'short', day: 'numeric' }) : '';

/** Next earnings report per symbol, from the economic calendar service. */
function reports() {
  const e = Object.values(fleet.snap?.services || {}).map(s => s?.earnings).find(x => Array.isArray(x?.reports));
  const today = new Date().toLocaleDateString('en-CA', { timeZone: 'America/New_York' });
  const next = new Map();
  for (const r of e?.reports || []) if (r.date >= today && (!next.has(r.symbol) || r.date < next.get(r.symbol).date)) next.set(r.symbol, r);
  return next;
}

// Columns: label, value for sorting, cell.
/** @type {[string, (r: any, next?: Map<string, any>) => any, (r: any, next?: Map<string, any>) => string][]} */
const COLS = [
  ['Symbol', r => r.symbol, r => `<b class="mono">${esc(r.symbol)}</b><span class="sc-name">${esc(r.name.replace(/\s+(Common Stock|Class [A-C].*|Ordinary Shares.*|American Depositary.*)$/i, ''))}</span>`],
  ['Sector', r => r.sector, r => `<span class="sc-sector">${esc(r.sector)}</span>`],
  ['Price', r => r.price, r => num(r.price)],
  ['Day', r => r.change_pct, r => `<span class="${r.change_pct > 0 ? 'sc-up' : r.change_pct < 0 ? 'sc-down' : ''}">${signed(r.change_pct)}</span>`],
  ['Cap', r => r.market_cap, r => money(r.market_cap)],
  ['$ volume', r => r.dollar_volume, r => money(r.dollar_volume)],
  ['Rel vol', r => r.rel_volume, r => r.rel_volume == null ? '' : `<span class="${r.rel_volume >= 2 ? 'sc-hot' : ''}">${num(r.rel_volume, 1)}×</span>`],
  ['IV 30d', r => r.atm_iv_30d, r => pct(r.atm_iv_30d)],
  ['Exp. move', weekMove, (r, next) => {
    if (r.implied_move == null) return r.note ? `<span class="muted" title="${esc(r.note)}">—</span>` : '';
    const e = next.get(r.symbol), inside = e && e.date <= r.move_expiry;
    return `<span title="±${pct(r.implied_move, 2)} to ${esc(r.move_expiry)} (${r.move_sessions} sessions); about ±${pct(weekMove(r), 2)} a week">±${pct(r.implied_move)} <span class="muted">${short(r.move_expiry)}</span></span>${inside ? '<span class="sc-e" title="Reports before this expiry: the move includes earnings">E</span>' : ''}`;
  }],
  ['Exp. $ move', dollarMove, r => money(dollarMove(r))],
  ['Skew', r => r.skew, r => r.skew == null ? '' : `<span title="25-delta risk reversal, 30 days: put less call implied volatility">${r.skew > 0 ? '+' : ''}${(r.skew * 100).toFixed(1)}</span>`],
  ['RV 20d', r => r.rv_20d, r => pct(r.rv_20d)],
  ['IV / RV', r => r.iv_rv, r => r.iv_rv == null ? '' : `<span class="${r.iv_rv >= 1.5 ? 'sc-hot' : r.iv_rv < 0.8 ? 'sc-cool' : ''}">${num(r.iv_rv)}</span>`],
  ['Earnings', (r, next) => next.get(r.symbol)?.date || null, (r, next) => { const e = next.get(r.symbol); return e ? `${short(e.date)}${e.when === 'before-open' ? ' am' : e.when === 'after-close' ? ' pm' : ''}` : ''; }],
];

defineWidget('screener', {
  title: 'Options screener',
  mount(el) {
    const my = { data: null, err: '', view: 'Highest IV', sort: null, desc: true, cap: 2e9, sector: '', all: false, alive: true, at: 0, busy: false };
    el.innerHTML = `<div class="sc">
      <div class="sc-top">
        <div class="sc-views chips"></div>
        <label class="sc-field">Size <select class="sc-cap">${CAPS.map(([v, l]) => `<option value="${v}">${l}</option>`).join('')}</select></label>
        <label class="sc-field">Sector <select class="sc-sec"><option value="">All</option></select></label>
      </div>
      <p class="sc-note muted"></p>
      <div class="sc-wrap"><table class="sc-table"></table></div>
      <p class="sc-msg muted"></p></div>`;
    const $ = s => el.querySelector(s);

    async function load() {
      const svc = service();
      if (!svc) { my.err = 'No service serves the screener (alpaca-data).'; render(); return; }
      if (my.busy) return;
      my.busy = true;
      try {
        const r = await fetch(`/api/stream/${encodeURIComponent(svc)}/screener?min_cap=${my.cap}`, { cache: 'no-store' });
        const v = await r.json().catch(() => ({}));
        if (!r.ok) throw new Error(v.error || `${r.status}`);
        if (!my.alive) return;
        my.data = v; my.err = '';
      } catch (e) { my.err = e.message; }
      my.busy = false; my.at = Date.now();
      render();
    }

    function render() {
      $('.sc-views').innerHTML = Object.keys(VIEWS).map(n => `<button type="button" class="chip-btn" data-view="${esc(n)}" aria-pressed="${my.view === n}">${esc(n)}</button>`).join('');
      $('.sc-cap').value = String(my.cap);
      const v = my.data;
      const rows = v?.rows || [];
      const sectors = [...new Set(rows.map(r => r.sector).filter(Boolean))].sort();
      $('.sc-sec').innerHTML = `<option value="">All</option>${sectors.map(s => `<option${s === my.sector ? ' selected' : ''}>${esc(s)}</option>`).join('')}`;
      $('.sc-note').textContent = VIEWS[my.view].note;
      if (!v) { $('.sc-table').innerHTML = ''; $('.sc-msg').textContent = my.err || 'Loading…'; return; }
      const next = reports();
      const col = my.sort != null ? COLS[my.sort] : null;
      const key = col ? (r => col[1](r, next)) : VIEWS[my.view].key;
      const list = rows.filter(r => !my.sector || r.sector === my.sector)
        .map(r => ({ r, k: key(r) }))
        .sort((a, b) => Number(a.k == null) - Number(b.k == null) || (typeof a.k === 'string' ? a.k.localeCompare(b.k) : a.k - b.k) * (my.desc ? -1 : 1));
      const shown = my.all ? list : list.slice(0, SHOW);
      $('.sc-table').innerHTML = `<thead><tr><th>#</th>${COLS.map(([l], i) => `<th data-col="${i}" class="${my.sort === i ? 'sc-sorted' : ''}">${l}${my.sort === i ? (my.desc ? ' ↓' : ' ↑') : ''}</th>`).join('')}</tr></thead>
        <tbody>${shown.map(({ r }, i) => `<tr data-sym="${esc(r.symbol)}" title="${esc(r.name)} · listed for ${esc(r.sources.join(', '))}"><td class="muted">${i + 1}</td>${COLS.map(c => `<td>${c[2](r, next)}</td>`).join('')}</tr>`).join('')}</tbody>`;
      const more = list.length - shown.length;
      const at = new Date(v.as_of * 1000).toLocaleTimeString([], { hour: 'numeric', minute: '2-digit' });
      $('.sc-msg').innerHTML = `${more > 0 ? `<button type="button" class="chip-btn sc-more">Show ${more} more</button> ` : ''}${rows.length} stocks of ${v.matched.toLocaleString()} listed at this size and over $${v.min_price}${v.pending ? `; scanning options for ${v.pending}` : ''}. As of ${at}; options from Alpaca's ${esc(v.options_feed)} feed.${my.err ? ` ${esc(my.err)}` : ''}`;
    }

    el.addEventListener('click', e => {
      const t = /** @type {HTMLElement} */ (e.target);
      const b = t.closest('button');
      if (b?.dataset.view) { my.view = b.dataset.view; my.sort = null; my.desc = true; my.all = false; render(); return; }
      if (b?.classList.contains('sc-more')) { my.all = true; render(); return; }
      const th = /** @type {HTMLElement | null} */ (t.closest('th[data-col]'));
      if (th) { const i = +th.dataset.col; my.desc = my.sort === i ? !my.desc : i > 1; my.sort = i; render(); return; }
      const row = t.closest('tr[data-sym]');
      if (row) {
        const sym = /** @type {HTMLElement} */ (row).dataset.sym, r = (my.data?.rows || []).find(x => x.symbol === sym);
        // A suggested position in the Spread builder (opened if need be), and the symbol everywhere else.
        askRecommendation({ sym, view: my.sort != null ? '' : my.view, row: r || null, earnings: reports().get(sym)?.date || null });
        document.dispatchEvent(new CustomEvent('tt-show-view', { detail: 'spreads' }));
        setFocus(sym); document.dispatchEvent(new CustomEvent('tt-focus', { detail: sym }));
      }
    });
    el.addEventListener('change', e => {
      const t = /** @type {HTMLSelectElement} */ (e.target);
      if (t.classList.contains('sc-cap')) { my.cap = +t.value; my.data = null; render(); load(); }
      if (t.classList.contains('sc-sec')) { my.sector = t.value; render(); }
    });
    render(); load();
    // Quickly while options are still being scanned; every two minutes after.
    const timer = setInterval(() => { if (Date.now() - my.at > (my.data?.pending ? 15e3 : 120e3)) load(); }, 5000);
    return {
      update() {},
      dispose() { my.alive = false; clearInterval(timer); },
    };
  },
});

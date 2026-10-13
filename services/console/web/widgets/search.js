// Search: the console's front page, laid out like the docs' home: the mark and
// a search box, here for a ticker (the header has the same box: bindSearch). As you type, a menu narrows the tickers the
// console knows (earnings calendar companies with their names, the relays'
// symbols, symbols in recent news): exact, then by symbol, then by name.
// Arrow keys choose, Enter opens, Escape closes. Picking a ticker shows its
// results in the same panel: price and the day's change, what the screener
// knows of it (sector, size, options), its next earnings and recent news,
// with buttons to chart it, build options on it, add it to a watchlist or
// open it as a view of its own. A search in the header shows its results in
// the view's Search panel, or a new one beside the active panel (lib/app.js).
import { defineWidget, paint } from '../lib/widgets.js';
import { fleet, newsItems } from '../lib/fleet.js';
import { parseSymbols, setFocus } from '../lib/theme.js';
import { esc, ago, unentity } from '../lib/format.js';
import { watchlists, addSymbols } from '../lib/watchlists.js';
import { mark } from './docs.js';

const MAX = 8;

/** Every ticker EDGAR lists, with its company's name: symbol to name. Fetched once
 * per page, from econ-calendar; empty until it arrives (or if it can't be had). */
let directory = null;
/** @type {Promise<void> | null} */
let loading = null;
function loadDirectory() {
  loading ||= fetch('/api/stream/econ-calendar/tickers', { cache: 'no-store' })
    .then(r => (r.ok ? r.json() : null))
    .then(j => { if (j?.names) directory = j.names; else loading = null; })
    .catch(() => { loading = null; });
  return loading;
}

/** Every ticker the console knows, with its company name where one is known. @returns {Map<string, string>} */
function universe() {
  const map = new Map();
  for (const [sym, name] of Object.entries(directory || {})) map.set(sym, name);
  for (const s of Object.values(fleet.snap?.services || {})) {
    for (const r of s?.earnings?.reports || []) if (r.symbol) map.set(r.symbol, r.name || map.get(r.symbol) || '');
  }
  for (const s of fleet.topo?.services || []) for (const x of s.schedule || []) for (const sym of x.symbols || []) if (!map.has(sym)) map.set(sym, '');
  for (const n of newsItems()) for (const sym of n.symbols || []) if (!map.has(sym)) map.set(sym, '');
  return map;
}

/** The tickers matching what's typed, best first: [symbol, name]. @param {string} q */
function suggest(q) {
  const typed = q.trim();
  if (!typed) return [];
  const sym = typed.toUpperCase(), word = typed.toLowerCase();
  const hits = [];
  for (const [s, name] of universe()) {
    const nm = name.toLowerCase();
    const rank = s === sym ? 0
      : s.startsWith(sym) ? 1
      : word.length >= 2 && nm.startsWith(word) ? 2
      : word.length >= 2 && nm.split(/\W+/).some(w => w.startsWith(word)) ? 3
      : s.includes(sym) ? 4 : -1;
    if (rank >= 0) hits.push({ rank, s, name });
  }
  hits.sort((a, b) => a.rank - b.rank || a.s.length - b.s.length || a.s.localeCompare(b.s));
  return hits.slice(0, MAX).map(({ s, name }) => /** @type {[string, string]} */ ([s, name]));
}

/**
 * A ticker search box: as you type, a menu of matching tickers; arrow keys
 * choose, Enter or a click picks it, and Escape closes. Used by the Search
 * widget (which shows the results itself) and the header's box (which asks
 * the shell to: `tt-open-ticker`).
 * @param {HTMLElement} root holds the form, menu and error line
 * @param {(symbol: string) => void} [onPick] what picking does; else `tt-open-ticker`
 * @returns {() => void} removes its listeners
 */
export function bindSearch(root, onPick) {
  const form = /** @type {HTMLFormElement} */ (root.querySelector('form'));
  const input = /** @type {HTMLInputElement} */ (root.querySelector('input[type="search"]'));
  const list = /** @type {HTMLElement} */ (root.querySelector('.hm-sugg'));
  const err = /** @type {HTMLElement} */ (root.querySelector('.hm-err'));
  /** @type {[string, string][]} */
  let shown = [];
  let active = -1;

  const close = () => { list.hidden = true; list.innerHTML = ''; shown = []; active = -1; input.setAttribute('aria-expanded', 'false'); };
  const paintList = () => {
    list.innerHTML = shown.map(([s, name], i) => `<li role="option" data-sym="${esc(s)}" aria-selected="${i === active}"><b>${esc(s)}</b>${name ? `<span class="muted">${esc(name)}</span>` : ''}</li>`).join('');
    list.hidden = !shown.length;
    input.setAttribute('aria-expanded', String(!list.hidden));
  };
  const show = () => { err.hidden = true; shown = suggest(input.value); active = -1; paintList(); };
  // The full list may arrive after the box is used: the menu then refreshes.
  const typedFirst = () => loadDirectory().then(() => { if (input.value.trim() && document.activeElement === input) show(); });

  const open = symbol => {
    input.value = '';
    close();
    err.hidden = true;
    if (onPick) { onPick(symbol); return; }
    const detail = { symbol, opened: false };
    root.dispatchEvent(new CustomEvent('tt-open-ticker', { bubbles: true, detail }));
    if (!detail.opened) { err.textContent = 'There\'s no view open to show it in.'; err.hidden = false; }
    else input.blur();
  };
  const submit = e => {
    e.preventDefault();
    const symbol = parseSymbols(input.value)[0];
    if (!symbol) { err.textContent = 'Type a ticker, such as SPY or NVDA.'; err.hidden = false; input.focus(); return; }
    open(symbol);
  };
  const key = e => {
    if (e.target !== input) return;
    if (e.key === 'Escape') { close(); err.hidden = true; return; }
    if (!shown.length) return;
    if (e.key === 'ArrowDown' || e.key === 'ArrowUp') {
      e.preventDefault();
      // Positions 0 (none) to shown.length, wrapping; active is the position less one.
      const n = shown.length + 1;
      active = ((active + 1 + (e.key === 'ArrowDown' ? 1 : -1)) % n + n) % n - 1;
      paintList();
    } else if (e.key === 'Enter' && active >= 0) {
      e.preventDefault();
      open(shown[active][0]);
    }
  };
  const pick = e => {
    const item = /** @type {HTMLElement | null} */ (/** @type {HTMLElement} */ (e.target).closest?.('[data-sym]'));
    if (item) open(item.dataset.sym || '');
  };
  // Typing narrows the menu and redraws only it, so the box keeps its focus.
  const typed = e => { if (e.target === input) { show(); typedFirst(); } };
  const blur = e => { if (!root.contains(/** @type {Node | null} */ (e.relatedTarget))) { close(); err.hidden = true; } };
  const hold = e => e.preventDefault(); // keeps the box focused for the click

  form.addEventListener('submit', submit);
  root.addEventListener('keydown', key);
  root.addEventListener('input', typed);
  list.addEventListener('mousedown', hold);
  list.addEventListener('click', pick);
  root.addEventListener('focusout', blur);
  return () => {
    form.removeEventListener('submit', submit);
    root.removeEventListener('keydown', key);
    root.removeEventListener('input', typed);
    list.removeEventListener('mousedown', hold);
    list.removeEventListener('click', pick);
    root.removeEventListener('focusout', blur);
  };
}

const cap = v => !v ? '' : v >= 1e12 ? `$${(v / 1e12).toFixed(2)}T` : v >= 1e9 ? `$${Math.round(v / 1e9)}B` : `$${Math.round(v / 1e6)}M`;
const pct = v => v == null ? '—' : `${v > 0 ? '+' : v < 0 ? '−' : ''}${Math.abs(v * 100).toFixed(2)}%`;
const svcWith = k => (fleet.topo?.services || []).find(s => s[k])?.id || null;
const getJson = async url => { const r = await fetch(url, { cache: 'no-store' }); return r.ok ? r.json() : null; };

/** A ticker's results: quote, the screener's row, next earnings and recent news. */
async function results(sym) {
  const snap = svcWith('snapshots'), scr = svcWith('screener'), news = svcWith('news_search');
  const [q, rows, found] = await Promise.all([
    snap ? getJson(`/api/stream/${encodeURIComponent(snap)}/snapshots?symbols=${encodeURIComponent(sym)}`).catch(() => null) : null,
    scr ? getJson(`/api/stream/${encodeURIComponent(scr)}/screener`).catch(() => null) : null,
    news ? getJson(`/api/stream/${encodeURIComponent(news)}/news_search?symbols=${encodeURIComponent(sym)}&limit=20`).catch(() => null) : null,
  ]);
  const today = new Date().toLocaleDateString('en-CA', { timeZone: 'America/New_York' });
  const earn = Object.values(fleet.snap?.services || {}).map(s => s?.earnings).find(e => Array.isArray(e?.reports));
  const live = newsItems().filter(n => (n.symbols || []).includes(sym));
  const stories = [...new Map([...live, ...(found?.items || [])].map(n => [n.id, n])).values()].sort((a, b) => (b.created_at || '').localeCompare(a.created_at || ''));
  return {
    quote: (q?.snapshots || []).find(x => x.symbol === sym) || null,
    row: (rows?.rows || []).find(x => x.symbol === sym) || null,
    next: (earn?.reports || []).filter(r => r.symbol === sym && r.date >= today).sort((a, b) => a.date.localeCompare(b.date))[0] || null,
    stories: stories.slice(0, 20),
  };
}

function resultsView(sym, res) {
  const name = res?.row?.name || universe().get(sym) || '';
  if (!res) return `<div class="sr"><div class="sr-head"><h2 class="mono">${esc(sym)}</h2><span class="muted">${esc(name)}</span></div><p class="muted">Looking it up…</p></div>`;
  const { quote: qt, row, next, stories } = res, c = qt?.change_pct;
  const stat = (k, v) => v ? `<div><span>${k}</span><b>${v}</b></div>` : '';
  const stats = row ? [
    stat('Sector', esc(row.sector || '')), stat('Market value', cap(row.market_cap)),
    stat('Week', pct(row.change_5d)), stat('Month', pct(row.change_21d)),
    stat('IV 30d', row.atm_iv_30d != null ? `${(row.atm_iv_30d * 100).toFixed(0)}%` : ''), stat('IV / RV', row.iv_rv != null ? row.iv_rv.toFixed(2) : ''),
    stat('Expected move', row.implied_move != null ? `±${(row.implied_move * 100).toFixed(1)}% to ${esc(row.move_expiry || '')}` : ''),
    stat('Volume vs 20d', row.rel_volume != null ? `${row.rel_volume.toFixed(2)}×` : ''),
  ].join('') : '';
  const when = { 'before-open': 'before the open', 'after-close': 'after the close' };
  return `<div class="sr">
    <div class="sr-head"><h2 class="mono">${esc(sym)}</h2><span class="sr-name">${esc(name)}</span></div>
    <div class="sr-quote">${qt?.price != null ? `<span class="big mono">${qt.price.toFixed(2)}</span><span class="mono ${c > 0 ? 'pos' : c < 0 ? 'neg' : ''}">${qt.change != null ? `${qt.change >= 0 ? '+' : '−'}${Math.abs(qt.change).toFixed(2)}` : ''} (${pct(c)}) today</span>` : '<span class="muted">No price for it.</span>'}</div>
    <div class="sr-acts"><button type="button" class="btn small" data-sr="chart">Chart</button><button type="button" class="btn small" data-sr="options">Options</button>
      <select class="sr-list" aria-label="Add to a watchlist"><option value="">Add to watchlist…</option>${watchlists().map(l => `<option value="${esc(l.name)}"${l.symbols.includes(sym) ? ' disabled' : ''}>${esc(l.name)}${l.symbols.includes(sym) ? ' (in it)' : ''}</option>`).join('')}</select>
      <button type="button" class="btn small" data-sr="view" title="A view of its own, with its chart, news and earnings">Open as a view</button></div>
    ${stats ? `<div class="sr-stats">${stats}</div>` : ''}
    ${next ? `<p class="sr-earn">Next earnings <b>${esc(next.date)}</b>${when[next.when] ? `, ${when[next.when]}` : ''}${next.eps_estimate != null ? ` · EPS estimate ${next.eps_estimate}` : ''}</p>` : ''}
    <h3>News</h3>
    <div class="news">${stories.map(n => `<article class="story"><div class="story-meta"><span>${esc(ago(n.created_at))}</span><span>${esc(n.source || '')}</span></div>
      ${/^https?:/.test(n.url || '') ? `<a href="${esc(n.url)}" target="_blank" rel="noopener">${esc(unentity(n.headline))}</a>` : `<b>${esc(unentity(n.headline))}</b>`}
      ${n.summary ? `<p>${esc(unentity(n.summary))}</p>` : ''}</article>`).join('') || '<p class="muted" style="margin:0">No recent stories.</p>'}</div>
  </div>`;
}

defineWidget('search', {
  title: 'Search',
  mount(el) {
    // A symbol to show from the start (the header's search, opening this panel).
    let args = {}; try { args = JSON.parse(el.dataset.args || '{}'); } catch (_) {}
    const my = { sym: /** @type {string | null} */ (args.symbol || null), res: /** @type {any} */ (null), alive: true };
    const box = (compact) => `<div class="${compact ? 'sr-search' : 'docs-home'}">
      ${compact ? `<button type="button" class="tile-btn" data-sr="back" title="Back to Search" aria-label="Back to Search">‹</button>` : mark()}
      <form class="hm-form" role="search" autocomplete="off">
        <input class="dh-search" type="search" name="ticker" placeholder="Search a ticker, such as SPY" aria-label="Search a ticker" aria-autocomplete="list" aria-controls="hm-list" aria-expanded="false" spellcheck="false">
        <ul class="hm-sugg" id="hm-list" role="listbox" aria-label="Tickers" hidden></ul>
      </form>
      <p class="muted hm-err" style="margin:0" hidden></p>
    </div>`;
    el.innerHTML = '<div class="sr-top"></div><div class="sr-main"></div>';
    const top = /** @type {HTMLElement} */ (el.querySelector('.sr-top')), main = /** @type {HTMLElement} */ (el.querySelector('.sr-main'));
    let unbind = () => {}, compact = null;
    const draw = () => {
      // The box is rebuilt only when it changes shape, so it keeps what's typed.
      if (compact !== !!my.sym) { compact = !!my.sym; unbind(); top.innerHTML = box(compact); unbind = bindSearch(top, show); }
      paint(main, my.sym ? resultsView(my.sym, my.res) : '');
    };
    const show = sym => {
      my.sym = sym; my.res = null; draw();
      el.dispatchEvent(new CustomEvent('tt-tab-args', { bubbles: true, detail: { args: { symbol: sym }, title: `Search · ${sym}` } }));
      results(sym).then(r => { if (my.alive && my.sym === sym) { my.res = r; draw(); } });
    };
    const click = e => {
      const a = /** @type {HTMLElement | null} */ (/** @type {HTMLElement} */ (e.target).closest('[data-sr]'))?.dataset.sr;
      if (!a) return;
      if (a === 'back') { my.sym = null; my.res = null; draw(); el.dispatchEvent(new CustomEvent('tt-tab-args', { bubbles: true, detail: { args: {}, title: 'Search' } })); return; }
      const sym = my.sym; if (!sym) return;
      if (a === 'chart' || a === 'options') {
        setFocus(sym); document.dispatchEvent(new CustomEvent('tt-focus', { detail: sym }));
        document.dispatchEvent(new CustomEvent('tt-show-view', { detail: a === 'chart' ? 'candles' : 'spreads' }));
      }
      if (a === 'view') el.dispatchEvent(new CustomEvent('tt-open-ticker-view', { bubbles: true, detail: { symbol: sym } }));
    };
    const change = e => {
      const s = /** @type {HTMLSelectElement} */ (e.target);
      if (s.classList.contains('sr-list') && s.value && my.sym) { addSymbols(s.value, [my.sym]); draw(); }
    };
    // The header's search, asking this panel to show a ticker.
    const asked = e => { const d = /** @type {CustomEvent} */ (e).detail; if (d?.panel === el) show(d.symbol); };
    el.addEventListener('click', click); el.addEventListener('change', change);
    document.addEventListener('tt-search-show', asked);
    draw();
    if (my.sym) show(my.sym);
    return { dispose: () => { my.alive = false; unbind(); el.removeEventListener('click', click); el.removeEventListener('change', change); document.removeEventListener('tt-search-show', asked); } };
  },
});

// Search: the console's front page, laid out like the docs' home: the mark and
// a search box, here for a ticker. As you type, a menu narrows the tickers the
// console knows (earnings calendar companies with their names, the relays'
// symbols, symbols in recent news): exact, then by symbol, then by name.
// Arrow keys choose, Enter opens, Escape closes. Picking a ticker opens a
// workspace on it, with its candle chart, news and earnings, all following
// that workspace's theme (lib/app.js). Panels are the existing widgets.
import { defineWidget } from '../lib/widgets.js';
import { fleet, newsItems } from '../lib/fleet.js';
import { parseSymbols } from '../lib/theme.js';
import { esc } from '../lib/format.js';
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

defineWidget('search', {
  title: 'Search',
  mount(el) {
    el.innerHTML = `<div class="docs-home">
      ${mark()}
      <form class="hm-form" role="search" autocomplete="off">
        <input class="dh-search" type="search" name="ticker" placeholder="Search a ticker, such as SPY" aria-label="Search a ticker" aria-autocomplete="list" aria-controls="hm-list" aria-expanded="false" spellcheck="false">
        <ul class="hm-sugg" id="hm-list" role="listbox" aria-label="Tickers" hidden></ul>
      </form>
      <p class="muted hm-err" style="margin:0" hidden></p>
    </div>`;
    const input = /** @type {HTMLInputElement} */ (el.querySelector('.dh-search'));
    const list = /** @type {HTMLElement} */ (el.querySelector('.hm-sugg'));
    const err = /** @type {HTMLElement} */ (el.querySelector('.hm-err'));
    /** @type {[string, string][]} */
    let shown = [];
    let active = -1;

    const close = () => { list.hidden = true; list.innerHTML = ''; shown = []; active = -1; input.setAttribute('aria-expanded', 'false'); };
    const paintList = () => {
      list.innerHTML = shown.map(([s, name], i) => `<li role="option" data-sym="${esc(s)}" aria-selected="${i === active}"><b>${esc(s)}</b>${name ? `<span class="muted">${esc(name)}</span>` : ''}</li>`).join('');
      list.hidden = !shown.length;
      input.setAttribute('aria-expanded', String(!list.hidden));
    };
    const show = () => { shown = suggest(input.value); active = -1; paintList(); };
    // The full list may arrive after the box is used: the menu then refreshes.
    loadDirectory().then(() => { if (input.value.trim()) show(); });

    // Opens a workspace on the ticker; the search box learns whether it did.
    const open = symbol => {
      input.value = '';
      close();
      err.hidden = true;
      const detail = { symbol, opened: false };
      el.dispatchEvent(new CustomEvent('tt-open-ticker', { bubbles: true, detail }));
      if (!detail.opened) { err.textContent = 'All ten workspaces are in use. Close one first.'; err.hidden = false; }
    };
    const submit = e => {
      e.preventDefault();
      const symbol = parseSymbols(input.value)[0];
      if (!symbol) { err.textContent = 'Type a ticker, such as SPY or NVDA.'; err.hidden = false; input.focus(); return; }
      open(symbol);
    };
    const key = e => {
      if (e.target !== input) return;
      if (e.key === 'Escape') { close(); return; }
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
    const typed = e => { if (e.target === input) show(); };
    const blur = e => { if (!el.contains(e.relatedTarget)) close(); };

    el.querySelector('form').addEventListener('submit', submit);
    el.addEventListener('keydown', key);
    el.addEventListener('input', typed);
    list.addEventListener('mousedown', e => e.preventDefault()); // keeps the box focused for the click
    list.addEventListener('click', pick);
    el.addEventListener('focusout', blur);
    return {
      dispose() {
        el.querySelector('form')?.removeEventListener('submit', submit);
        el.removeEventListener('keydown', key);
        el.removeEventListener('input', typed);
        el.removeEventListener('focusout', blur);
        list.removeEventListener('click', pick);
      },
    };
  },
});

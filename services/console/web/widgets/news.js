// News: recent stories from every service streaming news, newest first,
// narrowed by a search box (words in the headline or summary, or a symbol;
// a ticker typed and entered also fetches recent stories on it). In a workspace with a
// theme, the Theme chip (chosen first) shows stories on its symbols or
// mentioning its keywords, adding recent stories on its symbols from a
// service that searches news (alpaca-data). The Watchlist chip does the same
// for the list picked in the Watchlist widget (lib/watchlists.js), and is
// chosen first in a view without a theme.
import { defineWidget, paint } from '../lib/widgets.js';
import { newsItems, fleet } from '../lib/fleet.js';
import { esc, ago, unentity } from '../lib/format.js';
import { theme, onTheme } from '../lib/theme.js';
import { picked, symbolsOf, THEME_LIST } from '../lib/watchlists.js';

const THEME = '\u0000theme', WATCH = '\u0000watch';
/** The picked watchlist, unless it's the theme's: [name, symbols], or null. */
/** @returns {[string, string[]] | null} */
const watch = () => { const n = picked(!!theme()?.symbols.length); return n && n !== THEME_LIST ? [n, symbolsOf(n)] : null; };

// Stories matching what's typed: every word in the headline, summary or
// symbols (a word in capitals is taken as a symbol, matched exactly).
function matches(n, q) {
  const words = q.trim().split(/\s+/).filter(Boolean);
  if (!words.length) return true;
  const text = `${unentity(n.headline || '')} ${unentity(n.summary || '')}`.toLowerCase(), syms = n.symbols || [];
  return words.every(w => (/^[A-Z][A-Z0-9.]{0,9}$/.test(w) && syms.includes(w)) || text.includes(w.toLowerCase()) || syms.includes(w.toUpperCase()));
}

function draw(my) {
  const t = theme(), w = watch();
  const live = newsItems();
  // The theme's and the watchlist's stories, and those found for a ticker searched: the live feed, plus what was found.
  const items = [...new Map([...live, ...(t ? my.found || [] : []), ...(w ? my.watched || [] : []), ...(my.searched || [])].map(n => [n.id, n])).values()].sort((a, b) => (b.created_at || '').localeCompare(a.created_at || ''));
  if (my.symbol === undefined) my.symbol = t ? THEME : w?.[1].length ? WATCH : null;
  if (my.symbol === THEME && !t) my.symbol = null;
  if (my.symbol === WATCH && !w) my.symbol = null;
  const inList = w ? new Set(w[1]) : null;
  const pool = my.symbol === THEME && t ? items.filter(n => onTheme(n, t))
    : my.symbol === WATCH && inList ? items.filter(n => (n.symbols || []).some(s => inList.has(s)))
    : items;
  // A search looks through every story, whichever chip is chosen.
  const searching = !!my.q.trim(), shown = (searching ? items : pool).filter(n => matches(n, my.q));
  const chip = (sym, label) => `<button type="button" class="chip-btn" data-news-symbol="${esc(sym ?? '')}" aria-pressed="${(my.symbol ?? null) === sym}">${esc(label)}</button>`;
  const top = `<div class="nw-bar"><form class="nw-search" role="search"><input class="dh-search nw-q" type="search" placeholder="Search the news: words, or a ticker and Enter" aria-label="Search the news" autocomplete="off" spellcheck="false"></form>
    ${t || w ? `<span class="chips">${t ? chip(THEME, 'Theme') : ''}${w ? chip(WATCH, `Watchlist: ${w[0]}`) : ''}${chip(null, 'All')}</span>` : ''}</div>`;
  if (!items.length) return [top, '<p class="muted">No news service is running, or it has no stories yet.</p>'];
  const list = `<p class="muted nw-count">${searching ? `${shown.length} of ${items.length} stories match` : `${pool.length} recent stories`}, newest first${my.looking ? ' · looking for more…' : ''}</p>
    <div class="news">${shown.slice(0, 100).map(n => `<article class="story">
      <div class="story-meta"><span>${esc(ago(n.created_at))}</span><span>${esc(n.source || '')}${n.author ? ' · ' + esc(n.author) : ''}</span>
        ${(n.symbols || []).slice(0, 8).map(sym => `<button type="button" class="tag" data-news-q="${esc(sym)}" title="Search for ${esc(sym)}">${esc(sym)}</button>`).join('')}</div>
      ${/^https?:/.test(n.url || '') ? `<a href="${esc(n.url)}" target="_blank" rel="noopener">${esc(unentity(n.headline))}</a>` : `<b>${esc(unentity(n.headline))}</b>`}
      ${n.summary ? `<p>${esc(unentity(n.summary))}</p>` : ''}
    </article>`).join('') || `<p class="muted" style="padding:16px;margin:0">${my.q ? 'No stories match.' : 'No stories here.'}</p>`}</div>`;
  return [top, list];
}

defineWidget('news', {
  title: 'News',
  mount(el) {
    const my = { symbol: undefined, found: [], watched: [], searched: [], q: '', looking: false, alive: true };
    // The box and the list are painted apart, so a refresh never takes what's being typed.
    el.innerHTML = '<div class="nw-top"></div><div class="nw-list"></div>';
    const topEl = /** @type {HTMLElement} */ (el.querySelector('.nw-top')), listEl = /** @type {HTMLElement} */ (el.querySelector('.nw-list'));
    // Recent stories on the theme's symbols, from a service that searches news.
    async function find() {
      const t = theme(), svc = (fleet.topo?.services || []).find(s => s.news_search)?.id;
      if (!my.alive || !t?.symbols.length || !svc) return;
      try {
        const r = await fetch(`/api/stream/${encodeURIComponent(svc)}/news_search?symbols=${encodeURIComponent(t.symbols.join(','))}&limit=50`, { cache: 'no-store' });
        if (r.ok) { my.found = (await r.json()).items || []; if (my.alive) update(); }
      } catch (_) {}
    }
    // And on the picked watchlist's.
    async function findWatched() {
      const w = watch(), svc = (fleet.topo?.services || []).find(s => s.news_search)?.id;
      if (!my.alive || !w?.[1].length || !svc) { my.watched = []; return; }
      try {
        const r = await fetch(`/api/stream/${encodeURIComponent(svc)}/news_search?symbols=${encodeURIComponent(w[1].join(','))}&limit=50`, { cache: 'no-store' });
        if (r.ok) { my.watched = (await r.json()).items || []; if (my.alive) update(); }
      } catch (_) {}
    }
    const timer = setInterval(() => { find(); findWatched(); }, 60_000);
    const listed = () => { if (my.symbol === WATCH || my.symbol === undefined || my.symbol === null) my.symbol = undefined; my.watched = []; findWatched(); update(); };
    document.addEventListener('tt-watchlist', listed);
    const themed = () => { my.symbol = undefined; my.found = []; find(); update(); };
    document.addEventListener('tt-theme', themed);
    const update = () => {
      const [top, list] = draw(my), box = /** @type {HTMLInputElement | null} */ (topEl.querySelector('.nw-q')), had = document.activeElement === box;
      if (paint(topEl, top)) { const nb = /** @type {HTMLInputElement | null} */ (topEl.querySelector('.nw-q')); if (nb) { nb.value = my.q; if (had) nb.focus(); } }
      paint(listEl, list);
    };
    // A ticker entered: recent stories on it too, from a service that searches news.
    async function lookUp(sym) {
      const svc = (fleet.topo?.services || []).find(s => s.news_search)?.id; if (!svc) return;
      my.looking = true; update();
      try {
        const r = await fetch(`/api/stream/${encodeURIComponent(svc)}/news_search?symbols=${encodeURIComponent(sym)}&limit=50`, { cache: 'no-store' });
        if (r.ok) my.searched = (await r.json()).items || [];
      } catch (_) {}
      my.looking = false; if (my.alive) update();
    }
    const typed = e => { const t = /** @type {HTMLInputElement} */ (e.target); if (t.classList.contains('nw-q')) { my.q = t.value; update(); } };
    const submit = e => {
      const f = /** @type {HTMLElement} */ (e.target); if (!f.classList.contains('nw-search')) return;
      e.preventDefault();
      const q = my.q.trim(); if (/^[A-Za-z][A-Za-z0-9.]{0,9}$/.test(q)) { my.q = q.toUpperCase(); update(); lookUp(my.q); }
    };
    el.addEventListener('input', typed); el.addEventListener('submit', submit);
    // A symbol clicked in a watchlist: its stories (clicked again: all of the list's).
    const narrow = e => {
      // A symbol toggles (clicked again: back to everything); { symbol } sets it.
      const d = /** @type {CustomEvent} */ (e).detail, sym = typeof d === 'string' ? d : d?.symbol;
      if (typeof sym !== 'string') return;
      my.q = typeof d === 'string' && my.q === sym ? '' : sym;
      const box = /** @type {HTMLInputElement | null} */ (topEl.querySelector('.nw-q')); if (box) box.value = my.q;
      update(); if (my.q) lookUp(my.q);
    };
    document.addEventListener('tt-news-symbol', narrow);
    const click = e => {
      const chip = e.target.closest('[data-news-symbol]');
      if (chip) { my.symbol = chip.dataset.newsSymbol || null; update(); return; }
      // A story's symbol searches for it.
      const tag = e.target.closest('[data-news-q]');
      if (tag) { my.q = tag.dataset.newsQ; const box = /** @type {HTMLInputElement | null} */ (topEl.querySelector('.nw-q')); if (box) box.value = my.q; update(); lookUp(my.q); }
    };
    el.addEventListener('click', click);
    update(); find(); findWatched();
    return { update, dispose: () => { my.alive = false; clearInterval(timer); document.removeEventListener('tt-theme', themed); document.removeEventListener('tt-watchlist', listed); el.removeEventListener('click', click); el.removeEventListener('input', typed); el.removeEventListener('submit', submit); document.removeEventListener('tt-news-symbol', narrow); } };
  },
});

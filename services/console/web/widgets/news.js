// News: recent stories from every service streaming news, newest first,
// filtered by symbol with the chips along the top. In a workspace with a
// theme, the Theme chip (chosen first) shows stories on its symbols or
// mentioning its keywords, adding recent stories on its symbols from a
// service that searches news (alpaca-data).
import { defineWidget } from '../lib/widgets.js';
import { newsItems, fleet } from '../lib/fleet.js';
import { esc, ago, unentity } from '../lib/format.js';
import { theme, onTheme } from '../lib/theme.js';

const THEME = '\u0000theme';

function draw(my) {
  const t = theme(), h = 'h1';
  const live = newsItems();
  // The theme's stories: from the live feed, plus those found for its symbols.
  const items = t ? [...new Map([...live, ...(my.found || [])].map(n => [n.id, n])).values()].sort((a, b) => (b.created_at || '').localeCompare(a.created_at || '')) : live;
  if (my.symbol === undefined) my.symbol = t ? THEME : null;
  if (my.symbol === THEME && !t) my.symbol = null;
  if (!items.length) return `<div><${h}>News</${h}><p class="muted">No news service is running, or it has no stories yet.</p></div>`;
  const counts = new Map();
  for (const n of items) for (const sym of n.symbols || []) counts.set(sym, (counts.get(sym) || 0) + 1);
  const top = [...counts.entries()].sort((a, b) => b[1] - a[1]).slice(0, 14).map(([sym]) => sym);
  const shown = my.symbol === THEME && t ? items.filter(n => onTheme(n, t)) : my.symbol ? items.filter(n => (n.symbols || []).includes(my.symbol)) : live;
  const chip = (sym, label) => `<button type="button" class="chip-btn" data-news-symbol="${esc(sym ?? '')}" aria-pressed="${(my.symbol ?? null) === sym}">${esc(label)}</button>`;
  return `<div><${h}>News</${h}><p class="muted" style="margin:0">${items.length} recent stories, newest first.</p></div>
    <div class="chips">${t ? chip(THEME, 'Theme') : ''}${chip(null, 'All')}${top.map(sym => chip(sym, sym)).join('')}</div>
    <div class="news">${shown.slice(0, 100).map(n => `<article class="story">
      <div class="story-meta"><span>${esc(ago(n.created_at))}</span><span>${esc(n.source || '')}${n.author ? ' · ' + esc(n.author) : ''}</span>
        ${(n.symbols || []).slice(0, 8).map(sym => `<span class="tag">${esc(sym)}</span>`).join('')}</div>
      ${/^https?:/.test(n.url || '') ? `<a href="${esc(n.url)}" target="_blank" rel="noopener">${esc(unentity(n.headline))}</a>` : `<b>${esc(unentity(n.headline))}</b>`}
      ${n.summary ? `<p>${esc(unentity(n.summary))}</p>` : ''}
    </article>`).join('') || '<p class="muted" style="padding:16px;margin:0">No stories for this symbol.</p>'}</div>`;
}

defineWidget('news', {
  title: 'News',
  mount(el) {
    const my = { symbol: undefined, found: [], alive: true };
    // Recent stories on the theme's symbols, from a service that searches news.
    async function find() {
      const t = theme(), svc = (fleet.topo?.services || []).find(s => s.news_search)?.id;
      if (!my.alive || !t?.symbols.length || !svc) return;
      try {
        const r = await fetch(`/api/stream/${encodeURIComponent(svc)}/news_search?symbols=${encodeURIComponent(t.symbols.join(','))}&limit=50`, { cache: 'no-store' });
        if (r.ok) { my.found = (await r.json()).items || []; if (my.alive) update(); }
      } catch (_) {}
    }
    const timer = setInterval(find, 60_000);
    const themed = () => { my.symbol = undefined; my.found = []; find(); update(); };
    document.addEventListener('tt-theme', themed);
    const update = () => { const top = el.scrollTop; el.innerHTML = draw(my); el.scrollTop = top; };
    const click = e => {
      const chip = e.target.closest('[data-news-symbol]');
      if (chip) { my.symbol = chip.dataset.newsSymbol || null; update(); }
    };
    el.addEventListener('click', click);
    update(); find();
    return { update, dispose: () => { my.alive = false; clearInterval(timer); document.removeEventListener('tt-theme', themed); el.removeEventListener('click', click); } };
  },
});

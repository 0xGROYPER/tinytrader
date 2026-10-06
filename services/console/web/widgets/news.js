// News: recent stories from every service streaming news, newest first,
// filtered by symbol with the chips along the top.
import { defineWidget } from '../lib/widgets.js';
import { newsItems } from '../lib/fleet.js';
import { esc, ago, unentity } from '../lib/format.js';

function draw(my) {
  const items = newsItems(), h = 'h1';
  if (!items.length) return `<div><${h}>News</${h}><p class="muted">No news service is running, or it has no stories yet.</p></div>`;
  const counts = new Map();
  for (const n of items) for (const sym of n.symbols || []) counts.set(sym, (counts.get(sym) || 0) + 1);
  const top = [...counts.entries()].sort((a, b) => b[1] - a[1]).slice(0, 14).map(([sym]) => sym);
  const shown = my.symbol ? items.filter(n => (n.symbols || []).includes(my.symbol)) : items;
  const chip = (sym, label) => `<button type="button" class="chip-btn" data-news-symbol="${esc(sym ?? '')}" aria-pressed="${(my.symbol ?? null) === sym}">${esc(label)}</button>`;
  return `<div><${h}>News</${h}><p class="muted" style="margin:0">${items.length} recent stories, newest first.</p></div>
    <div class="chips">${chip(null, 'All')}${top.map(sym => chip(sym, sym)).join('')}</div>
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
    const my = { symbol: null };
    const update = () => { const top = el.scrollTop; el.innerHTML = draw(my); el.scrollTop = top; };
    const click = e => {
      const chip = e.target.closest('[data-news-symbol]');
      if (chip) { my.symbol = chip.dataset.newsSymbol || null; update(); }
    };
    el.addEventListener('click', click);
    update();
    return { update, dispose: () => el.removeEventListener('click', click) };
  },
});

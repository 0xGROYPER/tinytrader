// Watchlist: the workspace theme's symbols with price and day change, from a
// service serving stock snapshots (alpaca-data), every few seconds while
// shown. Clicking a symbol makes it the one charts follow.
import { defineWidget, paint } from '../lib/widgets.js';
import { fleet } from '../lib/fleet.js';
import { esc } from '../lib/format.js';
import { theme, focused, setFocus } from '../lib/theme.js';

const EVERY = 5000;

/** The first service serving stock snapshots. */
const source = () => (fleet.topo?.services || []).find(s => s.snapshots)?.id || null;

const pct = x => x == null ? '' : `${x >= 0 ? '+' : ''}${(x * 100).toFixed(2)}%`;
const num = x => x == null ? '' : x.toFixed(x >= 100 ? 2 : x >= 1 ? 2 : 4);

function draw(my) {
  const t = theme();
  if (!t?.symbols.length) return '<p class="muted">This workspace has no theme. Right-click its number in the header and choose Theme… to give it symbols and news keywords.</p>';
  if (!source()) return '<p class="muted">No service serves stock snapshots (alpaca-data).</p>';
  const by = new Map((my.rows || []).map(r => [r.symbol, r]));
  const f = focused();
  const rows = t.symbols.map(s => {
    const r = by.get(s), c = r?.change_pct;
    return `<tr data-wl="${esc(s)}"${s === f ? ' aria-current="true"' : ''}><td class="wl-sym">${esc(s)}</td><td class="num">${esc(num(r?.price))}</td>`
      + `<td class="num ${c == null ? '' : c >= 0 ? 'up' : 'down'}">${esc(r?.change == null ? '' : (r.change >= 0 ? '+' : '') + r.change.toFixed(2))}</td>`
      + `<td class="num ${c == null ? '' : c >= 0 ? 'up' : 'down'}">${esc(pct(c))}</td></tr>`;
  }).join('');
  return `<table class="wl"><thead><tr><th>Symbol</th><th class="num">Last</th><th class="num">Change</th><th class="num">%</th></tr></thead><tbody>${rows}</tbody></table>
    <p class="muted wl-foot">${my.err ? esc(my.err) : my.at ? `IEX prices, updated ${new Date(my.at).toLocaleTimeString()}` : 'Loading…'}</p>`;
}

defineWidget('watchlist', {
  title: 'Watchlist',
  mount(el) {
    const my = { rows: null, err: null, at: 0, alive: true };
    const update = () => { paint(el, draw(my)); };
    async function load() {
      const t = theme(), svc = source();
      if (!my.alive || !t?.symbols.length || !svc) return;
      try {
        const r = await fetch(`/api/stream/${encodeURIComponent(svc)}/snapshots?symbols=${encodeURIComponent(t.symbols.join(','))}`, { cache: 'no-store' });
        const v = await r.json().catch(() => ({}));
        if (!r.ok) throw new Error(v.error || `${r.status} ${r.statusText}`);
        my.rows = v.snapshots; my.err = null; my.at = Date.now();
      } catch (e) { my.err = `Prices unavailable: ${e.message}`; }
      if (my.alive) update();
    }
    const click = e => {
      const row = e.target.closest('[data-wl]'); if (!row) return;
      setFocus(row.dataset.wl);
      document.dispatchEvent(new CustomEvent('tt-focus', { detail: row.dataset.wl }));
      update();
    };
    el.addEventListener('click', click);
    const themed = () => { my.rows = null; my.at = 0; update(); load(); };
    document.addEventListener('tt-theme', themed);
    const timer = setInterval(load, EVERY);
    update(); load();
    return { update, dispose() { my.alive = false; clearInterval(timer); el.removeEventListener('click', click); document.removeEventListener('tt-theme', themed); } };
  },
});

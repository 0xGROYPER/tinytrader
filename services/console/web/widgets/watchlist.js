// Watchlist: a list of symbols with price and the day's change, from a
// service serving stock snapshots (alpaca-data), every few seconds while
// shown. The lists are kept in this browser (lib/watchlists.js): pick one,
// make, rename or delete one, add symbols (several at once) or remove them.
// The open view's theme, when it has symbols, is a list of its own. The News
// widget follows the list picked. Clicking a symbol makes it the one charts
// follow; the % heading sorts by the day's change.
import { defineWidget, paint } from '../lib/widgets.js';
import { fleet } from '../lib/fleet.js';
import { esc } from '../lib/format.js';
import { theme, focused, setFocus, parseSymbols } from '../lib/theme.js';
import { watchlists, picked, setPicked, symbolsOf, addList, renameList, deleteList, addSymbols, removeSymbol, THEME_LIST } from '../lib/watchlists.js';

const EVERY = 5000;

/** The first service serving stock snapshots. */
const source = () => (fleet.topo?.services || []).find(s => s.snapshots)?.id || null;

const pct = x => x == null ? '' : `${x >= 0 ? '+' : '−'}${Math.abs(x * 100).toFixed(2)}%`;
const num = x => x == null ? '' : x.toFixed(x >= 1 ? 2 : 4);

function draw(my) {
  const t = theme(), themed = !!t?.symbols.length, name = picked(themed), mine = name !== THEME_LIST;
  const syms = symbolsOf(name, t?.symbols || []);
  const options = (themed ? `<option value="${THEME_LIST}"${name === THEME_LIST ? ' selected' : ''}>This view's theme</option>` : '')
    + watchlists().map(l => `<option value="${esc(l.name)}"${l.name === name ? ' selected' : ''}>${esc(l.name)} (${l.symbols.length})</option>`).join('');
  const bar = my.edit === 'new' || my.edit === 'rename'
    ? `<form class="wl-bar" data-wl-form="${my.edit}"><input class="wl-in" name="v" value="${esc(my.edit === 'rename' ? name : '')}" placeholder="${my.edit === 'new' ? 'New list name' : 'Name'}" autocomplete="off" spellcheck="false"><button class="btn small primary">${my.edit === 'new' ? 'Make' : 'Rename'}</button><button type="button" class="btn small" data-wl-act="cancel">Cancel</button></form>`
    : my.edit === 'delete'
      ? `<div class="wl-bar wl-ask"><span>Delete the list “${esc(name)}”?</span><button type="button" class="btn small danger" data-wl-act="delete-yes">Delete</button><button type="button" class="btn small" data-wl-act="cancel">Keep</button></div>`
      : `<div class="wl-bar"><select class="wl-pick" aria-label="Watchlist">${options}</select>
        <button type="button" class="tile-btn" data-wl-act="new" title="New list" aria-label="New list">+</button>
        ${mine ? '<button type="button" class="tile-btn" data-wl-act="rename" title="Rename this list" aria-label="Rename this list">✎</button><button type="button" class="tile-btn" data-wl-act="delete" title="Delete this list" aria-label="Delete this list">×</button>' : ''}
        ${mine ? '<form class="wl-add" data-wl-form="add"><input class="wl-in" name="v" placeholder="Add: CCJ, VST" autocomplete="off" spellcheck="false" aria-label="Add symbols"></form>' : ''}</div>`;
  if (!source()) return [bar, '<p class="muted">No service serves stock snapshots (alpaca-data).</p>'];
  if (!syms.length) return [bar, `<p class="muted">${mine ? 'This list is empty: add symbols above.' : 'The theme has no symbols.'}</p>`];
  const by = new Map((my.rows || []).map(r => [r.symbol, r]));
  const f = focused();
  let order = syms;
  if (my.sort) order = [...syms].sort((a, b) => my.sort * ((by.get(b)?.change_pct ?? -Infinity) - (by.get(a)?.change_pct ?? -Infinity)));
  const rows = order.map(s => {
    const r = by.get(s), c = r?.change_pct, cls = c == null ? '' : c >= 0 ? 'up' : 'down';
    return `<tr data-wl="${esc(s)}"${s === f ? ' aria-current="true"' : ''}><td class="wl-sym">${esc(s)}</td><td class="num">${esc(num(r?.price))}</td>`
      + `<td class="num ${cls}">${esc(r?.change == null ? '' : (r.change >= 0 ? '+' : '−') + Math.abs(r.change).toFixed(2))}</td>`
      + `<td class="num ${cls}">${esc(pct(c))}</td>${mine ? `<td class="wl-x"><button type="button" class="sb-x" data-wl-rm="${esc(s)}" aria-label="Remove ${esc(s)}" title="Remove from the list">×</button></td>` : ''}</tr>`;
  }).join('');
  const arrow = my.sort > 0 ? ' ↓' : my.sort < 0 ? ' ↑' : '';
  return [bar, `<table class="wl"><thead><tr><th>Symbol</th><th class="num">Last</th><th class="num">Change</th><th class="num"><button type="button" class="wl-sort" data-wl-act="sort" title="Sort by the day's change">%${arrow}</button></th>${mine ? '<th></th>' : ''}</tr></thead><tbody>${rows}</tbody></table>
    <p class="muted wl-foot">${my.err ? esc(my.err) : my.at ? `IEX prices, updated ${new Date(my.at).toLocaleTimeString()}` : 'Loading…'}</p>`];
}

defineWidget('watchlist', {
  title: 'Watchlist',
  mount(el) {
    const my = { rows: null, err: null, at: 0, alive: true, edit: /** @type {string} */ (''), sort: 0, asked: '' };
    // The bar and the table are painted apart, so a price refresh never
    // replaces a box being typed in.
    el.innerHTML = '<div class="wl-top"></div><div class="wl-main"></div>';
    const top = /** @type {HTMLElement} */ (el.querySelector('.wl-top')), main = /** @type {HTMLElement} */ (el.querySelector('.wl-main'));
    const update = () => {
      const [bar, body] = draw(my);
      if (paint(top, bar)) /** @type {HTMLInputElement | null} */ (top.querySelector('[data-wl-form="new"] input, [data-wl-form="rename"] input'))?.focus();
      paint(main, body);
    };
    const name = () => picked(!!theme()?.symbols.length) || '';
    const current = () => symbolsOf(name(), theme()?.symbols || []);
    async function load() {
      const syms = current(), svc = source();
      if (!my.alive || !syms.length || !svc) return;
      const key = syms.join(',');
      try {
        const r = await fetch(`/api/stream/${encodeURIComponent(svc)}/snapshots?symbols=${encodeURIComponent(key)}`, { cache: 'no-store' });
        const v = await r.json().catch(() => ({}));
        if (!r.ok) throw new Error(v.error || `${r.status} ${r.statusText}`);
        my.rows = v.snapshots; my.err = null; my.at = Date.now(); my.asked = key;
      } catch (e) { my.err = `Prices unavailable: ${e.message}`; }
      if (my.alive) update();
    }
    const changed = () => { update(); if (current().join(',') !== my.asked) load(); };
    const click = e => {
      const t = /** @type {HTMLElement} */ (e.target);
      const rm = /** @type {HTMLElement | null} */ (t.closest('[data-wl-rm]'));
      if (rm) { removeSymbol(name(), rm.dataset.wlRm || ''); return; }
      const act = /** @type {HTMLElement | null} */ (t.closest('[data-wl-act]'))?.dataset.wlAct;
      if (act) {
        if (act === 'sort') my.sort = my.sort > 0 ? -1 : my.sort < 0 ? 0 : 1;
        else if (act === 'delete-yes') { deleteList(name()); my.edit = ''; }
        else if (act === 'cancel') my.edit = '';
        else my.edit = act;
        update(); return;
      }
      const row = /** @type {HTMLElement | null} */ (t.closest('[data-wl]')); if (!row) return;
      setFocus(row.dataset.wl || null);
      document.dispatchEvent(new CustomEvent('tt-focus', { detail: row.dataset.wl }));
      update();
    };
    const submit = e => {
      const form = /** @type {HTMLFormElement} */ (e.target), kind = form.dataset.wlForm; if (!kind) return;
      e.preventDefault();
      const v = /** @type {HTMLInputElement} */ (form.elements.namedItem('v')).value;
      if (kind === 'add') { const syms = parseSymbols(v); if (syms.length) addSymbols(name(), syms); return; }
      if (kind === 'new' && v.trim()) addList(v);
      if (kind === 'rename' && v.trim()) renameList(name(), v);
      my.edit = ''; update();
    };
    const change = e => { const s = /** @type {HTMLSelectElement} */ (e.target); if (s.classList.contains('wl-pick')) setPicked(s.value); };
    el.addEventListener('click', click); el.addEventListener('submit', submit); el.addEventListener('change', change);
    const themed = () => { my.rows = null; my.at = 0; changed(); };
    document.addEventListener('tt-theme', themed);
    document.addEventListener('tt-watchlist', changed);
    const timer = setInterval(load, EVERY);
    update(); load();
    return {
      update,
      dispose() {
        my.alive = false; clearInterval(timer);
        el.removeEventListener('click', click); el.removeEventListener('submit', submit); el.removeEventListener('change', change);
        document.removeEventListener('tt-theme', themed); document.removeEventListener('tt-watchlist', changed);
      },
    };
  },
});

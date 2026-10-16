// Watchlist: a list of symbols with price and the day's change, from a
// service serving stock snapshots (alpaca-data), every few seconds while
// shown. The lists are kept in this browser (lib/watchlists.js): pick one,
// make, rename or delete one, add symbols (several at once) or remove them.
// The open view's theme, when it has symbols, is a list of its own. Each
// Watchlist panel keeps the list it shows (in its layout), so views can show
// different lists; the News widget in the view follows it. A list shown
// (the view opening, or another list picked) points the view's charts at its
// first symbol, unless they're already on one of its symbols. Clicking a
// symbol makes it the one charts follow and narrows the view's News to it
// (the 1W and 1M columns are the change over 5 and 21 sessions, from daily
// closes)
// (clicking it again widens it); ↑/↓ step through the list the same way
// once a row has been clicked (or the table tabbed to). The bell on a row
// sets price alerts for it (lib/alerts.js); the % heading sorts by the day's
// change.
import { defineWidget, paint } from '../lib/widgets.js';
import { fleet, serviceFor } from '../lib/fleet.js';
import { esc, etTime } from '../lib/format.js';
import { theme, focused, setFocus, parseSymbols } from '../lib/theme.js';
import { alertsFor, alertForm, describe } from '../lib/alerts.js';
import { closes, analyse, fmtChange } from '../lib/trend.js';
import { watchlists, picked, setPicked, symbolsOf, addList, renameList, deleteList, addSymbols, removeSymbol, THEME_LIST } from '../lib/watchlists.js';
import { get, stream } from '../lib/cache.js';

const EVERY = 5000;

/** The first service serving stock snapshots. */
const source = () => serviceFor('snapshots') || null;

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
  if (!source()) return [bar, '<p class="muted">No service serves stock snapshots (a <code>snapshots</code> stream).</p>'];
  if (!syms.length) return [bar, `<p class="muted">${mine ? 'This list is empty: add symbols above.' : 'The theme has no symbols.'}</p>`];
  const by = new Map((my.rows || []).map(r => [r.symbol, r]));
  const f = focused();
  // The change over the day (snapshots), a week (5 sessions) and a month (21), from daily closes.
  const ch = (s, k) => k === 'd' ? by.get(s)?.change_pct : my.trend.get(s)?.[k];
  let order = syms;
  if (my.sort) order = [...syms].sort((a, b) => my.sort * ((ch(b, my.sortKey) ?? -Infinity) - (ch(a, my.sortKey) ?? -Infinity)));
  const tone = v => v == null ? '' : v >= 0 ? 'up' : 'down';
  const rows = order.map(s => {
    const r = by.get(s), c = r?.change_pct, cls = c == null ? '' : c >= 0 ? 'up' : 'down';
    const al = alertsFor(s), bell = al.some(a => a.fired) ? 'fired' : al.length ? 'set' : '';
    return `<tr data-wl="${esc(s)}"${s === f ? ' aria-current="true"' : ''}><td class="wl-sym">${esc(s)}<button type="button" class="wl-bell ${bell}" data-wl-alert="${esc(s)}" title="${al.length ? esc(al.map(describe).join('; ')) : 'Set an alert'}" aria-label="Alerts for ${esc(s)}">${bell ? '●' : '○'}</button></td><td class="num">${esc(num(r?.price))}</td>`
      + `<td class="num ${cls}">${esc(r?.change == null ? '' : (r.change >= 0 ? '+' : '−') + Math.abs(r.change).toFixed(2))}</td>`
      + `<td class="num ${cls}">${esc(pct(c))}</td>`
      + ['w', 'm'].map(k => { const v = ch(s, k); return `<td class="num wl-wm ${tone(v)}">${v == null ? '' : esc(fmtChange(v, false))}</td>`; }).join('')
      + `${mine ? `<td class="wl-x"><button type="button" class="sb-x" data-wl-rm="${esc(s)}" aria-label="Remove ${esc(s)}" title="Remove from the list">×</button></td>` : ''}</tr>`;
  }).join('');
  const arrow = k => my.sortKey === k ? (my.sort > 0 ? ' ↓' : my.sort < 0 ? ' ↑' : '') : '';
  const sortBtn = (k, label, title) => `<button type="button" class="wl-sort" data-wl-act="sort" data-wl-key="${k}" title="${title}">${label}${arrow(k)}</button>`;
  return [bar, `<table class="wl" tabindex="0" aria-label="Symbols: ↑↓ to step through"><thead><tr><th>Symbol</th><th class="num">Last</th><th class="num">Change</th><th class="num">${sortBtn('d', '%', "Sort by the day's change")}</th><th class="num">${sortBtn('w', '1W', 'The change over five sessions; click to sort')}</th><th class="num">${sortBtn('m', '1M', 'The change over 21 sessions; click to sort')}</th>${mine ? '<th></th>' : ''}</tr></thead><tbody>${rows}</tbody></table>
    <p class="muted wl-foot">${my.err ? esc(my.err) : my.at ? `IEX prices, updated ${etTime(my.at, true)} ET` : 'Loading…'}</p>`];
}

defineWidget('watchlist', {
  title: 'Watchlist',
  mount(el) {
    const my = { rows: null, err: null, at: 0, alive: true, edit: /** @type {string} */ (''), sort: 0, sortKey: 'd', asked: '', alertFor: '',
      /** Week and month changes by symbol, from daily closes (kept ten minutes by lib/trend.js). */
      trend: /** @type {Map<string, { w: number | null, m: number | null }>} */ (new Map()) };
    // The list this panel shows, from its layout; it becomes the one picked while the view is open.
    let args = {}; try { args = JSON.parse(el.dataset.args || '{}'); } catch (_) {}
    if (args.list && (watchlists().some(l => l.name === args.list) || args.list === THEME_LIST)) setPicked(args.list);
    const keep = () => el.dispatchEvent(new CustomEvent('tt-tab-args', { bubbles: true, detail: { args: { list: picked(!!theme()?.symbols.length) } } }));
    // The bar and the table are painted apart, so a price refresh never
    // replaces a box being typed in.
    el.innerHTML = '<div class="wl-top"></div><div class="wl-al"></div><div class="wl-main"></div>';
    const top = /** @type {HTMLElement} */ (el.querySelector('.wl-top')), main = /** @type {HTMLElement} */ (el.querySelector('.wl-main')), alBox = /** @type {HTMLElement} */ (el.querySelector('.wl-al'));
    const update = () => {
      const [bar, body] = draw(my);
      if (paint(top, bar)) /** @type {HTMLInputElement | null} */ (top.querySelector('[data-wl-form="new"] input, [data-wl-form="rename"] input'))?.focus();
      const hadKeys = main.contains(document.activeElement);
      if (paint(main, body) && hadKeys) /** @type {HTMLElement | null} */ (main.querySelector('table.wl'))?.focus({ preventScroll: true });
      // Its own box: prices refreshing never replace what's being typed.
      if (!alBox.contains(document.activeElement) || !my.alertFor) paint(alBox, my.alertFor ? alertForm(my.alertFor, (my.rows || []).find(r => r.symbol === my.alertFor)?.price, 'data-wl-act="alert-close"') : '');
    };
    // A symbol picked: the charts follow it and the news narrows to it (toggle: a click on it again widens).
    const pick = (s, toggle) => {
      setFocus(s);
      document.dispatchEvent(new CustomEvent('tt-news-symbol', { detail: toggle ? s : { symbol: s } }));
      update();
      /** @type {HTMLElement | null} */ (main.querySelector(`[data-wl="${CSS.escape(s)}"]`))?.scrollIntoView({ block: 'nearest' });
    };
    const keys = e => {
      if (!(e.key === 'ArrowDown' || e.key === 'ArrowUp') || e.altKey || e.metaKey || e.ctrlKey || !main.contains(/** @type {Node} */ (e.target))) return;
      const rows = [...main.querySelectorAll('[data-wl]')].map(r => /** @type {HTMLElement} */ (r).dataset.wl || '');
      if (!rows.length) return;
      e.preventDefault();
      const i = rows.indexOf(focused() || '');
      pick(rows[i < 0 ? 0 : Math.max(0, Math.min(rows.length - 1, i + (e.key === 'ArrowDown' ? 1 : -1)))], false);
    };
    const name = () => picked(!!theme()?.symbols.length) || '';
    const current = () => symbolsOf(name(), theme()?.symbols || []);
    async function load() {
      const syms = current(), svc = source();
      if (!my.alive || !syms.length || !svc) return;
      const key = syms.join(',');
      try {
        const v = await get(stream(svc, 'snapshots', { symbols: key }), { fresh: 1500 });
        my.rows = v.snapshots; my.err = null; my.at = Date.now(); my.asked = key;
      } catch (e) { my.err = `Prices unavailable: ${e.message}`; }
      if (my.alive) update();
      // The week and month, from daily closes (fetched at most every ten minutes).
      closes(fleet, syms).then(({ series }) => {
        for (const sym of syms) { const a = analyse(sym, series.get(sym)?.closes); if (a) my.trend.set(sym, { w: a.w1, m: a.m1 }); }
        if (my.alive) update();
      }).catch(() => {});
    }
    // The charts follow the list shown: its first symbol, unless already on one of them.
    let shown = '';
    const point = () => {
      const n = name(), syms = current();
      if (n === shown || !syms.length) return;
      shown = n;
      const f = focused();
      if (f && syms.includes(f)) return;
      setFocus(syms[0]);
    };
    const changed = () => { point(); update(); if (current().join(',') !== my.asked) load(); };
    const click = e => {
      const t = /** @type {HTMLElement} */ (e.target);
      const rm = /** @type {HTMLElement | null} */ (t.closest('[data-wl-rm]'));
      if (rm) { removeSymbol(name(), rm.dataset.wlRm || ''); return; }
      const bell = /** @type {HTMLElement | null} */ (t.closest('[data-wl-alert]'))?.dataset.wlAlert;
      if (bell) { my.alertFor = my.alertFor === bell ? '' : bell; alBox.innerHTML = ''; update(); /** @type {HTMLInputElement | null} */ (alBox.querySelector('input'))?.focus(); return; }
      if (t.closest('[data-al-rm], [data-al-rearm]')) return;
      const act = /** @type {HTMLElement | null} */ (t.closest('[data-wl-act]'))?.dataset.wlAct;
      if (act === 'alert-close') { my.alertFor = ''; update(); return; }
      if (act) {
        if (act === 'sort') {
          // A new column sorts by it, highest first; the same one again flips, then unsorts.
          const k = /** @type {HTMLElement | null} */ (t.closest('[data-wl-key]'))?.dataset.wlKey || 'd';
          if (k !== my.sortKey) { my.sortKey = k; my.sort = 1; } else my.sort = my.sort > 0 ? -1 : my.sort < 0 ? 0 : 1;
        }
        else if (act === 'delete-yes') { deleteList(name()); my.edit = ''; }
        else if (act === 'cancel') my.edit = '';
        else my.edit = act;
        update(); return;
      }
      const row = /** @type {HTMLElement | null} */ (t.closest('[data-wl]')); if (!row) return;
      // Then ↑/↓ carry on from it.
      /** @type {HTMLElement | null} */ (main.querySelector('table.wl'))?.focus({ preventScroll: true });
      pick(row.dataset.wl || '', true);
    };
    const submit = e => {
      const form = /** @type {HTMLFormElement} */ (e.target), kind = form.dataset.wlForm; if (!kind) return;
      e.preventDefault();
      const v = /** @type {HTMLInputElement} */ (form.elements.namedItem('v')).value;
      if (kind === 'add') { const syms = parseSymbols(v); if (syms.length) addSymbols(name(), syms); return; }
      if (kind === 'new' && v.trim()) { addList(v); keep(); }
      if (kind === 'rename' && v.trim() && renameList(name(), v)) keep();
      my.edit = ''; update();
    };
    const change = e => { const s = /** @type {HTMLSelectElement} */ (e.target); if (s.classList.contains('wl-pick')) { setPicked(s.value); keep(); } };
    el.addEventListener('click', click); el.addEventListener('submit', submit); el.addEventListener('change', change); el.addEventListener('keydown', keys);
    document.addEventListener('tt-alerts', update);
    const themed = () => { my.rows = null; my.at = 0; changed(); };
    document.addEventListener('tt-theme', themed);
    document.addEventListener('tt-watchlist', changed);
    const timer = setInterval(load, EVERY);
    point(); update(); load();
    return {
      update,
      dispose() {
        my.alive = false; clearInterval(timer);
        el.removeEventListener('click', click); el.removeEventListener('submit', submit); el.removeEventListener('change', change); el.removeEventListener('keydown', keys);
        document.removeEventListener('tt-alerts', update);
        document.removeEventListener('tt-theme', themed); document.removeEventListener('tt-watchlist', changed);
      },
    };
  },
});

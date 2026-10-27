// Lists: one of the deployment's external lists (CONSOLE_LISTS, served at
// /api/lists): a source's rows in the columns configured for it, when the
// console last read it, and why the last read failed if it did. Display only:
// the console reads the sources; this draws what it serves. With several
// lists, a picker chooses one, kept with the panel (args.list).
import { defineWidget, paint } from '../lib/widgets.js';
import { esc, etTime } from '../lib/format.js';

const EVERY = 5000;

/** A cell's text: a string as it is, anything else as JSON. */
const cell = v => v == null ? '' : typeof v === 'string' ? v : JSON.stringify(v);

/**
 * The panel's HTML for the list picked (or the first), every value escaped.
 * @param {{ lists: any[] | null, pick: string, err: string, at: number }} my
 *   `lists` as /api/lists last answered, read at `at` (ms); `err` why the
 *   last read of /api/lists failed.
 */
export function draw(my) {
  const err = my.err ? `<p class="msg-err">${esc(my.err)}</p>` : '';
  if (!my.lists) return err || '<p class="muted">Loading…</p>';
  if (!my.lists.length) return `${err}<p class="muted">No lists are configured (CONSOLE_LISTS).</p>`;
  const l = my.lists.find(x => x.id === my.pick) || my.lists[0], cols = l.columns || [];
  const pick = my.lists.length > 1 ? `<div class="field" style="max-width:320px;margin-bottom:8px"><select data-ls-pick aria-label="List">${my.lists.map(x => `<option value="${esc(x.id)}"${x === l ? ' selected' : ''}>${esc(x.label)}</option>`).join('')}</select></div>` : '';
  const rows = (l.rows || []).map(r => `<tr>${cols.map(c => `<td class="mono">${esc(cell(r[c.key]))}</td>`).join('')}</tr>`).join('');
  const read = l.age_ms != null;
  const failed = l.error ? `<p class="msg-err">${esc(l.error)}${read ? '. Showing what it last listed.' : ''}</p>` : '';
  return `${pick}<div class="box scroll"><table><thead><tr>${cols.map(c => `<th>${esc(c.label)}</th>`).join('')}</tr></thead>
    <tbody>${rows || (read ? `<tr><td colspan="${cols.length}" class="muted">Nothing listed.</td></tr>` : '')}</tbody></table></div>${failed}${err}
    <p class="muted" style="font-size:11px;margin:0">${read ? `Updated ${etTime(my.at - l.age_ms, true)} ET` : 'Not read yet.'}</p>`;
}

defineWidget('lists', {
  title: 'Lists',
  mount(el) {
    const my = { lists: /** @type {any[] | null} */ (null), pick: '', err: '', at: 0, alive: true, titled: false };
    try { const a = JSON.parse(el.dataset.args || '{}'); if (typeof a.list === 'string') my.pick = a.list; } catch (_) {}
    const keep = () => el.dispatchEvent(new CustomEvent('tt-tab-args', { bubbles: true, detail: { args: { list: my.pick }, title: my.lists?.find(x => x.id === my.pick)?.label } }));
    // Not while the picker is open: redrawing would close it.
    const update = () => { if (document.activeElement?.closest('[data-ls-pick]') && el.contains(document.activeElement)) return; paint(el, draw(my)); };
    async function load() {
      // Not while its panel is out of sight (another tab in front).
      if (!my.alive || (my.lists && el.offsetParent === null)) return;
      try {
        const r = await fetch('/api/lists', { cache: 'no-store' });
        const v = await r.json().catch(() => ({}));
        if (!r.ok) throw new Error(v.error || `${r.status}`);
        my.lists = v.lists || []; my.at = Date.now(); my.err = '';
        if (!my.titled && my.lists.length) { my.titled = true; my.pick = (my.lists.find(x => x.id === my.pick) || my.lists[0]).id; keep(); }
      } catch (e) { my.err = `The lists could not be read: ${e.message}`; }
      if (my.alive) update();
    }
    const change = e => {
      const t = /** @type {HTMLSelectElement} */ (e.target);
      if (!t.matches('[data-ls-pick]')) return;
      my.pick = t.value; t.blur(); update(); keep();
    };
    el.addEventListener('change', change);
    const timer = setInterval(load, EVERY);
    update(); load();
    return { dispose() { my.alive = false; clearInterval(timer); el.removeEventListener('change', change); } };
  },
});

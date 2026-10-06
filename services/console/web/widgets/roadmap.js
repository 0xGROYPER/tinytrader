// Roadmap: the console's reminders, after iOS Reminders. Lists on the left
// (with All, Flagged and Completed), the chosen list's items on the right:
// tap the circle to complete, edit a title or notes in place, flag, add with
// New Reminder. Saved by the console (/api/reminders); every panel shows the
// same document, each with its own list chosen.
import { defineWidget } from '../lib/widgets.js';
import { esc } from '../lib/format.js';

const rem = { doc: null, err: null, loaded: 0 };
const panels = new Set();
async function remApi(change) {
  const r = await fetch('/api/reminders', change ? { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify(change) } : { cache: 'no-store' });
  let v = null; try { v = await r.json(); } catch (_) {}
  if (!r.ok) throw new Error(v?.error || (r.status === 404 ? 'Reminders are not configured on this console.' : `${r.status} ${r.statusText}`));
  return v;
}
async function remLoad() {
  try { rem.doc = await remApi(); rem.err = null; } catch (e) { rem.err = e.message; }
  rem.loaded = Date.now(); for (const p of panels) p.draw();
}
async function remChange(change) {
  try { rem.doc = await remApi(change); rem.err = null; } catch (e) { rem.err = e.message; }
  for (const p of panels) p.draw();
}
// Kept current, but never under someone typing.
setInterval(() => { if (panels.size && ![...panels].some(p => p.typing()) && Date.now() - rem.loaded > 15000) remLoad(); }, 5000);
const FLAG = '<svg width="14" height="14" viewBox="0 0 16 16" fill="currentColor" aria-hidden="true"><path d="M3 1.5v13h1.5V9.6h7.2l-1.4-3.05L11.7 3.5H4.5V1.5z"/></svg>';
function remHtml(ui) {
  if (rem.err && !rem.doc) return `<p class="msg-err">${esc(rem.err)}</p>`;
  if (!rem.doc) return '<p class="muted">Loading reminders…</p>';
  const { lists, items } = rem.doc;
  const open = items.filter(i => !i.done);
  const count = id => open.filter(i => i.list === id).length;
  const list = lists.find(l => l.id === ui.list);
  if (ui.list !== 'all' && ui.list !== 'flagged' && ui.list !== 'done' && !list) ui.list = 'all';
  const smart = [['all', 'All', open.length, '#5F6B7A'], ['flagged', 'Flagged', open.filter(i => i.flagged).length, '#FF9500'], ['done', 'Completed', items.length - open.length, '#8E8E93']];
  const side = `<aside class="rem-side">
    <div class="rem-smart">${smart.map(([id, name, n, c]) => `<button type="button" class="rem-card" data-rem-list="${id}" aria-pressed="${ui.list === id}"><i style="background:${c}"></i><b>${n}</b><span>${name}</span></button>`).join('')}</div>
    <div class="rem-h">My Lists</div>
    ${lists.map(l => `<button type="button" class="rem-l" data-rem-list="${l.id}" aria-pressed="${ui.list === l.id}"><i style="background:${esc(l.color)}"></i><span>${esc(l.name)}</span><em>${count(l.id)}</em></button>`).join('')}
    ${ui.adding ? '<input class="rem-newlist" data-rem-newlist placeholder="List name" autocomplete="off">' : '<button type="button" class="rem-add" data-rem-addlist>+ Add List</button>'}
  </aside>`;
  const picker = `<select class="rem-pick" data-rem-pick aria-label="List">${smart.map(([id, name]) => `<option value="${id}"${ui.list === id ? ' selected' : ''}>${name}</option>`).join('')}${lists.map(l => `<option value="${l.id}"${ui.list === l.id ? ' selected' : ''}>${esc(l.name)}</option>`).join('')}</select>`;
  const color = list?.color || (ui.list === 'flagged' ? '#FF9500' : ui.list === 'done' ? '#8E8E93' : 'var(--fg)');
  const title = list?.name || { all: 'All', flagged: 'Flagged', done: 'Completed' }[ui.list];
  let shown = ui.list === 'all' ? items : ui.list === 'flagged' ? items.filter(i => i.flagged) : ui.list === 'done' ? items.filter(i => i.done) : items.filter(i => i.list === ui.list);
  const doneHere = shown.filter(i => i.done).length;
  if (ui.list !== 'done' && !ui.showDone) shown = shown.filter(i => !i.done);
  const listOf = i => lists.find(l => l.id === i.list);
  const row = i => `<div class="rem-item${i.done ? ' done' : ''}" data-rem-item="${i.id}">
    <button type="button" class="rem-check" data-rem-check="${i.id}" style="--c:${esc(listOf(i)?.color || '#8E8E93')}" aria-pressed="${i.done}" aria-label="${i.done ? 'Mark not done' : 'Complete'}"></button>
    <div class="rem-body">
      <input class="rem-title" data-rem-field="title" value="${esc(i.title)}" aria-label="Title">
      <input class="rem-notes" data-rem-field="notes" value="${esc(i.notes || '')}" placeholder="Notes" aria-label="Notes">
      ${ui.list === 'all' || ui.list === 'flagged' || ui.list === 'done' ? `<span class="rem-in" style="color:${esc(listOf(i)?.color || 'inherit')}">${esc(listOf(i)?.name || '')}</span>` : ''}
    </div>
    <button type="button" class="rem-flag${i.flagged ? ' on' : ''}" data-rem-flag="${i.id}" aria-pressed="${i.flagged}" aria-label="Flag" title="Flag">${FLAG}</button>
    <button type="button" class="rem-del" data-rem-del="${i.id}" aria-label="Delete" title="Delete">${CLOSE}</button>
  </div>`;
  // New reminders go to the chosen list, or from All, Flagged and Completed to the first list.
  const target = list || lists[0], canAdd = !!target;
  return `${side}<section class="rem-main">${picker}
    <div class="rem-top"><h2 style="color:${color}">${esc(title)}</h2><span class="rem-n" style="color:${color}">${shown.filter(i => !i.done).length || ''}</span></div>
    ${rem.err ? `<p class="msg-err">${esc(rem.err)}</p>` : ''}
    <div class="rem-items">${shown.map(row).join('') || (canAdd ? '' : '<p class="muted">Nothing here.</p>')}</div>
    ${canAdd ? `<div class="rem-new"><span style="color:${esc(target.color)}">+</span><input data-rem-new placeholder="New Reminder${list ? '' : ' in ' + esc(target.name)}" autocomplete="off"></div>` : ''}
    ${ui.list !== 'done' && doneHere ? `<button type="button" class="rem-showdone" data-rem-showdone>${ui.showDone ? 'Hide' : 'Show'} completed (${doneHere})</button>` : ''}
  </section>`;
}
const CLOSE = '<svg width="14" height="14" viewBox="0 0 14 14" fill="none" stroke="currentColor" stroke-width="1.5" aria-hidden="true"><path d="M3 3l8 8M11 3l-8 8"/></svg>';
const listId = v => /^\d+$/.test(v) ? +v : v;

defineWidget('roadmap', {
  title: 'Roadmap',
  mount(el) {
    const ui = { list: 'all', showDone: false, adding: false };
    el.innerHTML = '<div class="rem"></div>';
    const box = el.firstChild;
    const P = {
      typing: () => box.contains(document.activeElement),
      draw() {
        const f = document.activeElement, keep = f && box.contains(f) ? { nw: f.matches('[data-rem-new]'), nl: f.matches('[data-rem-newlist]'), item: f.closest('[data-rem-item]')?.dataset.remItem, field: f.dataset.remField, pos: f.selectionStart } : null;
        box.innerHTML = remHtml(ui);
        if (keep) {
          const t = keep.nw ? box.querySelector('[data-rem-new]') : keep.nl ? box.querySelector('[data-rem-newlist]') : box.querySelector(`[data-rem-item="${keep.item}"] [data-rem-field="${keep.field}"]`);
          if (t) { t.focus(); try { t.setSelectionRange(keep.pos, keep.pos); } catch (_) {} }
        }
      },
    };
    box.addEventListener('click', e => {
      const t = e.target.closest('[data-rem-list],[data-rem-check],[data-rem-flag],[data-rem-del],[data-rem-addlist],[data-rem-showdone]');
      if (!t) return;
      if (t.dataset.remList) { ui.list = listId(t.dataset.remList); P.draw(); return; }
      if (t.dataset.remCheck) { const i = rem.doc.items.find(x => x.id === +t.dataset.remCheck); remChange({ op: 'update_item', id: i.id, done: !i.done }); return; }
      if (t.dataset.remFlag) { const i = rem.doc.items.find(x => x.id === +t.dataset.remFlag); remChange({ op: 'update_item', id: i.id, flagged: !i.flagged }); return; }
      if (t.dataset.remDel) { remChange({ op: 'delete_item', id: +t.dataset.remDel }); return; }
      if ('remAddlist' in t.dataset) { ui.adding = true; P.draw(); box.querySelector('[data-rem-newlist]')?.focus(); return; }
      if ('remShowdone' in t.dataset) { ui.showDone = !ui.showDone; P.draw(); }
    });
    box.addEventListener('change', e => {
      if (e.target.dataset.remPick !== undefined) { ui.list = listId(e.target.value); P.draw(); return; }
      const f = e.target.dataset.remField; if (!f) return;
      const id = +e.target.closest('[data-rem-item]').dataset.remItem, item = rem.doc.items.find(x => x.id === id);
      if ((item[f] || '') !== e.target.value) remChange({ op: 'update_item', id, [f]: e.target.value });
    });
    box.addEventListener('keydown', e => {
      if (e.key !== 'Enter') return;
      if (e.target.matches('[data-rem-new]') && e.target.value.trim()) {
        const title = e.target.value; e.target.value = '';
        const list = typeof ui.list === 'number' ? ui.list : rem.doc.lists[0]?.id;
        remChange({ op: 'add_item', list, title }); return;
      }
      if (e.target.matches('[data-rem-newlist]')) { const name = e.target.value.trim(); ui.adding = false; if (name) remChange({ op: 'add_list', name }); else P.draw(); return; }
      if (e.target.dataset.remField) e.target.blur();
    });
    box.addEventListener('focusout', e => { if (e.target.matches('[data-rem-newlist]') && !e.target.value.trim()) { ui.adding = false; setTimeout(P.draw); } });
    panels.add(P); P.draw(); remLoad();
    return { dispose() { panels.delete(P); } };
  },
});

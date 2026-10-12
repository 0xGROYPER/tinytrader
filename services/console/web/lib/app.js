// The console's shell: the header (workspace numbers, the bell, themes),
// workspaces on Dockview, the palette and the switcher, the docs pages, and
// polling the fleet. The widgets are modules under ../widgets (decision 0010).
import { esc, bytes, duration, fmt, ago } from './format.js';
import { fleet, svcState, metric, statusOf, statusText, writerOf, nodesById, uptimeOf, subOf, newsItems } from './fleet.js';
import { widget, widgetNames } from './widgets.js';
import { drawCharts } from './charts.js';
import { prof, timed, statePoll, profileWanted, startProfile, stopProfile } from './profile.js';
import { folderLabel, groupLabel, collections } from './docs.js';
// The widgets, in the order the palette and Split offer them.
import { showNode } from '../widgets/graph.js';
import '../widgets/schedule.js';
import '../widgets/services.js';
import '../widgets/shell.js';
import '../widgets/prices.js';
import '../widgets/candles.js';
import '../widgets/accounts.js';
import '../widgets/news.js';
import '../widgets/calendar.js';
import '../widgets/data.js';
import '../widgets/jobs.js';
import '../widgets/queue.js';
import '../widgets/storage.js';
import '../widgets/reminders.js';
import '../widgets/search.js';
import '../widgets/docs.js';
import '../widgets/watchlist.js';
import '../widgets/ivmap.js';
import '../widgets/correlation.js';
import '../widgets/earnings.js';
import '../widgets/notebook.js';
import '../widgets/terminal.js';
import '../widgets/earningsweek.js';
import '../widgets/spreads.js';
import '../widgets/screener.js';
import { setTheme, PRESETS, parseSymbols, parseKeywords, LAYERS, DEFAULT_LAYERS } from './theme.js';
const state = { openGroups: new Set(), navKey: '', tab: 'workspace', lastOk: 0, collections: [], doc: null };
/** An element by ID, typed loosely enough for forms and buttons alike. @type {(id: string) => HTMLInputElement} */
const $ = id => /** @type {HTMLInputElement} */ (document.getElementById(id));
/** An event's target as an element. @type {(e: Event) => HTMLInputElement} */
const tgt = e => /** @type {HTMLInputElement} */ (e.target);
/** The first element matching a selector, typed as HTML. @type {(sel: string, root?: ParentNode) => HTMLElement | null} */
const one = (sel, root = document) => root.querySelector(sel);
/** The element an event happened in that matches a selector, if any. @type {(e: Event, sel: string) => HTMLElement | null} */
const near = (e, sel) => tgt(e).closest?.(sel) ?? null;
/** A palette command. @typedef {{ label: string, run: () => void, always?: boolean }} Command */










// ---------- document collections ----------
async function loadCollections() {
  try {
    state.collections = await collections();
    state.navKey = '';
    updateCollectionsNav();
  } catch (_) {}
}
// The Docs section, after Databento's sidebar: folders form a tree (a
// group `tinytrader/decisions` sits inside `tinytrader`), opening a folder closes
// its siblings and opens its first document, and the open document's folders
// are marked. With one collection its folders are the top level; with
// several, each collection is a top-level folder.
const CHEV = '<span class="chev"><svg width="20" height="20" viewBox="0 0 20 20" fill="none" aria-hidden="true"><path d="M8.3 13.3 11.7 10 8.3 6.7" stroke="currentColor" stroke-width="1.4" stroke-linecap="square"/></svg></span>';
function docTree(c) {
  const root = { path: '', docs: [], kids: new Map() };
  for (const d of c.docs) {
    let node = root, path = '';
    for (const seg of d.group.split('/')) {
      path = path ? path + '/' + seg : seg;
      if (!node.kids.has(seg)) node.kids.set(seg, { path, name: seg, docs: [], kids: new Map() });
      node = node.kids.get(seg);
    }
    node.docs.push(d);
  }
  return root;
}
const folderKey = (c, path) => c.id + ':' + path;
const parentOf = k => k.includes('/') ? k.slice(0, k.lastIndexOf('/')) : k.slice(0, k.indexOf(':') + 1);
// Open a folder and close its siblings (and everything inside them).
function openFolder(key) {
  const parent = parentOf(key);
  for (const k of [...state.openGroups]) {
    const top = k.split('/').slice(0, key.split('/').length).join('/');
    if (top !== key && parentOf(top) === parent) state.openGroups.delete(k);
  }
  state.openGroups.add(key);
}
function closeFolder(key) {
  for (const k of [...state.openGroups]) if (k === key || k.startsWith(key + '/')) state.openGroups.delete(k);
}
function revealDoc(c, d) {
  let path = '';
  for (const seg of d.group.split('/')) { path = path ? path + '/' + seg : seg; openFolder(folderKey(c, path)); }
}
function updateCollectionsNav() {
  const key = [state.tab, state.doc, [...state.openGroups].join(','), state.collections.length].join('|');
  if (key === state.navKey) return;
  state.navKey = key;
  if (!state.collections.length) { $('nav-collections').innerHTML = ''; return; }
  const open = currentCollection();
  const current = open && state.doc !== null ? open.docs.find(d => d.index === state.doc) : null;
  const rows = (c, node, lvl) => {
    let out = node.docs.map(d => `<button type="button" class="doc-item${lvl ? ' sub' : ''}" style="--lvl:${lvl}" data-col="${esc(c.id)}" data-doc="${d.index}" title="${esc(d.title)}"${d === current ? ' aria-current="page"' : ''}><span class="chev"></span><span class="lbl">${esc(d.title)}</span></button>`).join('');
    for (const kid of node.kids.values()) {
      const k = folderKey(c, kid.path), expanded = state.openGroups.has(k);
      const trail = current && c === open && (current.group === kid.path || current.group.startsWith(kid.path + '/'));
      out += `<button type="button" class="doc-item${lvl ? ' sub' : ''}${trail ? ' trail' : ''}" style="--lvl:${lvl}" data-col="${esc(c.id)}" data-folder="${esc(kid.path)}" aria-expanded="${expanded}">${CHEV}<span class="lbl">${esc(folderLabel(kid.name))}</span></button>`;
      if (expanded) out += rows(c, kid, lvl + 1);
    }
    return out;
  };
  let items;
  if (state.collections.length === 1) {
    const c = state.collections[0];
    // On a phone the tree is hidden, so the collection's own page stands in.
    items = `<button type="button" class="mobile-only" data-tab="col:${esc(c.id)}"><span class="lbl">${esc(c.name)}</span></button>` + rows(c, docTree(c), 0);
  } else {
    items = state.collections.map(c => {
      const expanded = c === open;
      return `<button type="button" data-tab="col:${esc(c.id)}" aria-expanded="${expanded}">${CHEV}<span class="lbl">${esc(c.name)}</span></button>` + (expanded ? rows(c, docTree(c), 1) : '');
    }).join('');
  }
  const here = (on) => on ? ' aria-current="page"' : '';
  $('nav-collections').innerHTML = `<div class="nav-sec"><button type="button" class="nav-label nav-home" data-docs-home${here(state.tab.startsWith('col:') && state.doc === null)}>Docs</button><div class="nav-items">${items}</div></div>`;
}
const currentCollection = () => state.collections.find(c => 'col:' + c.id === state.tab);
function collectionView() {
  const c = currentCollection();
  if (!c) return '<div class="empty">Collection not found.</div>';
  if (!c.docs.length) return `<div><h1>${esc(c.name)}</h1><p class="muted">No documents yet. Add Markdown files to this collection's folders.</p></div>`;
  const groups = [...new Set(c.docs.map(d => d.group))];
  // The docs home is the Docs widget: the mark, search, and where to start.
  if (state.doc === null) return '<div class="page-widget docs-page-home"></div>';
  return `<div><button type="button" class="back mobile-only" data-doc="">← All ${esc(c.name.toLowerCase())}</button>
    <article class="prose" id="doc-body"><p class="muted">Loading…</p></article></div>`;
}
async function wireDoc() {
  const c = currentCollection();
  if (!c || state.doc === null) return;
  const body = $('doc-body'); if (!body) return;
  try {
    const r = await fetch(`/api/collections/${encodeURIComponent(c.id)}/${state.doc}`, { cache: 'no-store' });
    const d = await r.json();
    body.innerHTML = `<div class="path">${esc(d.path)}</div>${d.html}`;
  } catch (_) { body.innerHTML = '<p class="muted">Could not load this document.</p>'; }
}
function openDoc(index) {
  state.doc = index; state.navigated = true;
  const c = currentCollection();
  const d = c?.docs.find(x => x.index === index);
  if (d) revealDoc(c, d);
  try { history.replaceState(null, '', '#' + (c ? c.id + (index === null ? '' : '-' + index) : '')); } catch (_) {}
  render(); window.scrollTo(0, 0);
}
// Relative links between Markdown documents open inside the console.
function resolveDocLink(href) {
  const c = currentCollection(); if (!c || state.doc === null) return null;
  const from = c.docs.find(d => d.index === state.doc); if (!from) return null;
  const [file] = href.split('#');
  if (!file.endsWith('.md')) return null;
  const parts = from.path.split('/').slice(0, -1);
  for (const seg of file.split('/')) { if (seg === '..') parts.pop(); else if (seg && seg !== '.') parts.push(seg); }
  const target = parts.join('/');
  return c.docs.find(d => d.path === target) ?? null;
}





// One view's HTML, for the page or for a workspace tile.

// Workspaces (decision 0008): Dockview manages the panels; each panel shows
// one view. Workspaces are named layouts in groups, switched from the header
// tabs (the current group's), the switcher (⌥W) and keys. Only the open
// workspace runs: a panel renders while it is shown, and others are saved
// layouts, which is all the switcher's previews read.
// Every widget is a module registered with its title (decision 0010).
const VIEWS = Object.fromEntries(widgetNames().map(n => [n, widget(n).title]));
const tileIcon = {
  max: '<svg width="15" height="15" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5" aria-hidden="true"><path d="M9.5 2.5h4v4M6.5 13.5h-4v-4M13.5 2.5 9 7M2.5 13.5 7 9"/></svg>',
  restore: '<svg width="15" height="15" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5" aria-hidden="true"><path d="M13.5 6.5h-4v-4M2.5 9.5h4v4M9.5 6.5 14 2M6.5 9.5 2 14"/></svg>',
  row: '<svg width="16" height="16" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.4" aria-hidden="true"><rect x="1.5" y="2.5" width="13" height="11" rx="1.5"/><path d="M8 2.5v11"/></svg>',
  col: '<svg width="16" height="16" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.4" aria-hidden="true"><rect x="1.5" y="2.5" width="13" height="11" rx="1.5"/><path d="M1.5 8h13"/></svg>',
  add: '<svg width="15" height="15" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.6" aria-hidden="true"><path d="M8 3v10M3 8h10"/></svg>',
  close: '<svg width="14" height="14" viewBox="0 0 14 14" fill="none" stroke="currentColor" stroke-width="1.5" aria-hidden="true"><path d="M3 3l8 8M11 3l-8 8"/></svg>',
};
// Templates and new workspaces start from a simple tree of splits.
const LAYOUTS = {
  Market: () => ({ dir: 'row', ratio: 0.68, a: { dir: 'col', ratio: 0.5, a: { view: 'prices' }, b: { view: 'accounts' } }, b: { dir: 'col', ratio: 0.6, a: { view: 'news' }, b: { view: 'calendar' } } }),
  Operations: () => ({ dir: 'row', ratio: 0.62, a: { dir: 'col', ratio: 0.5, a: { view: 'overview' }, b: { view: 'schedule' } }, b: { dir: 'col', ratio: 0.55, a: { view: 'services' }, b: { view: 'shell' } } }),
  Data: () => ({ dir: 'row', ratio: 0.55, a: { view: 'jobs' }, b: { dir: 'col', ratio: 0.55, a: { view: 'data' }, b: { view: 'storage' } } }),
};
// A themed workspace's first layout: watchlist (and correlations) over the
// chart (and IV), beside news over the calendar (and earnings).
const THEMED = () => ({ dir: 'row', ratio: 0.5,
  a: { dir: 'col', ratio: 0.45, a: { tabs: ['watchlist', 'correlation'] }, b: { tabs: ['candles', 'ivmap'] } },
  b: { dir: 'col', ratio: 0.5, a: { view: 'news' }, b: { tabs: ['calendar', 'earnings'] } } });
// A ticker searched on Search: its candles over its earnings, news beside them.
const TICKER = () => ({ dir: 'row', ratio: 0.58, a: { dir: 'col', ratio: 0.6, a: { view: 'candles' }, b: { view: 'earnings' } }, b: { view: 'news' } });
// Ready-made workspaces, each installed (or brought up to date) once per
// version of it, so a new one never resets the others.
/** @type {Record<string, [number, () => { tree: object, theme?: object }]>} */
const READY = {
  'Oil & Iran': [1, () => ({ tree: THEMED(), theme: PRESETS['Oil & Iran'] })],
  Earnings: [1, () => ({ tree: { dir: 'row', ratio: 0.6, a: { view: 'earnings' }, b: { dir: 'col', ratio: 0.45, a: { view: 'calendar' }, b: { tabs: ['ivmap', 'candles'] } } } })],
  Notebook: [1, () => ({ tree: { dir: 'row', ratio: 0.72, a: { view: 'notebook' }, b: { dir: 'col', ratio: 0.5, a: { tabs: ['watchlist', 'earnings'] }, b: { tabs: ['candles', 'ivmap'] } } } })],
};
const DV = globalThis['dockview-core'];
let dock = null, wsFocus = null, panelSeq = 0;
const panelBody = new Map(); // panel id → its content element
const panelId = () => 'p' + Date.now().toString(36) + (panelSeq++);
const openViews = () => (dock?.panels || []).map(p => p.params?.view);
const activePanel = () => dock?.activePanel || dock?.panels[0];
function workspaceView() { return '<div class="ws dv-tt" id="ws-root"></div>'; }
// A widget is mounted into its panel and kept as an instance.
const mounted = new Map(); // panel body → widget instance
// In profile mode each mount, update and dispose is timed (lib/profile.js).
let pfSeq = 0;
const call = (op, body, fn) => prof.on ? timed(op, body.dataset.view || '?', body.dataset.pfKey ||= `${body.dataset.view}#${++pfSeq}`, fn) : fn();
function unmount(body) { const m = mounted.get(body); if (m?.dispose) call('dispose', body, () => m.dispose()); mounted.delete(body); }
// Workspaces saved before Search was called that still say `home`.
const viewOf = v => v === 'home' ? 'search' : v;
function renderPanel(body, view) {
  view = viewOf(view);
  unmount(body);
  body.dataset.view = view;
  const w = widget(view);
  if (w) { body.innerHTML = ''; delete body.dataset.pfKey; mounted.set(body, call('mount', body, () => w.mount(body)) || {}); return; }
  body.innerHTML = `<p class="muted" style="padding:16px">There's no widget called “${esc(view)}” any more. Close this panel, or show another widget here from the palette (⌘K).</p>`;
}
// On each refresh, only visible panels without state of their own redraw.
function refreshTiles() {
  for (const body of panelBody.values()) {
    if (!body.isConnected) continue;
    const m = mounted.get(body);
    if (m?.update) call('update', body, () => m.update());
  }
}
// Build a template tree into the dock: each split's second half takes the
// side of the whole area, then the first half subdivides what's left.
// A leaf is one view, or several as tabs (`tabs: [...]`, the first shown).
function buildTree(tree) {
  const leaf = n => n.view || n.tabs;
  const first = n => leaf(n) ? n : first(n.a);
  const add = (view, ref, dir) => dock.addPanel({ id: panelId(), component: 'view', params: { view: viewOf(view) }, title: VIEWS[viewOf(view)] || view, ...(ref ? { position: { referencePanel: ref, direction: dir } } : {}) });
  const place = (n, ref, dir) => {
    const [v, ...more] = n.tabs || [n.view];
    const p = add(v, ref, dir);
    for (const m of more) add(m, p, 'within');
    if (more.length) p.api.setActive();
    return p;
  };
  const fill = (n, p) => { if (leaf(n)) return; const q = place(first(n.b), p, n.dir === 'row' ? 'right' : 'below'); fill(n.b, q); fill(n.a, p); };
  const p = place(first(tree)); fill(tree, p);
}
/** A pending save of the layout after it changes. */
let layoutSaveTimer = 0;
function mountWorkspace() {
  // Its widgets follow the workspace's theme, if it has one.
  setTheme(lay.list.find(l => l.name === lay.current)?.theme || null);
  for (const body of mounted.keys()) unmount(body);
  dock?.dispose(); dock = null; panelBody.clear();
  const el = $('ws-root'); if (!el) return;
  if (!DV) { el.innerHTML = '<p class="msg-err" style="padding:16px">The window manager did not load (/vendor/dockview-core).</p>'; return; }
  dock = new DV.DockviewComponent(el, {
    className: 'dv-tt',
    // Dockview's light theme for its sizes; its colours come from our palette (styles below).
    theme: { name: 'light', className: 'dockview-theme-light', colorScheme: 'light' },
    createComponent: () => {
      const body = document.createElement('div'); body.className = 'tile-body';
      return {
        element: body,
        init(p) {
          panelBody.set(p.api.id, body); renderPanel(body, p.params.view);
          // A tab brought into view shows the latest at once, not at the next
          // poll. (Not on becoming active: a click inside a panel does that,
          // and redrawing then would swallow the click.)
          p.api.onDidVisibilityChange(v => { const m = mounted.get(body); if (v.isVisible && m?.update) call('update', body, () => m.update()); });
        },
      };
    },
    createRightHeaderActionComponent: group => {
      const el = document.createElement('div'); el.className = 'dv-acts';
      el.innerHTML = `<button type="button" class="tile-btn" data-dv-add title="Add a widget" aria-label="Add a widget">${tileIcon.add}</button><button type="button" class="tile-btn" data-dv-split="right" title="Split right" aria-label="Split right">${tileIcon.row}</button><button type="button" class="tile-btn" data-dv-split="below" title="Split down" aria-label="Split down">${tileIcon.col}</button><button type="button" class="tile-btn" data-dv-max title="Maximise (⌥Enter)" aria-label="Maximise">${tileIcon.max}</button>`;
      // Split: the next widget not yet open goes beside or below this group.
      el.querySelectorAll('[data-dv-split]').forEach((/** @type {HTMLElement} */ b) => b.onclick = () => {
        const open = new Set(openViews()), next = Object.keys(VIEWS).find(v => !open.has(v)), ref = group.activePanel;
        if (!next || !ref) return;
        dock.addPanel({ id: panelId(), component: 'view', params: { view: next }, title: VIEWS[next], position: { referencePanel: ref, direction: b.dataset.dvSplit } });
      });
      one('[data-dv-add]', el).onclick = () => { group.activePanel?.api.setActive(); palOpen('add '); };
      one('[data-dv-max]', el).onclick = () => { const p = group.activePanel; if (p) { p.api.isMaximized() ? p.api.exitMaximized() : p.api.maximize(); } };
      return { element: el, init() {}, dispose() {} };
    },
    getTabContextMenuItems: () => ['maximize', 'float', 'separator', 'close'],
  });
  const cur = lay.list.find(l => l.name === lay.current);
  // On a phone the workspace's widgets are tabs of one full-screen group,
  // and that arrangement is never saved over the real layout.
  dock.narrow = matchMedia('(max-width: 700px)').matches;
  if (dock.narrow) {
    const views = cur ? layViews(cur) : ['prices']; let first = null;
    for (const view of views) {
      const p = dock.addPanel({ id: panelId(), component: 'view', params: { view }, title: VIEWS[view] || view, ...(first ? { position: { referencePanel: first, direction: 'within' } } : {}) });
      first = first || p;
    }
    first?.api.setActive();
  } else {
    try { if (cur?.dock) dock.fromJSON(cur.dock); else buildTree(cur?.tree || LAYOUTS.Market()); }
    catch (e) { dock.clear(); buildTree(cur?.tree || LAYOUTS.Market()); }
  }
  if (state.focusView) { dock.panels.find(p => p.params?.view === state.focusView)?.api.setActive(); state.focusView = null; }
  const mine = dock;
  dock.onDidRemovePanel(p => {
    const body = panelBody.get(p.id); if (body) unmount(body); panelBody.delete(p.id);
    // Closing a workspace's last panel deletes the workspace (the only one left
    // gets the Docs panel back instead). Checked after the event, so a layout
    // being cleared and rebuilt, or the dock being replaced, isn't mistaken for it.
    setTimeout(() => {
      if (dock !== mine || dock.panels.length || state.tab !== 'workspace') return;
      if (lay.list.length > 1) layDelete(lay.current);
      else dock.addPanel({ id: panelId(), component: 'view', params: { view: 'docs' }, title: VIEWS.docs });
    });
  });
  dock.onDidActivePanelChange(e => { wsFocus = (e?.panel || e)?.id ?? null; });
  dock.onDidLayoutChange(() => { clearTimeout(layoutSaveTimer); layoutSaveTimer = setTimeout(laySave, 300); drawCharts(); });
  laySave();
  // Charts measure their panels, so draw them once the layout has settled.
  requestAnimationFrame(() => requestAnimationFrame(drawCharts));
}
// Show a view: activate it if open; otherwise put it in the active panel,
// beside it, as a tab with it, or along an edge of the whole workspace.
function wsShow(view, where) {
  if (!dock) return;
  const open = dock.panels.find(p => p.params?.view === view);
  if (open) { open.api.setActive(); return; }
  const ref = activePanel();
  if (where === 'here' && ref) {
    ref.api.updateParameters({ view }); ref.params.view = view; ref.api.setTitle(VIEWS[view] || view);
    const body = panelBody.get(ref.id); if (body) renderPanel(body, view);
    laySave(); return;
  }
  const position = !ref ? undefined : where === 'tab' ? { referencePanel: ref, direction: 'within' } : where === 'right' || where === 'below' ? { referencePanel: ref, direction: where } : { direction: where === 'column' ? 'right' : 'below' };
  dock.addPanel({ id: panelId(), component: 'view', params: { view }, title: VIEWS[view] || view, ...(position ? { position } : {}) });
}
// A widget asking for another to be shown (the screener for the Spread
// builder, the builder for Candles): opened beside the active panel if it
// isn't in the workspace.
document.addEventListener('tt-show-view', e => { const v = /** @type {CustomEvent} */ (e).detail; if (state.tab === 'workspace' && VIEWS[v]) wsShow(v, 'right'); });
function wsMaximise() { const p = activePanel(); if (p) p.api.isMaximized() ? p.api.exitMaximized() : p.api.maximize(); }
document.addEventListener('keydown', e => {
  if (state.tab !== 'workspace' || pal.open || sw.open) return;
  if (e.altKey && e.key === 'Enter') { e.preventDefault(); wsMaximise(); }
  else if (e.key === 'Escape' && activePanel()?.api.isMaximized()) { e.preventDefault(); activePanel().api.exitMaximized(); }
});

// The palette (⌘K): commands for the active panel and for workspaces.
function wsCommands() {
  /** @type {Command[]} */
  const out = [];
  for (const [v, label] of Object.entries(VIEWS)) {
    out.push({ label: `Show ${label} here`, run: () => wsShow(v, 'here') });
    out.push({ label: `Add ${label} to the right`, run: () => wsShow(v, 'right') });
    out.push({ label: `Add ${label} below`, run: () => wsShow(v, 'below') });
    out.push({ label: `Add ${label} as a tab`, run: () => wsShow(v, 'tab') });
    out.push({ label: `Add ${label} as a column at the far right`, run: () => wsShow(v, 'column') });
    out.push({ label: `Add ${label} as a row along the bottom`, run: () => wsShow(v, 'row') });
  }
  out.push({ label: activePanel()?.api.isMaximized() ? 'Restore all panels' : 'Maximise this panel', run: wsMaximise });
  if ((dock?.panels.length || 0) > 1) out.push({ label: 'Close this panel', run: () => activePanel()?.api.close() });
  for (const [name, make] of Object.entries(LAYOUTS)) out.push({ label: `Apply template: ${name}`, run: () => { dock.clear(); buildTree(make()); } });
  for (const [name, [, make]] of Object.entries(READY)) out.push({ label: `New workspace: ${name}`, run: () => wsAdd(name, make()) });
  return out;
}
const matching = q => { const words = q.toLowerCase().split(/\s+/).filter(Boolean); return [...layoutCommands(q), ...wsCommands()].filter(c => c.always || words.every(w => c.label.toLowerCase().includes(w))); };
const pal = { open: false, sel: 0, list: [] };
function palDraw() {
  if (!$('ws-pal')) return;
  pal.list = matching($('ws-pal-in').value).slice(0, 12); pal.sel = Math.min(pal.sel, Math.max(0, pal.list.length - 1));
  $('ws-pal-list').innerHTML = pal.list.map((c, i) => `<li role="option" data-pal="${i}" aria-selected="${i === pal.sel}">${esc(c.label)}</li>`).join('') || '<li class="muted">No matching command</li>';
}
function palOpen(prefill = '') {
  if (state.tab !== 'workspace') { state.tab = 'workspace'; state.navigated = true; render(); }
  if (!$('ws-pal')) document.body.insertAdjacentHTML('beforeend', `<div class="pal-back" id="ws-pal"><div class="pal" role="dialog" aria-label="Commands"><input id="ws-pal-in" placeholder="Type a command: news right, graph below, save Trading…" autocomplete="off" spellcheck="false"><ul id="ws-pal-list" role="listbox"></ul><p class="pal-hint">Acts on the active panel${activePanel() ? ': ' + esc(VIEWS[activePanel().params?.view] || '') : ''}. ↑↓ to choose, Enter to run, Esc to close.</p></div></div>`);
  pal.open = true; pal.sel = 0; $('ws-pal-in').value = prefill; palDraw(); $('ws-pal-in').focus();
}
function palClose() { $('ws-pal')?.remove(); pal.open = false; }
function palRun(i) { const c = pal.list[i]; if (!c) return; palClose(); c.run(); }
document.addEventListener('keydown', e => {
  if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === 'k') { e.preventDefault(); pal.open ? palClose() : palOpen(); return; }
  if (!pal.open) return;
  if (e.key === 'Escape') { e.preventDefault(); palClose(); }
  else if (e.key === 'ArrowDown' || e.key === 'ArrowUp') { e.preventDefault(); pal.sel = (pal.sel + (e.key === 'ArrowDown' ? 1 : -1) + pal.list.length) % Math.max(1, pal.list.length); palDraw(); }
  else if (e.key === 'Enter') { e.preventDefault(); palRun(pal.sel); }
});
document.addEventListener('input', e => { if (tgt(e).id === 'ws-pal-in') { pal.sel = 0; palDraw(); } });
document.addEventListener('click', e => {
  if (!pal.open) return;
  const li = near(e, '[data-pal]'); if (li) { palRun(+li.dataset.pal); return; }
  if (tgt(e).id === 'ws-pal') palClose();
});

// Saved workspaces: { name, group, dock (Dockview's layout) or tree }.
const LS = { list: 'tinytrader-workspaces', current: 'tinytrader-workspace-current' };
const lay = { list: [], current: null };
(() => {
  try { lay.list = JSON.parse(localStorage.getItem(LS.list)) || []; lay.current = localStorage.getItem(LS.current); } catch (_) {}
  if (!lay.list.length) lay.list = Object.entries(LAYOUTS).map(([name, make]) => ({ name, tree: make() }));
  // The Roadmap widget is now Reminders: carry saved panels over.
  try { lay.list = JSON.parse(JSON.stringify(lay.list).replaceAll('"view":"roadmap"', '"view":"reminders"')); } catch (_) {}
  // Ready-made workspaces: added, or replaced when they've been improved.
  try {
    // Versions installed, by name (a plain 1 from before meant the first two).
    const raw = localStorage.getItem('tinytrader-ready');
    const had = raw === '1' ? { 'Oil & Iran': 1, Earnings: 1 } : JSON.parse(raw || '{}');
    let changed = false;
    for (const [name, [version, make]] of Object.entries(READY)) {
      if ((had[name] || 0) >= version) continue;
      const fresh = { name, ...make() }, i = lay.list.findIndex(l => l.name === name);
      if (i >= 0) lay.list[i] = { ...fresh, group: lay.list[i].group };
      else if (lay.list.length < 10) lay.list.push(fresh);
      had[name] = version; changed = true;
    }
    if (changed) { localStorage.setItem('tinytrader-ready', JSON.stringify(had)); localStorage.setItem(LS.list, JSON.stringify(lay.list)); }
  } catch (_) {}
  if (!lay.list.some(l => l.name === lay.current)) lay.current = lay.list[0].name;
})();
function layStore() { try { localStorage.setItem(LS.list, JSON.stringify(lay.list)); localStorage.setItem(LS.current, lay.current ?? ''); } catch (_) {} updateLayoutNav(); }
function laySave() {
  const cur = lay.list.find(l => l.name === lay.current);
  if (cur && dock && !dock.narrow && state.tab === 'workspace') { cur.dock = dock.toJSON(); delete cur.tree; }
  layStore();
}
function laySwitch(name) {
  if (!lay.list.some(l => l.name === name)) return;
  if (dock && state.tab === 'workspace') laySave();
  lay.current = name; layStore();
  state.tab = 'workspace'; state.doc = null;
  try { history.replaceState(null, '', '/#w-' + encodeURIComponent(name)); } catch (_) {}
  state.navigated = true; render(); window.scrollTo(0, 0);
}
// Groups, in order of first appearance; ungrouped workspaces form their own.
const groupOf = l => l?.group || '';
const groups = () => [...new Set(lay.list.map(groupOf))];
const inGroup = g => lay.list.filter(l => groupOf(l) === g);
const layViews = l => l.dock ? Object.values(l.dock.panels || {}).map(p => p.params?.view) : (function leaves(n) { return n ? (n.view ? [n.view] : n.tabs ? n.tabs : [...leaves(n.a), ...leaves(n.b)]) : []; })(l.tree);
// Header: every workspace as a number, 1–9 then 0 (⌥1–⌥0), then 11 on.
// At most ten: 1–9 then 0.
const WS_MAX = 10;
const wsNumber = i => i < 9 ? String(i + 1) : '0';
// A new workspace, numbered next, with one panel to build on; it opens.
/** @param {string} [name] @param {{ tree?: object, dock?: object, theme?: object }} [start] */
function wsAdd(name, start = { tree: { view: 'search' } }) {
  if (lay.list.length >= WS_MAX) return false;
  const base = name || `Workspace ${wsNumber(lay.list.length)}`;
  let n = base; for (let k = 2; lay.list.some(l => l.name === n); k++) n = `${base} ${k}`;
  lay.list.push({ name: n, group: groupOf(lay.list.find(l => l.name === lay.current)) || undefined, ...JSON.parse(JSON.stringify(start)) });
  layStore(); laySwitch(n); return true;
}
// A workspace number's menu (right-click): rename, duplicate or delete it.
// Delete asks in place first.
function wmOpen(name, x, y) {
  wmClose();
  const l = lay.list.find(w => w.name === name); if (!l) return;
  document.body.insertAdjacentHTML('beforeend', `<div class="nw-pop" id="wm" role="menu" aria-label="Workspace ${esc(name)}" style="top:${y}px;left:${Math.max(16, Math.min(x, innerWidth - 336))}px"></div>`);
  wmDraw(l, false);
}
function wmDraw(l, confirm) {
  const m = $('wm'); if (!m) return;
  m.dataset.ws = l.name;
  m.innerHTML = confirm
    ? `<h3>Delete “${esc(l.name)}”?</h3><div class="wm-confirm"><button type="button" class="btn" data-wm="keep">Keep</button><button type="button" class="btn danger" data-wm="delete-yes">Delete</button></div>`
    : `<h3>${esc(l.name)}</h3>
      <button type="button" class="nw-item" role="menuitem" data-wm="rename"><b>Rename…</b></button>
      <button type="button" class="nw-item" role="menuitem" data-wm="theme"><b>Theme…</b><span>${l.theme ? esc(`${l.theme.symbols.length} symbols, ${l.theme.keywords.length} keywords`) : 'Symbols and news keywords its widgets follow'}</span></button>
      ${lay.list.length < WS_MAX ? '<button type="button" class="nw-item" role="menuitem" data-wm="duplicate"><b>Duplicate</b></button>' : ''}
      ${lay.list.length > 1 ? '<button type="button" class="nw-item" role="menuitem" data-wm="delete"><b>Delete</b></button>' : ''}`;
}
// A workspace's theme, edited in its menu: symbols and news keywords, or a preset.
function wmTheme(l) {
  const m = $('wm'); if (!m) return;
  const t = l.theme || { symbols: [], keywords: [] };
  m.classList.add('wm-wide');
  m.innerHTML = `<h3>Theme for “${esc(l.name)}”</h3>
    <label class="wm-field">Symbols <input id="wm-sym" class="input" value="${esc(t.symbols.join(', '))}" placeholder="USO, XLE, GLD"></label>
    <label class="wm-field">News keywords <input id="wm-kw" class="input" value="${esc(t.keywords.join(', '))}" placeholder="Iran, OPEC, crude"></label>
    <div class="wm-field">Calendar layers <span class="wm-layers">${LAYERS.map(([id, label]) => `<label><input type="checkbox" data-wm-layer="${id}"${(t.layers || DEFAULT_LAYERS).includes(id) ? ' checked' : ''}> ${label}</label>`).join('')}</span></div>
    <div class="wm-presets">${Object.keys(PRESETS).map(p => `<button type="button" class="btn small" data-wm="preset:${esc(p)}">${esc(p)}</button>`).join('')}</div>
    <div class="wm-confirm">${l.theme ? '<button type="button" class="btn" data-wm="theme-clear">Remove theme</button>' : ''}<button type="button" class="btn primary" data-wm="theme-save">Save</button></div>`;
  $('wm-sym').focus();
}
function wmClose() { $('wm')?.remove(); }
document.addEventListener('contextmenu', e => {
  const b = near(e, '#wtabs [data-layout]'); if (!b) return;
  e.preventDefault(); const r = b.getBoundingClientRect(); wmOpen(b.dataset.layout, r.left, r.bottom + 6);
});
document.addEventListener('click', e => {
  const m = $('wm'); if (!m) return;
  const a = near(e, '[data-wm]');
  if (!a) { if (!m.contains(tgt(e))) wmClose(); return; }
  const l = lay.list.find(w => w.name === m.dataset.ws); if (!l) return wmClose();
  const act = a.dataset.wm;
  if (act === 'delete') return wmDraw(l, true);
  if (act === 'theme') return wmTheme(l);
  if (act.startsWith('preset:')) {
    const t = PRESETS[act.slice(7)];
    if (t) {
      $('wm-sym').value = t.symbols.join(', '); $('wm-kw').value = t.keywords.join(', ');
      document.querySelectorAll('#wm [data-wm-layer]').forEach((/** @type {HTMLInputElement} */ c) => { c.checked = (t.layers || DEFAULT_LAYERS).includes(c.dataset.wmLayer || ''); });
    }
    return;
  }
  if (act === 'theme-save' || act === 'theme-clear') {
    const layers = [...document.querySelectorAll('#wm [data-wm-layer]')].filter((/** @type {HTMLInputElement} */ c) => c.checked).map((/** @type {HTMLElement} */ c) => c.dataset.wmLayer || '');
    const t = act === 'theme-clear' ? null : { symbols: parseSymbols($('wm-sym').value), keywords: parseKeywords($('wm-kw').value), layers };
    if (t && (t.symbols.length || t.keywords.length)) l.theme = t; else delete l.theme;
    layStore(); wmClose();
    if (l.name === lay.current) { setTheme(l.theme || null); document.dispatchEvent(new CustomEvent('tt-theme')); refreshTiles(); }
    return;
  }
  if (act === 'keep') return wmDraw(l, false);
  wmClose();
  if (act === 'delete-yes') { layDelete(l.name); updateLayoutNav(); }
  else if (act === 'duplicate') { if (dock && state.tab === 'workspace' && l.name === lay.current) laySave(); wsAdd(l.name + ' copy', l.dock ? { dock: l.dock } : { tree: l.tree }); }
  else if (act === 'rename') { if (l.name !== lay.current) laySwitch(l.name); palOpen('rename '); }
}, true);
document.addEventListener('keydown', e => { if (e.key === 'Escape' && $('wm')) wmClose(); });

function updateLayoutNav() {
  const box = $('wtabs'); if (!box) return;
  const docs = state.tab.startsWith('col:');
  const html = lay.list.map((l, i) => {
    const here = !docs && l.name === lay.current, n = wsNumber(i);
    return `<button type="button" class="wtab wtab-num" data-layout="${esc(l.name)}" title="${esc(l.name)}${i < 10 ? ` (⌥${n})` : ''}${here ? ' · click for all workspaces (⌥W)' : ''} · right-click to rename, duplicate or delete" aria-label="Workspace ${n}: ${esc(l.name)}"${here ? ' aria-current="page"' : ''}>${n}</button>`;
  }).join('')
    + (lay.list.length < WS_MAX ? `<button type="button" class="wtab wtab-num" data-ws-add title="New workspace, opening on Search (⌥${wsNumber(lay.list.length)})" aria-label="New workspace">+</button>` : '');
  if (box.innerHTML !== html) box.innerHTML = html;
}
// A ticker searched on Search opens a workspace named for it, whose theme is
// that ticker: its candles, news and earnings follow the theme. The search
// box learns whether it opened (ten workspaces at most).
document.addEventListener('tt-open-ticker', e => {
  const detail = /** @type {CustomEvent} */ (e).detail;
  detail.opened = wsAdd(detail.symbol, { tree: TICKER(), theme: { symbols: [detail.symbol], keywords: [], layers: DEFAULT_LAYERS } });
});
// A document picked on the docs home opens in the docs pages.
document.addEventListener('tt-open-doc', e => {
  if (!state.tab.startsWith('col:')) return;
  const { id, index } = /** @type {CustomEvent} */ (e).detail;
  e.preventDefault();
  state.tab = 'col:' + id;
  openDoc(index);
});
document.addEventListener('click', e => {
  if (near(e, '[data-switcher]')) { swOpen(); return; }
  if (near(e, '[data-docs-home]') && state.collections[0]) {
    state.tab = 'col:' + state.collections[0].id; state.doc = null; state.navigated = true;
    try { history.replaceState(null, '', '/docs/'); } catch (_) {}
    render(); window.scrollTo(0, 0); return;
  }
  if (near(e, '[data-ws-add]')) { wsAdd(); return; }
  const b = near(e, '[data-layout]');
  if (b) { if (b.getAttribute('aria-current') === 'page') swOpen(); else laySwitch(b.dataset.layout); }
});
// Keys: ⌥↑/⌥↓ (or ⌃↑/⌃↓) within the group, ⌥←/⌥→ between groups, ⌥1–⌥9 and ⌥0 the numbered workspaces, ⌥W the switcher.
document.addEventListener('keydown', e => {
  if (pal.open || sw.open || e.metaKey) return;
  if (['INPUT', 'TEXTAREA', 'SELECT'].includes(document.activeElement?.tagName)) return;
  const cur = lay.list.find(l => l.name === lay.current), g = groupOf(cur), tabs = inGroup(g), gs = groups();
  if (e.altKey && e.code === 'KeyW') { e.preventDefault(); swOpen(); return; }
  if (e.altKey && /^Digit[0-9]$/.test(e.code)) {
    const d = +e.code.slice(5), i = d === 0 ? 9 : d - 1, l = lay.list[i];
    e.preventDefault();
    if (l) laySwitch(l.name); else if (i === lay.list.length) wsAdd();
    return;
  }
  if ((e.ctrlKey || e.altKey) && ['ArrowUp', 'ArrowDown'].includes(e.key)) {
    const i = tabs.indexOf(cur), n = tabs.length; e.preventDefault();
    laySwitch(tabs[(i + (e.key === 'ArrowDown' ? 1 : -1) + n) % n].name); return;
  }
  if (e.altKey && ['ArrowLeft', 'ArrowRight'].includes(e.key) && gs.length > 1) {
    const i = gs.indexOf(g), next = gs[(i + (e.key === 'ArrowRight' ? 1 : -1) + gs.length) % gs.length];
    e.preventDefault(); laySwitch(inGroup(next)[0].name);
  }
});
// Palette commands for workspaces: save, rename, group, duplicate, delete, switch.
function layoutCommands(q) {
  /** @type {Command[]} */
  const out = lay.list.map(l => ({ label: `Switch to workspace: ${groupOf(l) ? groupOf(l) + ' › ' : ''}${l.name}`, run: () => laySwitch(l.name) }));
  const m = q.match(/^\s*(save|rename|group)\s+(.*?)\s*$/i);
  const cur = lay.list.find(l => l.name === lay.current);
  if (m && m[2]) {
    const arg = m[2].slice(0, 40), verb = m[1].toLowerCase();
    if (verb === 'save') out.unshift({ label: `Save these panels as a new workspace “${arg}”`, always: true, run: () => { if (!lay.list.some(l => l.name === arg) && lay.list.length >= WS_MAX) return; laySave(); lay.list = lay.list.filter(l => l.name !== arg); lay.list.push({ name: arg, group: groupOf(cur) || undefined, dock: dock?.toJSON() }); lay.current = arg; layStore(); } });
    if (verb === 'rename') out.unshift({ label: `Rename “${lay.current}” to “${arg}”`, always: true, run: () => { if (cur && !lay.list.some(l => l.name === arg)) { cur.name = arg; lay.current = arg; layStore(); } } });
    if (verb === 'group') out.unshift({ label: `Move “${lay.current}” to the group “${arg}”`, always: true, run: () => { if (cur) { cur.group = arg; layStore(); } } });
  } else out.push({ label: 'Save these panels as a new workspace… (type: save <name>)', run: () => {} }, { label: 'Rename this workspace… (type: rename <name>)', run: () => {} }, { label: 'Move this workspace to a group… (type: group <name>)', run: () => {} });
  out.push({ label: 'Open the workspace switcher (⌥W)', run: swOpen });
  out.push(prof.on ? { label: 'Profile mode: turn off', run: stopProfile } : { label: 'Profile mode: turn on (measure everything on this page)', run: startProfile });
  if (lay.list.length > 1) out.push({ label: `Delete this workspace: ${lay.current}`, run: () => layDelete(lay.current) });
  return out;
}
function layDelete(name) {
  if (lay.list.length < 2) return;
  const i = lay.list.findIndex(l => l.name === name); if (i < 0) return;
  lay.list.splice(i, 1);
  const wasCurrent = lay.current === name;
  // Workspaces still under their default name ("Workspace 5") take their new number.
  const renamed = new Map();
  lay.list.forEach((l, k) => { if (/^Workspace \d+$/.test(l.name) && l.name !== `Workspace ${wsNumber(k)}`) renamed.set(l, `Workspace ${wsNumber(k)}`); });
  for (const [l, n] of renamed) {
    if (lay.list.some(o => o !== l && o.name === n && !renamed.has(o))) continue;
    if (lay.current === l.name) lay.current = n;
    l.name = n;
  }
  if (wasCurrent) { lay.current = lay.list[Math.max(0, i - 1)].name; layStore(); state.navigated = true; render(); } else layStore();
}

// The switcher (⌥W): every workspace as a tree of groups. ↑↓ move, →/Tab
// open a group, ← close it, Enter opens; F2 renames, D duplicates, G moves to
// a group, N saves the current panels as a new workspace, ⌫ deletes (asks
// first); drag a workspace to reorder it or onto a group to move it there.
const sw = { open: false, sel: 0, rows: [], expanded: new Set(), edit: null, confirm: null };
function swRows() {
  const rows = [];
  for (const g of groups()) {
    if (!g) { for (const l of inGroup('')) rows.push({ kind: 'ws', l, depth: 0 }); continue; }
    const open = sw.expanded.has(g);
    rows.push({ kind: 'group', g, depth: 0, open, n: inGroup(g).length });
    if (open) for (const l of inGroup(g)) rows.push({ kind: 'ws', l, depth: 1 });
  }
  return rows;
}
function preview(l) {
  const box = (v) => `<div class="pv-leaf"><span>${esc(VIEWS[v] || v || '')}</span></div>`;
  if (l?.dock?.grid?.root) {
    const panels = l.dock.panels || {};
    const node = (n, orient) => n.type === 'leaf'
      ? `<div class="pv-leaf" style="flex:${n.size || 1} 1 0"><span>${(n.data?.views || []).map(id => esc(VIEWS[panels[id]?.params?.view] || panels[id]?.title || '')).join(' · ')}</span></div>`
      : `<div class="pv-split ${orient === 'HORIZONTAL' ? 'row' : 'col'}" style="flex:${n.size || 1} 1 0">${(n.data || []).map(c => node(c, orient === 'HORIZONTAL' ? 'VERTICAL' : 'HORIZONTAL')).join('')}</div>`;
    return node(l.dock.grid.root, l.dock.grid.orientation);
  }
  const tree = n => n.view ? box(n.view) : `<div class="pv-split ${n.dir}">${tree(n.a)}${tree(n.b)}</div>`;
  return l?.tree ? tree(l.tree) : '';
}
function swDraw() {
  const box = $('sw'); if (!box) return;
  sw.rows = swRows(); sw.sel = Math.min(sw.sel, sw.rows.length - 1);
  const sel = sw.rows[sw.sel];
  box.querySelector('.sw-list').innerHTML = sw.rows.map((r, i) => {
    const on = i === sw.sel ? ' aria-selected="true"' : '';
    if (r.kind === 'group') return `<li class="sw-group" data-sw="${i}" draggable="false"${on}><span class="sw-tw">${r.open ? '▾' : '▸'}</span>${esc(r.g)} <em>${r.n}</em></li>`;
    const editing = sw.edit && sw.edit.row === i;
    return `<li class="sw-ws${r.l.name === lay.current ? ' current' : ''}" data-sw="${i}" draggable="true" style="padding-left:${12 + r.depth * 18}px"${on}>${editing ? `<input class="sw-in" id="sw-in" value="${esc(sw.edit.value)}" placeholder="${sw.edit.kind === 'group' ? 'Group (empty for none)' : 'Name'}">` : `<span class="sw-name">${esc(r.l.name)}</span><span class="sw-acts"><button type="button" data-sw-act="rename" title="Rename (F2)" aria-label="Rename ${esc(r.l.name)}">✎</button><button type="button" data-sw-act="delete" title="Delete (⌫)" aria-label="Delete ${esc(r.l.name)}"${lay.list.length > 1 ? '' : ' disabled'}>×</button></span>`}</li>`;
  }).join('');
  const l = sel?.kind === 'ws' ? sel.l : sel?.kind === 'group' ? inGroup(sel.g)[0] : null;
  box.querySelector('.sw-pv').innerHTML = l ? `<div class="pv">${preview(l)}</div><div class="sw-cap">${esc(l.name)} · ${layViews(l).length} widgets</div>` : '';
  box.querySelector('.sw-foot').innerHTML = sw.confirm
    ? `<b>Delete “${esc(sw.confirm)}”?</b><span class="sw-btns"><button type="button" class="btn" data-sw-act="keep">Keep</button><button type="button" class="btn danger" data-sw-act="really-delete">Delete</button></span>`
    : sw.edit?.kind === 'new' ? '<input class="sw-in" id="sw-in" placeholder="New workspace name, then Enter" autocomplete="off">'
    : `${lay.list.length < WS_MAX ? '<button type="button" class="btn" data-sw-act="new">+ New workspace</button>' : '<span>Ten workspaces, the most there can be.</span>'}<span>↑↓ move · Enter go · F2 rename · D duplicate · G group · ⌫ delete</span>`;
  $('sw-in')?.focus();
}
function swOpen() {
  if (state.tab !== 'workspace') { state.tab = 'workspace'; state.navigated = true; render(); }
  if (dock) laySave();
  const cur = lay.list.find(l => l.name === lay.current);
  if (groupOf(cur)) sw.expanded.add(groupOf(cur));
  document.body.insertAdjacentHTML('beforeend', `<div class="pal-back" id="sw-back"><div class="pal sw" id="sw" role="dialog" aria-label="Workspaces"><div class="sw-body"><ul class="sw-list" role="tree"></ul><div class="sw-pv"></div></div><div class="pal-hint sw-foot"></div></div></div>`);
  sw.open = true; sw.rows = swRows(); sw.sel = Math.max(0, sw.rows.findIndex(r => r.kind === 'ws' && r.l.name === lay.current)); swDraw();
}
function swClose() { $('sw-back')?.remove(); sw.open = false; sw.edit = null; sw.confirm = null; }
function swCommit() {
  const { kind, l, value } = sw.edit; const v = value.trim().slice(0, 40); sw.edit = null;
  if (kind === 'rename' && v && !lay.list.some(x => x.name === v)) { if (lay.current === l.name) lay.current = v; l.name = v; }
  if (kind === 'group') { l.group = v || undefined; if (v) sw.expanded.add(v); }
  if (kind === 'new' && v && !lay.list.some(x => x.name === v) && lay.list.length < WS_MAX) { swClose(); wsAdd(v); return; }
  layStore(); swDraw();
}
document.addEventListener('keydown', e => {
  if (!sw.open) return;
  const r = sw.rows[sw.sel];
  if (sw.edit) {
    if (e.key === 'Enter') { e.preventDefault(); sw.edit.value = $('sw-in').value; swCommit(); }
    else if (e.key === 'Escape') { e.preventDefault(); sw.edit = null; swDraw(); }
    return;
  }
  if (sw.confirm) {
    e.preventDefault();
    if (e.key === 'Enter') { const n = sw.confirm; sw.confirm = null; layDelete(n); swDraw(); }
    else if (e.key === 'Escape') { sw.confirm = null; swDraw(); }
    return;
  }
  const k = e.key;
  if (k === 'Escape' || (e.altKey && e.code === 'KeyW')) { e.preventDefault(); swClose(); }
  else if (k === 'ArrowDown' || k === 'ArrowUp') { e.preventDefault(); sw.sel = (sw.sel + (k === 'ArrowDown' ? 1 : -1) + sw.rows.length) % sw.rows.length; swDraw(); }
  else if ((k === 'ArrowRight' || k === 'Tab') && r?.kind === 'group') { e.preventDefault(); if (!r.open) sw.expanded.add(r.g); else sw.sel++; swDraw(); }
  else if (k === 'ArrowLeft') { e.preventDefault(); if (r?.kind === 'group') sw.expanded.delete(r.g); else if (r?.depth) { sw.expanded.delete(groupOf(r.l)); sw.sel = sw.rows.findIndex(x => x.kind === 'group' && x.g === groupOf(r.l)); } swDraw(); }
  else if (k === 'Enter') { e.preventDefault(); if (r?.kind === 'group') { r.open ? sw.expanded.delete(r.g) : sw.expanded.add(r.g); swDraw(); } else if (r) { swClose(); laySwitch(r.l.name); } }
  else if (k === 'F2' && r?.kind === 'ws') { e.preventDefault(); sw.edit = { kind: 'rename', row: sw.sel, l: r.l, value: r.l.name }; swDraw(); }
  else if ((k === 'g' || k === 'G') && r?.kind === 'ws') { e.preventDefault(); sw.edit = { kind: 'group', row: sw.sel, l: r.l, value: groupOf(r.l) }; swDraw(); }
  else if ((k === 'd' || k === 'D') && r?.kind === 'ws' && lay.list.length < WS_MAX) { e.preventDefault(); let n = r.l.name + ' copy'; while (lay.list.some(x => x.name === n)) n += '′'; lay.list.splice(lay.list.indexOf(r.l) + 1, 0, { ...JSON.parse(JSON.stringify(r.l)), name: n }); layStore(); swDraw(); }
  else if ((k === 'n' || k === 'N') && lay.list.length < WS_MAX) { e.preventDefault(); sw.edit = { kind: 'new', row: sw.sel, l: null, value: '' }; swDraw(); }
  else if ((k === 'Backspace' || k === 'Delete') && r?.kind === 'ws' && lay.list.length > 1) { e.preventDefault(); sw.confirm = r.l.name; swDraw(); }
}, true);
document.addEventListener('click', e => {
  if (!sw.open) return;
  if (tgt(e).id === 'sw-back') { swClose(); return; }
  const act = near(e, '[data-sw-act]');
  if (act) {
    const a = act.dataset.swAct, i = +(/** @type {HTMLElement | null} */ (act.closest('[data-sw]'))?.dataset.sw ?? -1), r = sw.rows[i];
    if (a === 'rename' && r) { sw.sel = i; sw.edit = { kind: 'rename', row: i, l: r.l, value: r.l.name }; }
    else if (a === 'delete' && r && lay.list.length > 1) { sw.sel = i; sw.confirm = r.l.name; }
    else if (a === 'new') { sw.edit = { kind: 'new', row: -1, l: null, value: '' }; }
    else if (a === 'keep') sw.confirm = null;
    else if (a === 'really-delete') { const n = sw.confirm; sw.confirm = null; layDelete(n); }
    swDraw(); return;
  }
  const li = near(e, '[data-sw]'); if (!li || tgt(e).id === 'sw-in') return;
  const i = +li.dataset.sw, r = sw.rows[i];
  if (sw.sel === i && r.kind === 'ws') { swClose(); laySwitch(r.l.name); return; }
  sw.sel = i; if (r.kind === 'group') { r.open ? sw.expanded.delete(r.g) : sw.expanded.add(r.g); } swDraw();
});
// Drag a workspace onto another to put it before that one (and in its group), or onto a group.
document.addEventListener('dragstart', e => { const li = near(e, '.sw-ws[data-sw]'); if (li) e.dataTransfer.setData('text/plain', li.dataset.sw); });
document.addEventListener('dragover', e => { if (sw.open && near(e, '[data-sw]')) e.preventDefault(); });
document.addEventListener('drop', e => {
  const li = sw.open && near(e, '[data-sw]'); if (!li) return;
  e.preventDefault();
  const from = sw.rows[+e.dataTransfer.getData('text/plain')], to = sw.rows[+li.dataset.sw];
  if (!from || from.kind !== 'ws' || !to || from === to) return;
  lay.list.splice(lay.list.indexOf(from.l), 1);
  if (to.kind === 'group') { from.l.group = to.g; lay.list.push(from.l); }
  else { from.l.group = to.l.group; lay.list.splice(lay.list.indexOf(to.l), 0, from.l); }
  layStore(); swDraw();
});


function render() {
  // Rebuilding the page must never move the reader: keep the scroll position.
  const scrollY = window.scrollY;
  const focusId = /** @type {HTMLElement | null} */ (document.activeElement)?.dataset?.node;
  // Collections only change on navigation, not on every refresh.
  if (state.tab.startsWith('col:') && !state.navigated) return status();
  if (state.tab === 'workspace' && !state.navigated && $('ws-root')) { refreshTiles(); drawCharts(); return status(); }
  state.navigated = false;
  document.body.classList.toggle('ws-full', state.tab === 'workspace');
  document.body.classList.toggle('docs-mode', state.tab.startsWith('col:'));
  document.documentElement.style.setProperty('--top-h', one('.top').offsetHeight + 'px');
  for (const body of mounted.keys()) unmount(body);
  if (dock) { dock.dispose(); dock = null; panelBody.clear(); }
  if (state.tab === 'workspace') { $('view').innerHTML = workspaceView(); mountWorkspace(); }
  else {
    $('view').innerHTML = collectionView();
    const home = one('.docs-page-home', $('view'));
    if (home) mounted.set(home, widget('docs').mount(home));
  }
  if (focusId) { const n = one(`[data-node="${CSS.escape(focusId)}"]`); if (n) n.focus({ preventScroll: true }); }
  updateCollectionsNav();
  const docOpen = state.tab.startsWith('col:') && state.doc !== null;
  document.querySelectorAll('.nav button[data-tab]').forEach((/** @type {HTMLElement} */ b) => { if (b.dataset.tab === state.tab && !docOpen) b.setAttribute('aria-current', 'page'); else b.removeAttribute('aria-current'); });
  updateLayoutNav();
  if (state.tab.startsWith('col:')) wireDoc();
  drawCharts();
  status();
  if (window.scrollY !== scrollY) window.scrollTo(0, scrollY);
}
// Alerts: the bell beside the theme button. Its count is the services down
// or needing attention; the list says which and why, and opens each one.
function alerts() {
  return (fleet.topo?.services || []).map(s => ({ s, st: statusOf({ ...s, kind: 'svc' }) })).filter(x => x.st === 'crit' || x.st === 'warn')
    .sort((a, b) => (a.st === 'crit' ? 0 : 1) - (b.st === 'crit' ? 0 : 1));
}
function updateHeaderStatus() {
  if (!fleet.topo) return;
  const list = alerts(), n = $('bell-n'), crit = list.some(x => x.st === 'crit');
  n.hidden = !list.length; n.textContent = String(list.length); n.classList.toggle('crit', crit);
  $('bell').title = list.length ? `${list.length} alert${list.length === 1 ? '' : 's'}` : 'No alerts';
  if (!$('bell-pop').hidden) bellDraw();
}
function bellDraw() {
  const list = alerts();
  const html = '<h3>Alerts</h3>' + (list.length
    ? list.map(x => `<button type="button" class="bell-item" data-alert="${esc(x.s.id)}"><i class="${x.st}"></i><b>${esc(x.s.id)} ${x.st === 'crit' ? 'is down' : 'needs attention'}</b><span>${esc(subOf({ ...x.s, kind: 'svc' }))}</span></button>`).join('')
    : '<p class="bell-none">Every service is running or off schedule.</p>');
  if ($('bell-pop').innerHTML !== html) $('bell-pop').innerHTML = html;
}
function bellToggle(open) {
  $('bell-pop').hidden = !open; $('bell').setAttribute('aria-expanded', open);
  if (open) { const r = $('bell').getBoundingClientRect(); $('bell-pop').style.top = r.bottom + 6 + 'px'; bellDraw(); }
}
$('bell').addEventListener('click', e => { e.stopPropagation(); bellToggle($('bell-pop').hidden); });
document.addEventListener('click', e => {
  const a = near(e, '[data-alert]');
  if (a) { bellToggle(false); const home = lay.list.find(l => layViews(l).includes('overview')); if (home) { laySwitch(home.name); showNode(a.dataset.alert); } return; }
  if (!$('bell-pop').hidden && !near(e, '.bell-wrap')) bellToggle(false);
});
document.addEventListener('keydown', e => { if (e.key === 'Escape' && !$('bell-pop').hidden) bellToggle(false); });
function status() {
  updateHeaderStatus();
  const age = state.lastOk ? (Date.now() - state.lastOk) / 1000 : null;
  const stale = age == null || age > 6;
  const problems = [];
  if (stale && age != null) problems.push('The console backend is not responding. Data shown may be out of date.');
  const d = fleet.snap?.docker;
  if (d?.configured && d.error) problems.push('Docker API unavailable: ' + d.error + '. Container state is missing.');
  if (fleet.snap?.error) problems.push(fleet.snap.error);
  $('banner').hidden = !problems.length;
  $('banner').textContent = problems.join(' ');
}
async function loadTopology() {
  try { const r = await fetch('/api/topology', { cache: 'no-store' }); if (r.ok) { fleet.topo = await r.json(); document.title = fleet.topo.name ? `tinytrader · ${fleet.topo.name}` : 'tinytrader'; } } catch (_) {}
}
// The snapshot, as changes since the last answer: each service's unchanged
// fields keep their previous value (and object), so most polls carry only
// what moved (see services/console/src/delta.rs).
const since = { generation: 0, epoch: '' };
async function poll() {
  try {
    const r = await fetch(`/api/state?since=${since.generation}&epoch=${since.epoch}`, { cache: 'no-store' });
    if (r.ok) {
      let v;
      if (prof.on) { const text = await r.text(), a = performance.now(); v = JSON.parse(text); statePoll(text.length, performance.now() - a); }
      else v = await r.json();
      const old = v.full ? {} : (fleet.snap?.services || {}), services = {};
      for (const [id, s] of Object.entries(v.services || {})) {
        if (!s || typeof s !== 'object' || !Array.isArray(s._fields)) { services[id] = s; continue; }
        const prev = old[id] || {}, o = {};
        for (const f of s._fields) o[f] = f in s ? s[f] : prev[f];
        services[id] = o;
      }
      since.generation = v.generation ?? 0; since.epoch = v.epoch ?? '';
      delete v.generation; delete v.epoch; delete v.full;
      fleet.snap = { ...v, services };
      state.lastOk = Date.now();
    }
  } catch (_) {}
  if (!document.querySelector('.tip:not([hidden])')) render(); else status();
}

document.querySelector('.nav').addEventListener('click', e => {
  const f = near(e, '[data-folder]');
  if (f) {
    const c = state.collections.find(x => x.id === f.dataset.col), k = folderKey(c, f.dataset.folder);
    if (state.openGroups.has(k)) { closeFolder(k); updateCollectionsNav(); return; }
    openFolder(k);
    // Like a docs site: opening a folder shows its first document.
    const first = c.docs.find(d => d.group === f.dataset.folder);
    if (first) { state.tab = 'col:' + c.id; openDoc(first.index); } else updateCollectionsNav();
    return;
  }
  const dc = near(e, '[data-doc]');
  if (dc) { state.tab = 'col:' + dc.dataset.col; openDoc(+dc.dataset.doc); return; }
  const b = near(e, 'button[data-tab]'); if (!b) return;
  state.tab = b.dataset.tab; state.doc = null; state.navigated = true;
  const c = currentCollection();
  try { history.replaceState(null, '', '#' + (c ? c.id : state.tab)); } catch (_) {}
  render(); window.scrollTo(0, 0);
});
$('view').addEventListener('click', e => {
  const dc = near(e, '[data-doc]');
  if (dc) { openDoc(dc.dataset.doc === '' ? null : +dc.dataset.doc); return; }
  const a = /** @type {HTMLAnchorElement | null} */ (near(e, '.prose a[href]'));
  if (a) {
    const href = a.getAttribute('href');
    if (href.startsWith('#')) return;
    const target = resolveDocLink(href);
    if (target) { e.preventDefault(); openDoc(target.index); }
    else if (/^https?:/.test(href)) { a.target = '_blank'; a.rel = 'noopener'; }
    else e.preventDefault();
    return;
  }
});
let rt; window.addEventListener('resize', () => { clearTimeout(rt); rt = setTimeout(drawCharts, 150); });

// Theme: light or dark, following the system until the viewer picks one.
// Picking the system's own choice again goes back to following it.
const darkQuery = window.matchMedia('(prefers-color-scheme: dark)');
const systemTheme = () => darkQuery.matches ? 'dark' : 'light';
const themeIcon = {
  light: '<svg width="22" height="22" viewBox="0 0 20 20" fill="none" stroke="currentColor" stroke-width="1.6" aria-hidden="true"><circle cx="10" cy="10" r="3.5"/><path d="M10 1.5v2M10 16.5v2M1.5 10h2M16.5 10h2M4 4l1.4 1.4M14.6 14.6 16 16M4 16l1.4-1.4M14.6 5.4 16 4"/></svg>',
  dark: '<svg width="22" height="22" viewBox="0 0 20 20" fill="none" stroke="currentColor" stroke-width="1.6" aria-hidden="true"><path d="M16.5 12.5A7 7 0 0 1 7.5 3.5a7 7 0 1 0 9 9Z"/></svg>'
};
let themeChoice = null;
try { const saved = localStorage.getItem('tinytrader-theme'); if (saved === 'light' || saved === 'dark') themeChoice = saved; } catch (_) {}
function applyTheme() {
  if (themeChoice) document.documentElement.setAttribute('data-theme', themeChoice);
  else document.documentElement.removeAttribute('data-theme');
  const now = themeChoice || systemTheme(), next = now === 'dark' ? 'light' : 'dark';
  $('theme-btn').innerHTML = themeIcon[now];
  $('theme-btn').setAttribute('aria-label', `Switch to ${next} theme`);
  $('theme-btn').title = `Switch to ${next} theme`;
}
applyTheme();
darkQuery.addEventListener('change', applyTheme);
// Palette: Frost by default; the choice is remembered per browser.
(() => {
  const set = v => { if (v) document.documentElement.setAttribute('data-palette', v); else document.documentElement.removeAttribute('data-palette'); };
  const palettes = ['frost', 'spruce', 'pbj', 'amber', 'swiss', 'pbj2', 'harbor', 'dusk'];
  let saved = 'frost'; try { const v = localStorage.getItem('tinytrader-palette'); if (palettes.includes(v)) saved = v; } catch (_) {}
  set(saved); $('palette').value = saved;
  $('palette').addEventListener('change', e => { set(tgt(e).value); try { localStorage.setItem('tinytrader-palette', tgt(e).value); } catch (_) {} drawCharts(); });
})();
$('theme-btn').addEventListener('click', () => {
  const next = (themeChoice || systemTheme()) === 'dark' ? 'light' : 'dark';
  themeChoice = next === systemTheme() ? null : next;
  try { if (themeChoice) localStorage.setItem('tinytrader-theme', themeChoice); else localStorage.removeItem('tinytrader-theme'); } catch (_) {}
  applyTheme();
  drawCharts();
});

// Profile mode first, so the first mounts are measured too.
if (profileWanted()) startProfile();
(async () => {
  await loadCollections();
  const h = location.hash.slice(1), hc = state.collections.find(c => h === c.id || h.startsWith(c.id + '-'));
  if (location.pathname.startsWith('/docs') && !hc && state.collections[0]) { state.tab = 'col:' + state.collections[0].id; state.doc = null; state.navigated = true; }
  if (hc) {
    state.tab = 'col:' + hc.id; state.doc = h.length > hc.id.length ? +h.slice(hc.id.length + 1) : null; state.navigated = true;
    const d = hc.docs.find(x => x.index === state.doc); if (d) revealDoc(hc, d);
  }
  else if (h.startsWith('w-') && lay.list.some(l => l.name === decodeURIComponent(h.slice(2)))) laySwitch(decodeURIComponent(h.slice(2)));
  else if (h) {
    // Old page links open the workspace holding that view, with it focused.
    const view = { charts: 'prices', workspace: null }[h] ?? h;
    const home = view && lay.list.find(l => layViews(l).includes(view));
    if (home) { state.focusView = view; laySwitch(home.name); }
  }
  await loadTopology(); await poll();
  setInterval(poll, 2000);
  setInterval(loadTopology, 5000);
})();

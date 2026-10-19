// The console's shell: the header (workspace numbers, the bell, themes),
// workspaces on Dockview, the palette and the switcher, the docs pages, and
// polling the fleet. The widgets are modules under ../widgets (decision 0010).
// Settings shared between devices: read before anything here runs.
import { refusedNote } from './prefs.js';
import { writeGate } from './api.js';
import { esc, bytes, duration, fmt, ago, etStamp } from './format.js';
import { fleet, setTopology, statusOf, subOf, serviceFor } from './fleet.js';
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
import '../widgets/performance.js';
import '../widgets/news.js';
import '../widgets/calendar.js';
import '../widgets/data.js';
import '../widgets/jobs.js';
import '../widgets/queue.js';
import '../widgets/storage.js';
import '../widgets/reminders.js';
import { bindSearch } from '../widgets/search.js';
import '../widgets/docs.js';
import '../widgets/watchlist.js';
import '../widgets/ivmap.js';
import '../widgets/correlation.js';
import '../widgets/earnings.js';
import '../widgets/notebook.js';
import '../widgets/terminal.js';
import '../widgets/earningsweek.js';
import '../widgets/spreads.js';
import '../widgets/alerts.js';
import '../widgets/tape.js';
import { watch as watchAlerts, unseen, alerts as priceAlerts, describe as alertText, markSeen, alertForm, alertsFor } from './alerts.js';
import '../widgets/screener.js';
import '../widgets/day.js';
import '../widgets/heatmap.js';
import '../widgets/crossasset.js';
import '../widgets/observations.js';
import '../widgets/relations.js';
import { watchlists, addList, addSymbols } from './watchlists.js';
import { setTheme, setFocus, PRESETS, parseSymbols, parseKeywords, LAYERS, DEFAULT_LAYERS } from './theme.js';
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
  // A watchlist (lib/watchlists.js) with a chart of the symbol clicked, and news on the list.
  Watchlist: [1, () => ({ tree: { dir: 'row', ratio: 0.45, a: { dir: 'col', ratio: 0.55, a: { view: 'watchlist' }, b: { view: 'candles' } }, b: { view: 'news' } } })],
  Earnings: [1, () => ({ tree: { dir: 'row', ratio: 0.6, a: { view: 'earnings' }, b: { dir: 'col', ratio: 0.45, a: { view: 'calendar' }, b: { tabs: ['ivmap', 'candles'] } } } })],
  // One account (the live one to start), with room for its positions and orders.
  Account: [1, () => ({ tree: { view: 'account' } })],
  // One account's P/L: the day's trades as a heatmap, and its days.
  Performance: [1, () => ({ tree: { view: 'performance' } })],
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
  // A leaf may carry its panel's arguments (`args`: a watchlist's list).
  const add = (view, ref, dir, args) => dock.addPanel({ id: panelId(), component: 'view', params: { view: viewOf(view), ...(args ? { args } : {}) }, title: VIEWS[viewOf(view)] || view, ...(ref ? { position: { referencePanel: ref, direction: dir } } : {}) });
  const place = (n, ref, dir) => {
    const [v, ...more] = n.tabs || [n.view];
    const p = add(v, ref, dir, n.args);
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
          // A panel's arguments (a Day tab's day) reach its widget as data-args.
          if (p.params.args) body.dataset.args = JSON.stringify(p.params.args);
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
  // Panels copied or moved here from another view (the palette's "Copy this
  // panel to…"), added along the right on arriving.
  if (cur?.add?.length) {
    for (const a of cur.add) dock.addPanel({ id: panelId(), component: 'view', params: { view: a.view, ...(a.args ? { args: a.args } : {}) }, title: a.title || VIEWS[a.view] || a.view, position: { direction: 'right' } });
    delete cur.add; setTimeout(laySave);
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
  dock.onDidActivePanelChange(e => { wsFocus = (e?.panel || e)?.id ?? null; wsnDraw(); });
  dock.onDidAddPanel(() => wsnDraw());
  dock.onDidLayoutChange(() => { clearTimeout(layoutSaveTimer); layoutSaveTimer = setTimeout(laySave, 300); drawCharts(); wsnDraw(); });
  laySave();
  // Charts measure their panels, so draw them once the layout has settled.
  requestAnimationFrame(() => requestAnimationFrame(drawCharts));
}
// A watchlist made later gets its view; one renamed takes its view's name along.
document.addEventListener('tt-watchlist', () => { if (watchViews()) layStore(); watchHome(); });
document.addEventListener('tt-watchlist-renamed', e => {
  const { from, to } = /** @type {CustomEvent} */ (e).detail;
  const v = lay.list.find(x => x.watch === from); if (!v) return;
  v.watch = to;
  if (v.name === from && !lay.list.some(x => x.name === to)) { if (lay.current === from) lay.current = to; v.name = to; }
  // The list it shows, in its saved layout.
  const re = JSON.stringify(v.dock || v.tree).replaceAll(`"list":${JSON.stringify(from)}`, `"list":${JSON.stringify(to)}`);
  if (v.dock) v.dock = JSON.parse(re); else v.tree = JSON.parse(re);
  const seeded = new Set(parseList(SEEDED)); seeded.add(to); lsSet(SEEDED, JSON.stringify([...seeded]));
  layStore();
});
// A widget asking for a tab beside itself (the Calendar's day, opened as a
// Day tab): one such tab per workspace, reused and pointed at the new
// arguments, else added to the asking panel's group.
document.addEventListener('tt-open-tab', e => {
  const { view, args, title, from } = /** @type {CustomEvent} */ (e).detail || {};
  if (!dock || !VIEWS[view]) return;
  const ask = [...panelBody].find(([, b]) => b === from || b.contains(from))?.[0];
  const ref = dock.panels.find(p => p.id === ask) || activePanel();
  const open = dock.panels.find(p => p.params?.view === view);
  if (open) {
    open.api.updateParameters({ ...open.params, args });
    open.api.setTitle(title || VIEWS[view]);
    const body = panelBody.get(open.id);
    if (body) { body.dataset.args = JSON.stringify(args || {}); renderPanel(body, view); }
    open.api.setActive();
  } else {
    dock.addPanel({ id: panelId(), component: 'view', params: { view, args }, title: title || VIEWS[view], ...(ref ? { position: { referencePanel: ref, direction: 'within' } } : {}) });
  }
  laySave();
});
// A tab's widget changing its own arguments (a Day tab stepping a day), kept in the layout.
document.addEventListener('tt-tab-args', e => {
  const { args, title } = /** @type {CustomEvent} */ (e).detail || {};
  const id = [...panelBody].find(([, b]) => b.contains(/** @type {Node} */ (e.target)))?.[0], p = dock?.panels.find(x => x.id === id);
  if (!p) return;
  p.api.updateParameters({ ...p.params, args });
  if (title) p.api.setTitle(title);
  const body = panelBody.get(id); if (body) body.dataset.args = JSON.stringify(args || {});
  laySave();
});
// Show a view: activate it if open; otherwise put it in the active panel,
// beside it, as a tab with it, or along an edge of the whole workspace.
// Added from the palette or a panel's +, a widget is added even when the view
// has one (several charts); asked for by another widget (reuse), the one
// there is shown instead.
function wsShow(view, where, reuse = false) {
  if (!dock) return;
  const open = reuse && dock.panels.find(p => p.params?.view === view);
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
document.addEventListener('tt-show-view', e => { const v = /** @type {CustomEvent} */ (e).detail; if (state.tab === 'workspace' && VIEWS[v]) wsShow(v, 'right', true); });
// A symbol picked in a list (the options screener): its chart and its news,
// each reused if the workspace has one, else opened beside the asking panel
// (the chart to its right, the news under the chart).
document.addEventListener('tt-show-ticker', e => {
  const { symbol, from } = /** @type {CustomEvent} */ (e).detail || {};
  if (!dock || state.tab !== 'workspace' || typeof symbol !== 'string') return;
  const ask = [...panelBody].find(([, b]) => b === from || b.contains(from))?.[0];
  const ref = dock.panels.find(p => p.id === ask) || activePanel();
  const add = (view, position) => dock.addPanel({ id: panelId(), component: 'view', params: { view }, title: VIEWS[view], ...(position ? { position } : {}) });
  let chart = dock.panels.find(p => p.params?.view === 'candles');
  if (chart) chart.api.setActive(); else chart = add('candles', ref ? { referencePanel: ref, direction: 'right' } : undefined);
  const news = dock.panels.find(p => p.params?.view === 'news');
  if (news) news.api.setActive(); else add('news', { referencePanel: chart, direction: 'below' });
  laySave();
  // Once mounted, both pointed at it.
  requestAnimationFrame(() => {
    setFocus(symbol);
    document.dispatchEvent(new CustomEvent('tt-news-symbol', { detail: { symbol } }));
  });
});
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
  // This panel into another view (with its settings): a copy, or moved.
  const ap = activePanel();
  if (ap) for (const l of lay.list) {
    if (l.name === lay.current) continue;
    const send = move => () => {
      l.add = [...(l.add || []), { view: ap.params.view, args: ap.params.args, title: ap.api.title }];
      if (move && (dock?.panels.length || 0) > 1) ap.api.close();
      layStore(); wsnHint(`${ap.api.title || 'The panel'} ${move ? 'moved' : 'copied'} to ${l.name}`);
    };
    const where = `“${l.name}”${groupOf(l) && groupOf(l) !== l.name ? ` (${groupOf(l).split('/').join(' / ')})` : ''}`;
    out.push({ label: `Copy this panel to ${where}`, run: send(false) }, { label: `Move this panel to ${where}`, run: send(true) });
  }
  if ((dock?.panels.length || 0) > 1) out.push({ label: 'Close this panel', run: () => activePanel()?.api.close() });
  for (const [name, make] of Object.entries(LAYOUTS)) out.push({ label: `Apply template: ${name}`, run: () => { dock.clear(); buildTree(make()); } });
  for (const [name, [, make]] of Object.entries(READY)) out.push({ label: `New view in this folder: ${name}`, run: () => wsAdd(name, make()) });
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
const lay = { list: [], current: null, reset: false };
// A saved workspace's folder.
const groupOf = l => l?.group || '';
// The ready-made views' folders: market views together, operations together.
const HOME = { Market: 'Market', Watchlist: 'Market/Watchlists', 'Oil & Iran': 'Market', Earnings: 'Market', Operations: 'Operations', Data: 'Operations' };
// The side pane's settings and folders (below).
const WSN = { folders: 'tinytrader-folders', expanded: 'tinytrader-folders-open' };
const lsGet = k => { try { return localStorage.getItem(k); } catch (_) { return null; } };
const lsSet = (k, v) => { try { localStorage.setItem(k, v); } catch (_) {} };
const parseList = k => { try { const v = JSON.parse(lsGet(k) || '[]'); return Array.isArray(v) ? v : []; } catch (_) { return []; } };
// It lies over the workspace, so it starts hidden.
const wsn = { open: false, folders: parseList(WSN.folders), expanded: new Set(parseList(WSN.expanded)),
  /** @type {null | { kind: 'new' | 'rename', path: string, value: string }} */ edit: null, html: '',
  // The row picked with the keys.
  sel: '',
  /** @type {null | { kind: 'folder' | 'view', name: string }} */ confirm: null };
// Where ⌥ and the arrows are in the tree (below): the folder, what's picked in it.
const nav = { level: /** @type {string | null} */ (null), pick: /** @type {null | { kind: 'view' | 'folder', name: string }} */ (null), at: '',
  // The view last open in each folder.
  last: new Map(), timer: 0 };
const folderUp = p => p.includes('/') ? p.slice(0, p.lastIndexOf('/')) : '';
const leafOf = p => p.slice(p.lastIndexOf('/') + 1);
const joinPath = (a, b) => a ? `${a}/${b}` : b;
const under = (p, f) => p === f || p.startsWith(f + '/');
// Every folder: the ones made, and any a workspace is filed in, with their parents.
function allFolders() {
  const out = [];
  const add = p => { if (!p || out.includes(p)) return; add(folderUp(p)); out.push(p); };
  for (const f of wsn.folders) add(f);
  for (const l of lay.list) add(groupOf(l));
  return out;
}
// Wiping: every saved view and folder in this browser goes. "defaults"
// brings the ready-made views back; "blank" leaves one empty view, Home, and
// marks the ready-made ones as installed so they don't return. Also from a
// link: /#wipe-views (blank), so each device can be wiped by opening it.
const VIEW_KEYS = [LS.list, LS.current, 'tinytrader-ready', 'tinytrader-tree', 'tinytrader-ws-open', WSN.folders, WSN.expanded];
function wipeViews(how) {
  lay.reset = true; clearTimeout(layoutSaveTimer);
  for (const k of VIEW_KEYS) { try { localStorage.removeItem(k); } catch (_) {} }
  wsn.folders = []; wsn.expanded.clear();
  if (how === 'blank') {
    lsSet(LS.list, JSON.stringify([{ name: 'Home', group: 'Home', tree: { view: 'search' } }]));
    lsSet(LS.current, 'Home');
    lsSet('tinytrader-ready', JSON.stringify(Object.fromEntries(Object.entries(READY).map(([n, [v]]) => [n, v]))));
    lsSet('tinytrader-tree', '1');
  }
}
if (location.hash === '#wipe-views') { wipeViews('blank'); lay.reset = false; try { history.replaceState(null, '', '/'); } catch (_) {} }
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
      else if (lay.list.length < 100) lay.list.push(fresh);
      had[name] = version; changed = true;
    }
    if (changed) { localStorage.setItem('tinytrader-ready', JSON.stringify(had)); localStorage.setItem(LS.list, JSON.stringify(lay.list)); }
  } catch (_) {}
  if (!lay.list.some(l => l.name === lay.current)) lay.current = lay.list[0].name;
})();
// Once: the ready-made views filed in shared folders, where each was given
// a folder of its own (by name) before.
if (lsGet('tinytrader-tree') == null) {
  for (const l of lay.list) if (HOME[l.name] && l.group === l.name) l.group = HOME[l.name];
  lsSet('tinytrader-tree', '1');
}
// Once: the ready-made Watchlist view moves into its own folder, Market › Watchlists.
if (lsGet('tinytrader-tree-2') == null) {
  for (const l of lay.list) if (l.name === 'Watchlist' && l.group === 'Market') l.group = 'Market/Watchlists';
  lsSet('tinytrader-tree-2', '1');
}
homeless();
// A view for each watchlist, in a Watchlists folder inside the market
// folder (Market, or Markets if that's what yours is called): the list, a
// chart of the symbol picked and its news, so ⌥↑/⌥↓ step through the lists.
// Once the folder has been moved, new lists' views follow it there. A list
// gets its view once (when first seen); deleting the view doesn't bring it
// back, and a list renamed keeps its view, renamed with it.
const SEEDED = 'tinytrader-watch-views';
const watchView = name => ({ dir: 'row', ratio: 0.45, a: { dir: 'col', ratio: 0.55, a: { view: 'watchlist', args: { list: name } }, b: { view: 'candles' } }, b: { view: 'news' } });
function marketRoot() {
  const roots = allFolders().filter(f => !f.includes('/') && /^markets?$/i.test(f));
  // One of yours (with views of its own) before the ready-made Market.
  return roots.find(f => lay.list.some(l => under(groupOf(l), f) && !l.watch)) || roots[0] || 'Market';
}
function watchFolder() {
  const had = lay.list.find(v => v.watch);
  return had ? groupOf(had) : `${marketRoot()}/Watchlists`;
}
// The Watchlists folder left in a Market of its own, beside a Markets of
// yours: moved into yours.
function watchHome() {
  const f = watchFolder(), root = f.split('/')[0], want = marketRoot();
  if (f !== `${root}/Watchlists` || root === want || !/^markets?$/i.test(root)) return;
  if (lay.list.some(l => under(groupOf(l), root) && !under(groupOf(l), f))) return;
  moveFolder(f, `${want}/Watchlists`);
  wsn.folders = wsn.folders.filter(x => x !== root); wsn.expanded.delete(root); wsnKeep(); layStore();
}
function watchViews() {
  const seeded = new Set(parseList(SEEDED)), into = watchFolder();
  let added = false;
  for (const l of watchlists()) {
    if (seeded.has(l.name)) continue;
    seeded.add(l.name);
    if (lay.list.some(v => v.watch === l.name)) continue;
    let n = l.name; for (let k = 2; lay.list.some(v => v.name === n); k++) n = `${l.name} ${k}`;
    lay.list.push({ name: n, group: into, watch: l.name, tree: watchView(l.name) });
    added = true;
  }
  lsSet(SEEDED, JSON.stringify([...seeded]));
  if (added) for (let f = into; f; f = folderUp(f)) wsn.expanded.add(f);
  return added;
}
watchViews();
{ const f = lay.list.find(l => l.name === lay.current)?.group || ''; for (let p = f; p; p = folderUp(p)) wsn.expanded.add(p); }
// Every view lives in a workspace: one found loose (from before workspaces,
// a ready-made one, or moved out of a deleted workspace) gets a workspace of
// its own name.
function homeless() { for (const l of lay.list) if (!l.group) l.group = HOME[l.name] || l.name.replace(/\//g, '-'); }
function layStore() {
  if (lay.reset) return;
  // Kept in the tree's order, so ⌥↑/⌥↓ follow what the side pane shows.
  homeless(); wsn.folders = allFolders(); lay.list = treeOrder();
  try { localStorage.setItem(LS.list, JSON.stringify(lay.list)); localStorage.setItem(LS.current, lay.current ?? ''); localStorage.setItem(WSN.folders, JSON.stringify(wsn.folders)); } catch (_) {}
  updateLayoutNav();
}
function laySave() {
  const cur = lay.list.find(l => l.name === lay.current);
  if (cur && dock && !dock.narrow && state.tab === 'workspace') { cur.dock = dock.toJSON(); delete cur.tree; }
  layStore();
}
function laySwitch(name) {
  if (!lay.list.some(l => l.name === name)) return;
  if (dock && state.tab === 'workspace') laySave();
  lay.current = name;
  // Its folder opens in the side pane.
  for (let f = groupOf(lay.list.find(l => l.name === name)); f; f = folderUp(f)) wsn.expanded.add(f);
  wsnKeep(); layStore();
  nav.last.set(groupOf(lay.list.find(l => l.name === name)), name);
  state.tab = 'workspace'; state.doc = null;
  try { history.replaceState(null, '', '/#w-' + encodeURIComponent(name)); } catch (_) {}
  state.navigated = true; render(); window.scrollTo(0, 0);
}
// Groups, in order of first appearance; ungrouped workspaces form their own.
const groups = () => [...new Set(lay.list.map(groupOf))];
const inGroup = g => lay.list.filter(l => groupOf(l) === g);
const layViews = l => l.dock ? Object.values(l.dock.panels || {}).map(p => p.params?.view) : (function leaves(n) { return n ? (n.view ? [n.view] : n.tabs ? n.tabs : [...leaves(n.a), ...leaves(n.b)]) : []; })(l.tree);
// Views are listed in the side pane; the header names the open one.
const WS_MAX = 100;
// A new workspace, numbered next, with one panel to build on; it opens.
/** @param {string} [name] @param {{ tree?: object, dock?: object, theme?: object }} [start] */
function wsAdd(name, start = { tree: { view: 'search' } }, folder = groupOf(lay.list.find(l => l.name === lay.current))) {
  if (lay.list.length >= WS_MAX) return false;
  const base = name || `View ${lay.list.length + 1}`;
  let n = base; for (let k = 2; lay.list.some(l => l.name === n); k++) n = `${base} ${k}`;
  lay.list.push({ name: n, group: folder || undefined, ...JSON.parse(JSON.stringify(start)) });
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
  const p = near(e, '.wsn-ws[data-wsn-ws]');
  if (p) { e.preventDefault(); wmOpen(p.dataset.wsnWs, e.clientX, e.clientY + 4); return; }
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
  if (act === 'delete-yes') { deleteView(l.name); updateLayoutNav(); }
  else if (act === 'duplicate') { if (dock && state.tab === 'workspace' && l.name === lay.current) laySave(); wsAdd(l.name + ' copy', l.dock ? { dock: l.dock } : { tree: l.tree }); }
  else if (act === 'rename') { if (l.name !== lay.current) laySwitch(l.name); palOpen('rename '); }
}, true);
document.addEventListener('keydown', e => { if (e.key === 'Escape' && $('wm')) wmClose(); });

function updateLayoutNav() {
  wsnDraw();
  const box = $('wtabs'); if (!box) return;
  const docs = state.tab.startsWith('col:');
  const where = groupOf(lay.list.find(l => l.name === lay.current));
  const html = docs ? '' : `<button type="button" class="wtab wtab-name" data-wsn-toggle title="Views (⌥B) · ⌥↑⌥↓ this folder · ⌥→ into a folder · ⌥← up a level" aria-label="Folder ${esc(where || 'top level')}, view ${esc(lay.current || '')}">${where && where !== lay.current ? `<span class="muted">${esc(where.split('/').join(' / '))} ›</span>&nbsp;` : ''}${esc(lay.current || '')}</button>${tickerTools()}`;
  if (box.innerHTML !== html) box.innerHTML = html;
}
// A view on one ticker (a header search's) gets, beside its name, a menu
// to add the ticker to a watchlist and a button for its price alerts.
const viewTicker = () => { const t = lay.list.find(l => l.name === lay.current)?.theme; return t?.symbols?.length === 1 ? t.symbols[0] : ''; };
function tickerTools() {
  const sym = viewTicker(); if (!sym) return '';
  const lists = watchlists(), n = alertsFor(sym).length;
  return `<select class="wtab-wl" data-tk-list aria-label="Add ${esc(sym)} to a watchlist"><option value="">+ Watchlist</option>${lists.map(l => `<option value="${esc(l.name)}"${l.symbols.includes(sym) ? ' disabled' : ''}>${esc(l.name)}${l.symbols.includes(sym) ? ' ✓' : ''}</option>`).join('')}<option value="__new__">New list…</option></select>`
    + `<button type="button" class="wtab wtab-al${n ? ' on' : ''}" data-tk-alert aria-expanded="${!!$('tk-pop')}" title="Price alerts for ${esc(sym)}">${n ? `Alerts ${n}` : 'Alert'}</button>`;
}
function tkPop(open) {
  $('tk-pop')?.remove();
  if (open) {
    const b = /** @type {HTMLElement} */ (document.querySelector('[data-tk-alert]')); if (!b) return;
    const r = b.getBoundingClientRect(), sym = viewTicker();
    document.body.insertAdjacentHTML('beforeend', `<div class="tk-pop" id="tk-pop" style="top:${r.bottom + 6}px;left:${Math.max(8, Math.min(r.left, innerWidth - 380))}px">${alertForm(sym, null, 'data-tk-done')}</div>`);
    /** @type {HTMLElement | null} */ ($('tk-pop')?.querySelector('input'))?.focus();
  }
  updateLayoutNav();
}
document.addEventListener('tt-alerts', () => { const p = $('tk-pop'); if (p) { p.innerHTML = alertForm(viewTicker(), null, 'data-tk-done'); } updateLayoutNav(); });
document.addEventListener('tt-watchlist', () => updateLayoutNav());
document.addEventListener('change', e => {
  const s = /** @type {HTMLSelectElement} */ (e.target); if (!s.matches?.('[data-tk-list]') || !s.value) return;
  const sym = viewTicker();
  if (s.value === '__new__') addList('New list', [sym]); else addSymbols(s.value, [sym]);
  wsnHint(`${sym} added to ${s.value === '__new__' ? '“New list” (rename it in its Watchlist)' : s.value}`);
});
document.addEventListener('click', e => {
  if (near(e, '[data-tk-alert]')) { tkPop(!$('tk-pop')); return; }
  if (near(e, '[data-tk-done]') || ($('tk-pop') && !near(e, '#tk-pop'))) tkPop(false);
});
document.addEventListener('keydown', e => { if (e.key === 'Escape' && $('tk-pop')) tkPop(false); });
// The side pane (⌥B, or ☰ in the header): a tree of folders (parents) and
// views (leaves). A view is one saved layout (`lay.list`, called a workspace
// in the code), with its tabs under it; a folder is a path in each layout's
// `group` ("Market/Options"), nested as deep as you like and kept when empty.
// Every view is in a folder. ⌥↑/⌥↓ walk the views under the current
// top-level folder. Drag a view onto a folder to file it there, onto another
// view to put it before that one, or onto the pane's title for a folder of
// its own; folders drag the same way. Right-click a view for its menu. It
// slides over the page (nothing under it moves) and goes away on a pick, Esc
// or a click outside it.
const kids = (fs, p) => fs.filter(f => folderUp(f) === p);
// Workspaces in the tree's order: a folder's own, then its folders', depth first.
function treeOrder() {
  const fs = allFolders(), out = [];
  const walk = p => { out.push(...lay.list.filter(l => groupOf(l) === p)); for (const f of kids(fs, p)) walk(f); };
  walk('');
  return out;
}
function wsnKeep() { lsSet(WSN.expanded, JSON.stringify([...wsn.expanded])); }
function wsnOpen(name) { if (name !== lay.current) laySwitch(name); }
// A note beside the header's name for a moment (⌥↑ with nowhere to go).
let hintTimer = 0;
function wsnHint(text) {
  const box = $('wtabs'); if (!box) return;
  box.querySelector('.wtab-hint')?.remove();
  box.insertAdjacentHTML('beforeend', `<span class="wtab-hint muted">${esc(text)}</span>`);
  clearTimeout(hintTimer); hintTimer = setTimeout(() => box.querySelector('.wtab-hint')?.remove(), 3500);
}
// Move or rename a folder (and everything in it) to a new path.
function moveFolder(from, to) {
  if (!to || from === to || under(to, from)) return;
  const re = p => under(p, from) ? to + p.slice(from.length) : p;
  for (const l of lay.list) if (groupOf(l) && under(groupOf(l), from)) l.group = re(groupOf(l));
  wsn.folders = [...new Set(wsn.folders.map(re))];
  wsn.expanded = new Set([...wsn.expanded].map(re)); wsn.expanded.add(to); wsnKeep();
  layStore();
}
// Delete a folder and everything in it (asked first in its row); a view's
// folder goes with its last view. At least one view always stays.
function deleteFolder(f) {
  const left = lay.list.filter(l => !under(groupOf(l), f));
  if (!left.length) return false;
  lay.list = left;
  wsn.folders = wsn.folders.filter(x => !under(x, f));
  for (const x of [...wsn.expanded]) if (under(x, f)) wsn.expanded.delete(x);
  wsnKeep();
  if (!lay.list.some(l => l.name === lay.current)) { lay.current = lay.list[0].name; layStore(); if (state.tab === 'workspace') { state.navigated = true; render(); } }
  else layStore();
  return true;
}
function deleteView(name) {
  const l = lay.list.find(x => x.name === name); if (!l || lay.list.length < 2) return false;
  const f = groupOf(l), last = !lay.list.some(x => x !== l && under(groupOf(x), f));
  layDelete(name);
  if (last && !wsn.folders.some(x => x !== f && under(x, f))) { wsn.folders = wsn.folders.filter(x => x !== f); layStore(); }
  return true;
}
function wsnToggle(open = !wsn.open) {
  wsn.open = open;
  document.body.classList.toggle('wsn-open', open);
  $('wsn-toggle')?.setAttribute('aria-expanded', String(open));
  wsnDraw();
}
/** @returns {any[]} */
function wsnRows() {
  const fs = allFolders(), rows = [];
  const walk = (p, d) => {
    for (const l of lay.list.filter(x => groupOf(x) === p)) rows.push({ kind: 'ws', l, d });
    for (const f of kids(fs, p)) {
      const open = wsn.expanded.has(f), n = lay.list.filter(x => groupOf(x) && under(groupOf(x), f)).length;
      rows.push({ kind: 'folder', f, d, open, n });
      if (wsn.edit?.kind === 'new' && wsn.edit.path === f && open) rows.push({ kind: 'edit', d: d + 1 });
      if (open) walk(f, d + 1);
    }
  };
  if (wsn.edit?.kind === 'new' && !wsn.edit.path) rows.push({ kind: 'edit', d: 0 });
  walk('', 0);
  return rows;
}
function wsnDraw() {
  const box = $('wsnav'); if (!box) return;
  document.body.classList.toggle('wsn-open', wsn.open);
  if (!wsn.open) return;
  const input = v => `<input class="wsn-in" id="wsn-in" value="${esc(v)}" placeholder="Folder name, then Enter" autocomplete="off" spellcheck="false">`;
  const rows = wsnRows().map(r => {
    const key = rowKey(r), sel = key === wsn.sel;
    const pad = `style="--d:${r.d}" data-key="${esc(key)}" tabindex="${sel ? 0 : -1}"${sel ? ' data-sel' : ''}`;
    if (r.kind === 'edit') return `<li class="wsn-row" ${pad}>${input(wsn.edit?.value || '')}</li>`;
    const asking = wsn.confirm && ((r.kind === 'folder' && wsn.confirm.kind === 'folder' && wsn.confirm.name === r.f) || (r.kind === 'ws' && wsn.confirm.kind === 'view' && wsn.confirm.name === r.l.name));
    if (asking) {
      const views = r.kind === 'folder' ? lay.list.filter(l => under(groupOf(l), r.f)).length : 1, all = views >= lay.list.length;
      const what = r.kind === 'folder' ? `“${esc(leafOf(r.f))}” and ${views === 1 ? 'its view' : `its ${views} views`}` : `the view “${esc(r.l.name)}”`;
      return `<li class="wsn-row wsn-confirm" ${pad}><span>${all ? `Can't delete ${what}: it would leave no views.` : `Delete ${what}?`}</span>${all ? '' : '<button type="button" class="btn small danger" data-wsn-act="yes">Delete</button>'}<button type="button" class="btn small" data-wsn-act="no">${all ? 'OK' : 'Keep'}</button></li>`;
    }
    if (r.kind === 'folder') {
      if (wsn.edit?.kind === 'rename' && wsn.edit.path === r.f) return `<li class="wsn-row" ${pad}>${input(wsn.edit.value)}</li>`;
      return `<li class="wsn-row wsn-folder" ${pad} data-wsn-folder="${esc(r.f)}" draggable="true" role="treeitem" aria-expanded="${r.open}"><span class="wsn-tw">${r.open ? '▾' : '▸'}</span><span class="wsn-name">${esc(leafOf(r.f))}</span><em>${r.n}</em>
        <span class="wsn-acts"><button type="button" data-wsn-act="add" title="New view in this folder" aria-label="New view in ${esc(r.f)}">+</button><button type="button" data-wsn-act="sub" title="New folder inside this one" aria-label="New folder in ${esc(r.f)}">⊞</button><button type="button" data-wsn-act="rename" title="Rename" aria-label="Rename ${esc(r.f)}">✎</button><button type="button" data-wsn-act="delete" title="Delete this folder and the views in it" aria-label="Delete ${esc(r.f)}">×</button></span></li>`;
    }
    const here = state.tab === 'workspace' && r.l.name === lay.current;
    return `<li class="wsn-row wsn-ws${here ? ' current' : ''}" ${pad} data-wsn-ws="${esc(r.l.name)}" draggable="true" role="treeitem" title="${esc(r.l.name)}: ${esc(layViews(r.l).map(v => VIEWS[viewOf(v)] || v).join(', '))} · right-click to rename, theme, duplicate or delete"${here ? ' aria-current="page"' : ''}><span class="wsn-dot" aria-hidden="true"></span><span class="wsn-name">${esc(r.l.name)}</span><span class="wsn-acts"><button type="button" data-wsn-act="delete-view" title="Delete this view" aria-label="Delete ${esc(r.l.name)}">×</button></span></li>`;
  }).join('');
  const html = `<div class="wsn-head" data-wsn-root title="Drop here for the top level"><b>Views</b>
      <span class="wsn-acts on"><button type="button" data-wsn-act="folder" title="New folder" aria-label="New folder">⊞</button><button type="button" data-wsn-toggle title="Hide (⌥B)" aria-label="Hide the side pane">×</button></span></div>
    <ul class="wsn-tree" role="tree" aria-label="Views">${rows}</ul>
    <p class="wsn-opt">↑↓ pick · → ← open and close folders · Enter opens (a folder: its last view) · N a folder inside · F2 rename · Esc · Hover a folder for + (view), ⊞ (folder inside), ✎, ×; drag a folder onto another to nest it. Anywhere: ⌥↑⌥↓ this folder, ⌥→ into a folder, ⌥← up a level.</p>`;
  if (html === wsn.html) return;
  // Not while typing a folder's name.
  if (document.activeElement?.id === 'wsn-in' && wsn.edit) return;
  const had = box.contains(document.activeElement);
  box.innerHTML = html; wsn.html = html;
  if ($('wsn-in')) $('wsn-in').focus();
  else if (had) /** @type {HTMLElement | null} */ (box.querySelector('[data-sel]'))?.focus();
}
const rowKey = r => r.kind === 'ws' ? 'ws:' + r.l.name : r.kind === 'folder' ? 'folder:' + r.f : 'edit';
// ⌥B: into the pane, on the open view; leaving puts it away.
function wsnEnter() {
  if (!wsn.open) wsnToggle(true);
  wsn.sel = 'ws:' + lay.current; wsn.html = ''; wsnDraw();
  /** @type {HTMLElement | null} */ ($('wsnav')?.querySelector('[data-sel]'))?.focus();
}
function wsnLeave() {
  /** @type {HTMLElement} */ (document.activeElement)?.blur?.();
  wsnToggle(false);
}
document.addEventListener('keydown', e => {
  const box = $('wsnav'); if (!box || !box.contains(document.activeElement) || tgt(e).id === 'wsn-in' || e.altKey || e.metaKey || e.ctrlKey) return;
  const rows = wsnRows().filter(r => r.kind !== 'edit'), i = Math.max(0, rows.findIndex(r => rowKey(r) === wsn.sel)), r = rows[i];
  const go = j => { wsn.sel = rowKey(rows[Math.max(0, Math.min(rows.length - 1, j))]); wsnDraw(); };
  const k = e.key;
  if (k === 'ArrowDown' || k === 'ArrowUp') { e.preventDefault(); go(i + (k === 'ArrowDown' ? 1 : -1)); }
  else if (k === 'Home' || k === 'End') { e.preventDefault(); go(k === 'Home' ? 0 : rows.length - 1); }
  else if (k === 'ArrowRight' && r?.kind === 'folder') { e.preventDefault(); if (!r.open) { wsn.expanded.add(r.f); wsnKeep(); wsnDraw(); } else go(i + 1); }
  else if (k === 'ArrowLeft') {
    e.preventDefault();
    if (r?.kind === 'folder' && r.open) { wsn.expanded.delete(r.f); wsnKeep(); wsnDraw(); }
    else { const up = r?.kind === 'folder' ? folderUp(r.f) : r?.kind === 'ws' ? groupOf(r.l) : ''; if (up) { wsn.sel = 'folder:' + up; wsnDraw(); } }
  }
  else if (k === ' ' && r?.kind === 'folder') { e.preventDefault(); r.open ? wsn.expanded.delete(r.f) : wsn.expanded.add(r.f); wsnKeep(); wsnDraw(); }
  else if (k === 'Enter' || k === ' ') {
    // Enter opens what's picked and puts the pane away; on a folder, the view
    // last open in it, else its first.
    e.preventDefault();
    const v = r?.kind === 'ws' ? r.l : r?.kind === 'folder' ? (lay.list.find(l => l.name === nav.last.get(r.f) && under(groupOf(l), r.f)) || lay.list.find(l => under(groupOf(l), r.f))) : null;
    if (v) { wsnLeave(); wsnOpen(v.name); }
  }
  else if ((k === 'n' || k === 'N') && r) { e.preventDefault(); const f = r.kind === 'folder' ? r.f : groupOf(r.l); wsn.expanded.add(f); wsn.edit = { kind: 'new', path: f, value: '' }; wsn.html = ''; wsnDraw(); }
  else if (k === 'F2' && r?.kind === 'folder') { e.preventDefault(); wsn.edit = { kind: 'rename', path: r.f, value: leafOf(r.f) }; wsn.html = ''; wsnDraw(); }
  else if (k === 'Escape') { e.preventDefault(); wsnLeave(); }
});
function wsnCommit() {
  const e = wsn.edit; wsn.edit = null; if (!e) return wsnDraw();
  const name = e.value.replace(/\//g, '-').trim().slice(0, 40);
  if (name && e.kind === 'new') {
    const p = joinPath(e.path, name), fresh = !allFolders().includes(p);
    if (fresh) wsn.folders.push(p);
    for (let f = p; f; f = folderUp(f)) wsn.expanded.add(f);
    wsnKeep();
    // A new folder opens on a first view, named after it.
    if (fresh) { wsn.edit = null; wsAdd(name, undefined, p); wsnToggle(false); return; }
    layStore();
  }
  else if (name && e.kind === 'rename') moveFolder(e.path, joinPath(folderUp(e.path), name));
  wsn.html = ''; wsnDraw();
}
document.addEventListener('click', e => {
  // Opened with a click, the keys work in it too.
  if (near(e, '[data-wsn-toggle]')) { wsn.open ? wsnLeave() : wsnEnter(); return; }
  // A click anywhere else (but a menu it opened) puts it away.
  if (wsn.open && !$('wsnav')?.contains(tgt(e)) && !near(e, '#wm')) { wsnToggle(false); return; }
  const box = $('wsnav'); if (!box || !box.contains(tgt(e))) return;
  const key = near(e, '[data-key]')?.dataset.key; if (key) wsn.sel = key;
  const act = near(e, '[data-wsn-act]')?.dataset.wsnAct, folder = near(e, '[data-wsn-folder]')?.dataset.wsnFolder;
  if (act === 'folder') { wsn.edit = { kind: 'new', path: '', value: '' }; wsn.html = ''; wsnDraw(); return; }
  if (act === 'yes' || act === 'no') {
    const c = wsn.confirm; wsn.confirm = null;
    if (act === 'yes' && c) c.kind === 'folder' ? deleteFolder(c.name) : deleteView(c.name);
    wsn.html = ''; wsnDraw(); return;
  }
  if (act === 'delete-view') { const n = near(e, '[data-wsn-ws]')?.dataset.wsnWs; if (n) wsn.confirm = { kind: 'view', name: n }; wsn.html = ''; wsnDraw(); return; }
  if (folder != null && act) {
    if (act === 'add') { wsAdd(undefined, undefined, folder); wsnToggle(false); return; }
    else if (act === 'sub') { wsn.expanded.add(folder); wsn.edit = { kind: 'new', path: folder, value: '' }; }
    else if (act === 'rename') wsn.edit = { kind: 'rename', path: folder, value: leafOf(folder) };
    else if (act === 'delete') wsn.confirm = { kind: 'folder', name: folder };
    wsn.html = ''; wsnDraw(); return;
  }
  if (folder != null) { wsn.expanded.has(folder) ? wsn.expanded.delete(folder) : wsn.expanded.add(folder); wsnKeep(); wsnDraw(); return; }
  const ws = near(e, '[data-wsn-ws]')?.dataset.wsnWs;
  if (ws) { wsnOpen(ws); wsnToggle(false); }
});
document.addEventListener('keydown', e => {
  if (tgt(e).id !== 'wsn-in' || !wsn.edit) return;
  if (e.key === 'Enter') { e.preventDefault(); wsn.edit.value = tgt(e).value; wsnCommit(); }
  else if (e.key === 'Escape') { e.preventDefault(); wsn.edit = null; wsn.html = ''; wsnDraw(); }
}, true);
document.addEventListener('focusout', e => { if (tgt(e).id === 'wsn-in' && wsn.edit) { wsn.edit.value = tgt(e).value; setTimeout(wsnCommit); } });
// Drag and drop within the pane.
document.addEventListener('dragstart', e => {
  const r = near(e, '#wsnav [data-wsn-ws], #wsnav [data-wsn-folder]'); if (!r) return;
  e.dataTransfer.setData('text/x-wsn', r.dataset.wsnWs != null ? 'ws:' + r.dataset.wsnWs : 'folder:' + r.dataset.wsnFolder);
  e.dataTransfer.effectAllowed = 'move';
});
document.addEventListener('dragover', e => {
  const t = near(e, '#wsnav [data-wsn-ws], #wsnav [data-wsn-folder], #wsnav [data-wsn-root]');
  if (!t || !e.dataTransfer.types.includes('text/x-wsn')) return;
  e.preventDefault();
  document.querySelectorAll('#wsnav .drop').forEach(x => x.classList.remove('drop')); t.classList.add('drop');
});
document.addEventListener('dragend', () => document.querySelectorAll('#wsnav .drop').forEach(x => x.classList.remove('drop')));
document.addEventListener('drop', e => {
  const t = near(e, '#wsnav [data-wsn-ws], #wsnav [data-wsn-folder], #wsnav [data-wsn-root]'); if (!t) return;
  const src = e.dataTransfer.getData('text/x-wsn'); if (!src) return;
  e.preventDefault();
  const [kind, name] = [src.slice(0, src.indexOf(':')), src.slice(src.indexOf(':') + 1)];
  // Where it goes: into a folder, beside a workspace (its folder), or the top level.
  const dest = t.dataset.wsnFolder ?? (t.dataset.wsnWs != null ? groupOf(lay.list.find(l => l.name === t.dataset.wsnWs)) : '');
  if (kind === 'ws') {
    const l = lay.list.find(x => x.name === name); if (!l || name === t.dataset.wsnWs) return;
    lay.list.splice(lay.list.indexOf(l), 1);
    l.group = dest || undefined;
    const before = t.dataset.wsnWs != null ? lay.list.findIndex(x => x.name === t.dataset.wsnWs) : -1;
    if (before >= 0) lay.list.splice(before, 0, l); else lay.list.push(l);
    if (dest) wsn.expanded.add(dest);
    wsnKeep(); layStore();
  } else if (kind === 'folder') moveFolder(name, joinPath(dest, leafOf(name)));
});
// The header's ticker search, the same as the Search page's; / focuses it
// from anywhere but a text field.
{
  const box = /** @type {HTMLElement | null} */ (document.querySelector('.top-search'));
  if (box) bindSearch(box);
  document.addEventListener('keydown', e => {
    const t = /** @type {HTMLElement} */ (e.target);
    if (e.key !== '/' || e.metaKey || e.ctrlKey || e.altKey || t.closest('input, textarea, select, [contenteditable="true"]')) return;
    e.preventDefault(); box?.querySelector('input')?.focus();
  });
}
// A ticker searched in the header opens a view on it (its chart, news and
// earnings, the theme its symbol), in the Tickers folder; one already open
// for it is reused. The Search widget shows its results in place instead.
const tickerView = symbol => {
  const had = lay.list.find(l => l.name === symbol && l.theme?.symbols?.[0] === symbol);
  if (had) { laySwitch(had.name); return true; }
  return wsAdd(symbol, { tree: TICKER(), theme: { symbols: [symbol], keywords: [], layers: DEFAULT_LAYERS } }, 'Tickers');
};
document.addEventListener('tt-open-ticker', e => {
  const detail = /** @type {CustomEvent} */ (e).detail;
  detail.opened = tickerView(detail.symbol);
});
document.addEventListener('tt-open-ticker-view', e => { tickerView(/** @type {CustomEvent} */ (e).detail.symbol); });
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
// Keys, one level of the tree at a time, as in a file manager: ⌥↑/⌥↓ (or
// ⌃↑/⌃↓) step through what's in this folder, its views and then its
// folders; landing on a view opens it, landing on a folder only picks it.
// ⌥→ goes into the picked folder (to the view last open there, else its
// first); ⌥← goes up a level, picking the folder it came out of. A card
// shows the level when a folder is picked or the level changes; stepping
// between views shows none. ⌥W the switcher, ⌥B the side pane.
// What's in a folder, in the pane's order.
const navItems = f => [...lay.list.filter(l => groupOf(l) === f).map(l => ({ kind: /** @type {'view'} */ ('view'), name: l.name })),
  ...kids(allFolders(), f).map(x => ({ kind: /** @type {'folder'} */ ('folder'), name: x }))];
function navSync() {
  // Opened some other way since: start from where that view is.
  if (nav.level != null && nav.at === lay.current) return;
  nav.level = groupOf(lay.list.find(l => l.name === lay.current)); nav.pick = { kind: 'view', name: lay.current || '' }; nav.at = lay.current || '';
}
// Stepping onto a view just opens it (the header names it); the card shows
// only when a folder is picked or the level changes.
function navGo(item, card = item.kind === 'folder') {
  nav.pick = item;
  if (item.kind === 'view') { nav.last.set(nav.level, item.name); wsnOpen(item.name); nav.at = lay.current || ''; }
  if (card) navCard(); else navHide();
}
function navCard(hold = false) {
  const items = navItems(nav.level), crumbs = nav.level ? nav.level.split('/') : [];
  const row = it => {
    const on = nav.pick?.kind === it.kind && nav.pick.name === it.name, here = it.kind === 'view' && it.name === lay.current;
    const n = it.kind === 'folder' ? lay.list.filter(l => under(groupOf(l), it.name)).length : 0;
    return `<li class="nav-i${on ? ' on' : ''}${here ? ' here' : ''}${it.kind === 'folder' ? ' dir' : ''}">${it.kind === 'folder' ? `<span>▸</span><b>${esc(leafOf(it.name))}/</b><em>${n}</em>${on ? '<kbd>⌥→</kbd>' : ''}` : `<span>${here ? '●' : '○'}</span>${esc(it.name)}`}</li>`;
  };
  const html = `<p class="nav-path">${['Views', ...crumbs].map(esc).join(' <i>›</i> ')}</p><ul>${items.map(row).join('') || '<li class="muted">Empty</li>'}</ul>
    <p class="nav-keys">⌥↑⌥↓ this folder · ⌥→ into a folder · Enter its first view · ⌥← up a level${nav.level ? '' : ' (top)'}</p>`;
  let box = $('navcard');
  if (!box) { document.body.insertAdjacentHTML('beforeend', '<div class="navcard" id="navcard" role="status" aria-live="polite"></div>'); box = $('navcard'); }
  box.innerHTML = html; box.hidden = false;
  clearTimeout(nav.timer); if (!hold) nav.timer = setTimeout(navHide, 2600);
}
function navHide() { clearTimeout(nav.timer); const b = $('navcard'); if (b) b.hidden = true; }
// Enter (with ⌥ or without) while the card shows a folder picked: its first
// view (the first under it, depth first, if it has only folders).
document.addEventListener('keydown', e => {
  if (e.key !== 'Enter' || e.metaKey || e.ctrlKey || $('navcard')?.hidden !== false || nav.pick?.kind !== 'folder') return;
  if (['INPUT', 'TEXTAREA', 'SELECT'].includes(document.activeElement?.tagName)) return;
  const f = nav.pick.name, first = navItems(f).find(x => x.kind === 'view')?.name || lay.list.find(l => under(groupOf(l), f))?.name;
  if (!first) return;
  // Before ⌥Enter's maximise.
  e.preventDefault(); e.stopImmediatePropagation();
  nav.level = groupOf(lay.list.find(l => l.name === first)); navGo({ kind: 'view', name: first });
}, true);
document.addEventListener('keyup', e => { if (e.key === 'Alt' && $('navcard') && !$('navcard').hidden) { clearTimeout(nav.timer); nav.timer = setTimeout(navHide, 900); } });
document.addEventListener('keydown', e => {
  if (pal.open || sw.open || e.metaKey) return;
  if (['INPUT', 'TEXTAREA', 'SELECT'].includes(document.activeElement?.tagName)) return;
  if (e.altKey && e.code === 'KeyW') { e.preventDefault(); swOpen(); return; }
  if (e.altKey && e.code === 'KeyB') { e.preventDefault(); navHide(); $('wsnav')?.contains(document.activeElement) ? wsnLeave() : wsnEnter(); return; }
  const k = e.key, vert = (e.ctrlKey || e.altKey) && (k === 'ArrowUp' || k === 'ArrowDown'), horiz = e.altKey && (k === 'ArrowLeft' || k === 'ArrowRight');
  if (!vert && !horiz) return;
  e.preventDefault(); navSync();
  const items = navItems(nav.level), i = items.findIndex(x => x.kind === nav.pick?.kind && x.name === nav.pick.name);
  if (vert) {
    if (items.length) navGo(items[((i < 0 ? 0 : i + (k === 'ArrowDown' ? 1 : -1)) + items.length) % items.length]);
    return;
  }
  if (k === 'ArrowRight') {
    if (nav.pick?.kind !== 'folder') { navCard(); return; }
    const inside = navItems(nav.pick.name); if (!inside.length) { navCard(); return; }
    nav.level = nav.pick.name;
    const back = nav.last.get(nav.level);
    navGo(inside.find(x => x.kind === 'view' && x.name === back) || inside.find(x => x.kind === 'view') || inside[0], true);
    return;
  }
  if (nav.level) { nav.pick = { kind: 'folder', name: nav.level }; nav.level = folderUp(nav.level); }
  navCard();
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
  // Start over: every saved view and folder in this browser goes, and the ready-made ones come back.
  if (/^\s*reset views now\s*$/i.test(q)) out.unshift({ label: 'Yes: delete every saved view and folder in this browser, and start from the defaults', always: true, run: () => { wipeViews('defaults'); location.href = '/'; } });
  else if (/^\s*wipe views now\s*$/i.test(q)) out.unshift({ label: 'Yes: delete every saved view and folder in this browser, and start with one blank view', always: true, run: () => { wipeViews('blank'); location.href = '/'; } });
  else out.push({ label: 'Reset views to the defaults… (type: reset views now)', run: () => palOpen('reset views now') },
    { label: 'Wipe all views, starting blank… (type: wipe views now)', run: () => palOpen('wipe views now') });
  out.push(prof.on ? { label: 'Profile mode: turn off', run: stopProfile } : { label: 'Profile mode: turn on (measure everything on this page)', run: startProfile });
  if (lay.list.length > 1) out.push({ label: `Delete this workspace: ${lay.current}`, run: () => layDelete(lay.current) });
  return out;
}
function layDelete(name) {
  if (lay.list.length < 2) return;
  const i = lay.list.findIndex(l => l.name === name); if (i < 0) return;
  const [gone] = lay.list.splice(i, 1);
  const wasCurrent = lay.current === name, g = groupOf(gone);
  // From the open view to a neighbour in its workspace, if it has one.
  const sib = lay.list.filter(l => groupOf(l) === g);
  if (wasCurrent) { lay.current = (sib.find((l, k) => lay.list.indexOf(l) >= i || k === sib.length - 1) || lay.list[Math.max(0, i - 1)]).name; layStore(); state.navigated = true; render(); } else layStore();
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
    : sw.edit?.kind === 'new' ? '<input class="sw-in" id="sw-in" placeholder="New view name, then Enter" autocomplete="off">'
    : `${lay.list.length < WS_MAX ? '<button type="button" class="btn" data-sw-act="new">+ New view</button>' : `<span>${WS_MAX} workspaces, the most there can be.</span>`}<span>↑↓ move · Enter go · F2 rename · D duplicate · G group · ⌫ delete</span>`;
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
// or needing attention, and price alerts fired since it was last opened
// (lib/alerts.js); the list says which and why, and opens each service.
function alerts() {
  return (fleet.topo?.services || []).map(s => ({ s, st: statusOf({ ...s, kind: 'svc' }) })).filter(x => x.st === 'crit' || x.st === 'warn')
    .sort((a, b) => (a.st === 'crit' ? 0 : 1) - (b.st === 'crit' ? 0 : 1));
}
function updateHeaderStatus() {
  if (!fleet.topo) return;
  const list = alerts(), n = $('bell-n'), crit = list.some(x => x.st === 'crit'), count = list.length + unseen().length;
  n.hidden = !count; n.textContent = String(count); n.classList.toggle('crit', crit);
  $('bell').title = count ? `${count} alert${count === 1 ? '' : 's'}` : 'No alerts';
  if (!$('bell-pop').hidden) bellDraw();
}
function bellDraw() {
  const list = alerts();
  const fired = priceAlerts().filter(a => a.fired).sort((a, b) => b.fired - a.fired), armed = priceAlerts().filter(a => !a.fired).length;
  const when = t => etStamp(t);
  const prices = fired.map(a => `<div class="bell-item bell-price${a.seen ? '' : ' new'}"><i class="warn"></i><b><button type="button" class="linkish mono" data-alert-sym="${esc(a.symbol)}">${esc(a.symbol)}</button> ${a.price?.toFixed(2) ?? ''}</b><span>${esc(alertText(a))} · ${esc(when(a.fired))} <button type="button" class="chip-btn" data-al-rearm="${esc(a.id)}">Re-arm</button><button type="button" class="chip-btn" data-al-rm="${esc(a.id)}">Remove</button></span></div>`).join('');
  const html = '<h3>Alerts</h3>' + prices + (armed ? `<p class="bell-none">${armed} price alert${armed === 1 ? '' : 's'} armed (the Alerts widget lists them).</p>` : '') + (list.length
    ? list.map(x => `<button type="button" class="bell-item" data-alert="${esc(x.s.id)}"><i class="${x.st}"></i><b>${esc(x.s.id)} ${x.st === 'crit' ? 'is down' : 'needs attention'}</b><span>${esc(subOf({ ...x.s, kind: 'svc' }))}</span></button>`).join('')
    : '<p class="bell-none">Every service is running or off schedule.</p>');
  if ($('bell-pop').innerHTML !== html) $('bell-pop').innerHTML = html;
}
function bellToggle(open) {
  $('bell-pop').hidden = !open; $('bell').setAttribute('aria-expanded', open);
  if (open) { const r = $('bell').getBoundingClientRect(); $('bell-pop').style.top = r.bottom + 6 + 'px'; bellDraw(); }
  // Looked at: the count goes, the list stays (until removed or re-armed).
  else markSeen();
}
watchAlerts(() => serviceFor('snapshots') || null);
document.addEventListener('tt-alerts', () => { updateHeaderStatus(); if (!$('bell-pop').hidden) bellDraw(); });
$('bell').addEventListener('click', e => { e.stopPropagation(); bellToggle($('bell-pop').hidden); });
document.addEventListener('click', e => {
  const sym = near(e, '[data-alert-sym]');
  if (near(e, '#bell-pop [data-al-rearm], #bell-pop [data-al-rm]')) return;
  if (sym) { bellToggle(false); document.dispatchEvent(new CustomEvent('tt-open-ticker-view', { detail: { symbol: sym.dataset.alertSym } })); return; }
  const a = near(e, '[data-alert]');
  if (a) { bellToggle(false); const home = lay.list.find(l => layViews(l).includes('overview')); if (home) { laySwitch(home.name); showNode(a.dataset.alert); } return; }
  if (!$('bell-pop').hidden && !near(e, '.bell-wrap')) bellToggle(false);
});
document.addEventListener('keydown', e => { if (e.key === 'Escape' && !$('bell-pop').hidden) bellToggle(false); });
// The write gate's note for the banner ('' when writes are on for this viewer); see loadWriteGate.
let gateNote = '';
function status() {
  updateHeaderStatus();
  const age = state.lastOk ? (Date.now() - state.lastOk) / 1000 : null;
  const stale = age == null || age > 6;
  const problems = [];
  if (stale && age != null) problems.push('The console backend is not responding. Data shown may be out of date.');
  const d = fleet.snap?.docker;
  if (d?.configured && d.error) problems.push('Docker API unavailable: ' + d.error + '. Container state is missing.');
  if (fleet.snap?.error) problems.push(fleet.snap.error);
  if (gateNote) problems.push(gateNote);
  if (refusedNote && !gateNote) problems.push(refusedNote);
  $('banner').hidden = !problems.length;
  $('banner').textContent = problems.join(' ');
}
document.addEventListener('tt-prefs-refused', status);
// The console's write gate as this viewer meets it (GET /api/write-gate): when
// writes are off, the banner says so, since every change made here is refused.
async function loadWriteGate() {
  const g = await writeGate();
  gateNote = !g ? ''
    : !g.writes ? 'Changes are off: the console has no allowed logins configured (CONSOLE_WRITE_LOGINS). Orders, settings, the journal and reminders are read-only.'
    : g.hosts === false ? 'Changes are off: the console has no allowed host names configured (CONSOLE_HOSTS). Orders, settings, the journal and reminders are read-only.'
    : !g.allowed ? 'Changes are off here: this login, or the address the console was opened at, is not one the console allows. Orders, settings, the journal and reminders are read-only.'
    : '';
  status();
}
async function loadTopology() {
  try { const r = await fetch('/api/topology', { cache: 'no-store' }); if (r.ok) { setTopology(await r.json()); document.title = fleet.topo.name ? `tinytrader · ${fleet.topo.name}` : 'tinytrader'; } } catch (_) {}
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
  watchHome();
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
  await loadTopology(); await poll(); loadWriteGate();
  setInterval(loadWriteGate, 60000);
  setInterval(poll, 2000);
  setInterval(loadTopology, 5000);
})();

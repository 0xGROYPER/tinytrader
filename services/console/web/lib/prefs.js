// Settings that follow you between devices. The page keeps its settings in
// browser storage as before; the ones below are also kept by the console
// (`/api/prefs`), read before anything else on the page runs (every module
// that reads them at load imports this first), and saved there on every
// change. What stays per device: which view is open, which folders are open,
// the watchlist picked, panel sizes.
//
// The console's copy wins, except the first time a device syncs: then its
// watchlists, alerts and watchlist views are merged in (by name or id), so
// no device's lists are lost; its views and folders, if they differ from the
// console's, are kept aside in `tinytrader-workspaces-before-sync`.

const SYNCED = new Set([
  'tinytrader-workspaces', 'tinytrader-ready', 'tinytrader-tree', 'tinytrader-tree-2', 'tinytrader-folders',
  'tinytrader-watchlists', 'tinytrader-watch-views', 'tinytrader-alerts', 'tinytrader-board',
  'tinytrader-palette', 'tinytrader-theme', 'tt-hm-period',
]);
/** How the first sync merges a device's own copy: union by this field (or the values themselves). */
const MERGE = { 'tinytrader-watchlists': 'name', 'tinytrader-alerts': 'id', 'tinytrader-watch-views': '' };
const FIRST = 'tinytrader-prefs-synced';

const store = (() => { try { return window.localStorage; } catch (_) { return null; } })();
const setRaw = Storage.prototype.setItem, removeRaw = Storage.prototype.removeItem, getRaw = Storage.prototype.getItem;
const get = k => { try { return store ? getRaw.call(store, k) : null; } catch (_) { return null; } };
const put = (k, v) => { try { if (store) v == null ? removeRaw.call(store, k) : setRaw.call(store, k, v); } catch (_) {} };

/** The console's copy, or null when it can't be had (offline, or an older console). */
let remote = null;
try {
  const r = await fetch('/api/prefs', { cache: 'no-store', signal: AbortSignal.timeout(3000) });
  if (r.ok) remote = await r.json();
} catch (_) {}

// Saving: each change, a moment later (several changes to a key send once).
const waiting = new Set();
let timer = 0;
function send(k) {
  if (!remote) return;
  waiting.add(k);
  clearTimeout(timer);
  timer = setTimeout(() => {
    for (const key of waiting) {
      const v = get(key);
      fetch(`/api/prefs/${encodeURIComponent(key)}`, { method: 'PUT', headers: { 'content-type': 'application/json' }, body: JSON.stringify(v) }).catch(() => {});
    }
    waiting.clear();
  }, 400);
}

const parse = t => { try { return JSON.parse(t); } catch (_) { return null; } };
if (remote && store) {
  const first = get(FIRST) == null;
  for (const k of SYNCED) {
    const mine = get(k), theirs = typeof remote[k] === 'string' ? remote[k] : null;
    if (first && k in MERGE && mine != null && theirs != null && mine !== theirs) {
      const a = parse(theirs), b = parse(mine), by = MERGE[k];
      if (Array.isArray(a) && Array.isArray(b)) {
        const key = x => by ? x?.[by] : x;
        const merged = [...a, ...b.filter(x => !a.some(y => key(y) === key(x)))];
        put(k, JSON.stringify(merged));
        if (merged.length !== a.length) send(k);
        continue;
      }
    }
    if (theirs != null) {
      if (first && k === 'tinytrader-workspaces' && mine != null && mine !== theirs) put('tinytrader-workspaces-before-sync', mine);
      put(k, theirs);
    } else if (mine != null) send(k);
  }
  put(FIRST, String(Date.now()));
}

// Every later change to a synced setting is saved to the console too.
Storage.prototype.setItem = function (k, v) {
  setRaw.call(this, k, v);
  if (this === store && SYNCED.has(k)) send(k);
};
Storage.prototype.removeItem = function (k) {
  removeRaw.call(this, k);
  if (this === store && SYNCED.has(k)) send(k);
};

/** Whether this device's settings come from the console (false offline). */
export const synced = !!remote;

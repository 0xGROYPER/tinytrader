// Settings that follow you between devices. The page keeps its settings in
// browser storage as before; the ones below are also kept by the console
// (`/api/prefs`), read before anything else on the page runs (every module
// that reads them at load imports this first), and saved there on every
// change. What stays per device: which view is open, which folders are open,
// the watchlist picked, panel sizes.
//
// A save names the version it started from; if the console's copy has
// changed since (another device, or a tab left open), it's refused, and the
// page merges the two (lists and views by name, alerts by id, its own winning
// where both have one) and saves the merge, so a stale page never drops what
// another saved. Coming back to a tab fetches the latest: watchlists and
// alerts update in place (tt-prefs), views on the next reload.
//
// The first time a device syncs, its watchlists, alerts and watchlist views
// are merged in too; its views, if they differ, are kept aside in
// `tinytrader-workspaces-before-sync`.
//
// A save is a write like the others (lib/api.js): it needs the console's login
// allowlist, not the order token. A save the console refuses is shown in the page's banner
// (`refusedNote`, tt-prefs-refused) until one goes through.

import { orderFetch } from './api.js';

const SYNCED = new Set([
  'tinytrader-workspaces', 'tinytrader-ready', 'tinytrader-tree', 'tinytrader-tree-2', 'tinytrader-folders',
  'tinytrader-watchlists', 'tinytrader-watch-views', 'tinytrader-alerts', 'tinytrader-board',
  'tinytrader-palette', 'tinytrader-theme', 'tt-hm-period',
]);
/** How two copies merge: union of arrays by this field (or the values themselves). */
const MERGE = { 'tinytrader-watchlists': 'name', 'tinytrader-alerts': 'id', 'tinytrader-watch-views': '', 'tinytrader-workspaces': 'name' };
const FIRST = 'tinytrader-prefs-synced';
/** Keys the page holds in memory once loaded (views and folders). */
const VIEWS = new Set(['tinytrader-workspaces', 'tinytrader-ready', 'tinytrader-tree', 'tinytrader-tree-2', 'tinytrader-folders']);

const store = (() => { try { return window.localStorage; } catch (_) { return null; } })();
const setRaw = Storage.prototype.setItem, removeRaw = Storage.prototype.removeItem, getRaw = Storage.prototype.getItem;
const get = k => { try { return store ? getRaw.call(store, k) : null; } catch (_) { return null; } };
const put = (k, v) => { try { if (store) v == null ? removeRaw.call(store, k) : setRaw.call(store, k, v); } catch (_) {} };

/** SHA-256 of a string's UTF-8, as hex: a value's version, as the console computes it. */
function sha256(text) {
  const K = [0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
    0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
    0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
    0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3, 0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2];
  const bytes = new TextEncoder().encode(text), n = bytes.length, total = ((n + 9 + 63) >> 6) << 6;
  const m = new Uint8Array(total); m.set(bytes); m[n] = 0x80;
  const bits = n * 8; for (let i = 0; i < 8; i++) m[total - 1 - i] = Math.floor(bits / 2 ** (8 * i)) & 0xff;
  const h = [0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19], w = new Uint32Array(64);
  const r = (x, k) => (x >>> k) | (x << (32 - k));
  for (let o = 0; o < total; o += 64) {
    for (let i = 0; i < 16; i++) w[i] = (m[o + 4 * i] << 24) | (m[o + 4 * i + 1] << 16) | (m[o + 4 * i + 2] << 8) | m[o + 4 * i + 3];
    for (let i = 16; i < 64; i++) {
      const s0 = r(w[i - 15], 7) ^ r(w[i - 15], 18) ^ (w[i - 15] >>> 3), s1 = r(w[i - 2], 17) ^ r(w[i - 2], 19) ^ (w[i - 2] >>> 10);
      w[i] = (w[i - 16] + s0 + w[i - 7] + s1) | 0;
    }
    let [a, b, c, d, e, f, g, hh] = h;
    for (let i = 0; i < 64; i++) {
      const t1 = (hh + (r(e, 6) ^ r(e, 11) ^ r(e, 25)) + ((e & f) ^ (~e & g)) + K[i] + w[i]) | 0;
      const t2 = ((r(a, 2) ^ r(a, 13) ^ r(a, 22)) + ((a & b) ^ (a & c) ^ (b & c))) | 0;
      hh = g; g = f; f = e; e = (d + t1) | 0; d = c; c = b; b = a; a = (t1 + t2) | 0;
    }
    h[0] = (h[0] + a) | 0; h[1] = (h[1] + b) | 0; h[2] = (h[2] + c) | 0; h[3] = (h[3] + d) | 0;
    h[4] = (h[4] + e) | 0; h[5] = (h[5] + f) | 0; h[6] = (h[6] + g) | 0; h[7] = (h[7] + hh) | 0;
  }
  return h.map(x => (x >>> 0).toString(16).padStart(8, '0')).join('');
}
const version = v => v == null ? 'absent' : sha256(v);

/** The console's copy, or null when it can't be had (offline, or an older console). */
async function fetchRemote() {
  try {
    const r = await fetch('/api/prefs', { cache: 'no-store', signal: AbortSignal.timeout(3000) });
    return r.ok ? await r.json() : null;
  } catch (_) { return null; }
}
let remote = await fetchRemote();
/** The version of each key this page last agreed with the console on. */
const known = new Map();

const parse = t => { try { return JSON.parse(t); } catch (_) { return null; } };
/** Merge two copies of a key: theirs, with ours added and winning where both have an entry. Null if not mergeable. */
function merge(k, theirs, mine) {
  if (!(k in MERGE)) return null;
  const a = parse(theirs), b = parse(mine), by = MERGE[k];
  if (!Array.isArray(a) || !Array.isArray(b)) return null;
  const key = x => by ? x?.[by] : x;
  const out = a.map(x => b.find(y => key(y) === key(x)) ?? x);
  for (const x of b) if (!a.some(y => key(y) === key(x))) out.push(x);
  return JSON.stringify(out);
}

// Saving: each change, a moment later. A refused save merges and tries again.
const waiting = new Set();
let timer = 0;
function send(k) {
  if (!remote) return;
  waiting.add(k);
  clearTimeout(timer);
  timer = setTimeout(() => { const keys = [...waiting]; waiting.clear(); for (const key of keys) save(key, 0); }, 400);
}
/** A save the console refused for a reason other than a stale version, shown in the banner ('' when none). */
export let refusedNote = '';
function refuse(note) {
  refusedNote = note;
  document.dispatchEvent(new CustomEvent('tt-prefs-refused'));
}
async function save(k, tries) {
  const v = get(k);
  let r;
  try {
    r = await orderFetch(`/api/prefs/${encodeURIComponent(k)}`, v, 'PUT', { 'if-match': known.get(k) ?? version(remote?.[k] ?? null) });
  } catch (_) { refuse('Settings were not saved: the console could not be reached.'); return; }
  if (r.ok) {
    known.set(k, version(v));
    if (refusedNote) refuse('');
    return;
  }
  if (r.status !== 409) { refuse(`Settings were not saved: the console refused the save (HTTP ${r.status}).`); return; }
  if (tries > 2) return;
  // Someone saved since: merge theirs with ours, keep it here, and save the merge.
  const theirs = (await r.json().catch(() => ({}))).current;
  const t = typeof theirs === 'string' ? theirs : null;
  known.set(k, version(t));
  const merged = t == null ? v : merge(k, t, v ?? '[]') ?? v;
  if (merged !== v) { put(k, merged); changed(k); }
  if (merged === t) return;
  save(k, tries + 1);
}

/** Tell the page a setting changed under it (another device): modules re-read. */
function changed(k) {
  document.dispatchEvent(new CustomEvent('tt-prefs', { detail: k }));
}

// Loading: the console's copy wins (the first sync of a device merges).
if (remote && store) {
  const first = get(FIRST) == null;
  for (const k of SYNCED) {
    const mine = get(k), theirs = typeof remote[k] === 'string' ? remote[k] : null;
    known.set(k, version(theirs));
    if (first && mine != null && theirs != null && mine !== theirs && k !== 'tinytrader-workspaces') {
      const merged = merge(k, theirs, mine);
      if (merged) { put(k, merged); if (merged !== theirs) send(k); continue; }
    }
    if (theirs != null) {
      if (first && k === 'tinytrader-workspaces' && mine != null && mine !== theirs) put('tinytrader-workspaces-before-sync', mine);
      put(k, theirs);
    } else if (mine != null) send(k);
  }
  put(FIRST, String(Date.now()));
}

// Coming back to a tab: take what changed elsewhere, where this page hasn't
// changed it since.
document.addEventListener('visibilitychange', async () => {
  if (document.hidden || !store) return;
  const latest = await fetchRemote();
  if (!latest) return;
  remote = latest;
  for (const k of SYNCED) {
    const theirs = typeof latest[k] === 'string' ? latest[k] : null, tv = version(theirs);
    if (tv === known.get(k) || waiting.has(k)) continue;
    // Views live in the page's memory: they're taken on the next reload, and
    // a save before then is refused and merged rather than overwriting them.
    if (VIEWS.has(k)) continue;
    if (version(get(k)) !== known.get(k)) { send(k); continue; } // changed here too: the save merges
    put(k, theirs); known.set(k, tv); changed(k);
  }
});

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

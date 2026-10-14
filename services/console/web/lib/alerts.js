// Price alerts: per symbol, kept by the console and shared between devices
// (lib/prefs.js). Each is a price to cross
// (above or below) or a move on the day (up or down by at least a percent).
// While a console page is open, armed alerts are checked against stock
// snapshots (alpaca-data, IEX prices) every 15 seconds; one that's met fires
// once: it's listed under the bell, and shown as a notification when the
// browser allows them. Re-arming it starts it again. Several tabs share the
// alerts (and fire each once); nothing watches with every page closed.

import './prefs.js'; // settings shared between devices, read first

/** @typedef {{ id: string, symbol: string, kind: 'above' | 'below' | 'move', value: number, made: number,
 *   fired: number | null, price: number | null, change: number | null, seen: boolean }} Alert */

const KEY = 'tinytrader-alerts', EVERY = 15e3;
export const KINDS = { above: 'rises to', below: 'falls to', move: 'moves (either way) by' };

/** @returns {Alert[]} */
function read() { try { const v = JSON.parse(localStorage.getItem(KEY) || '[]'); return Array.isArray(v) ? v : []; } catch (_) { return []; } }
let list = read();
function save() {
  try { localStorage.setItem(KEY, JSON.stringify(list)); } catch (_) {}
  document.dispatchEvent(new CustomEvent('tt-alerts'));
}
// Another tab's changes.
window.addEventListener('storage', e => { if (e.key === KEY) { list = read(); document.dispatchEvent(new CustomEvent('tt-alerts')); } });

export const alerts = () => list;
/** @param {string} symbol */
export const alertsFor = symbol => list.filter(a => a.symbol === symbol);
/** Fired and not yet looked at. */
export const unseen = () => list.filter(a => a.fired && !a.seen);

/** What an alert waits for, as text. @param {Alert} a */
export function describe(a) {
  return a.kind === 'move' ? `${a.symbol} moves ${a.value}% on the day` : `${a.symbol} ${a.kind === 'above' ? 'at or above' : 'at or below'} ${a.value}`;
}

/** @param {string} symbol @param {'above' | 'below' | 'move'} kind @param {number} value */
export function addAlert(symbol, kind, value) {
  if (!symbol || !KINDS[kind] || !(value > 0)) return null;
  const a = { id: `${Date.now().toString(36)}${Math.random().toString(36).slice(2, 6)}`, symbol, kind, value, made: Date.now(), fired: null, price: null, change: null, seen: false };
  list = [...read(), a]; save();
  // Asked once, from the click that made the first alert.
  try { if ('Notification' in window && Notification.permission === 'default') Notification.requestPermission(); } catch (_) {}
  return a;
}
/** @param {string} id */
export function removeAlert(id) { list = read().filter(a => a.id !== id); save(); }
/** @param {string} id */
export function rearm(id) { list = read().map(a => a.id === id ? { ...a, fired: null, price: null, change: null, seen: false } : a); save(); }
/** Fired alerts, looked at. */
export function markSeen() { if (!unseen().length) return; list = read().map(a => a.fired ? { ...a, seen: true } : a); save(); }

const esc = s => String(s).replace(/[&<>"']/g, c => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' })[c]);
const when = t => new Date(t).toLocaleString(undefined, { month: 'short', day: 'numeric', hour: 'numeric', minute: '2-digit' });
/**
 * A symbol's alerts and a form for another, for any widget to show; this
 * module handles its buttons and its submit (on the document).
 * @param {string} sym @param {number | null | undefined} price for the placeholder @param {string} [done] a data-act for a Done button
 */
export function alertForm(sym, price, done = '') {
  const items = alertsFor(sym).map(a => `<li class="${a.fired ? 'fired' : ''}">${esc(describe(a))}${a.fired ? ` <span class="muted">fired ${esc(when(a.fired))} at ${a.price?.toFixed(2) ?? '?'}</span> <button type="button" class="chip-btn" data-al-rearm="${esc(a.id)}">Re-arm</button>` : ''} <button type="button" class="sb-x" data-al-rm="${esc(a.id)}" aria-label="Remove this alert" title="Remove">×</button></li>`).join('');
  return `<form class="al-form" data-al-form="${esc(sym)}"><b class="mono">${esc(sym)}</b><span>alert when it</span>
    <select name="k" aria-label="When">${Object.entries(KINDS).map(([k, t]) => `<option value="${k}">${esc(t)}</option>`).join('')}</select>
    <input class="wl-in mono" name="v" type="number" step="any" min="0" placeholder="${price ? price.toFixed(2) : 'price'}" aria-label="Price, or percent for a move" required>
    <button class="btn small primary">Set</button>${done ? `<button type="button" class="btn small" ${done}>Done</button>` : ''}
    ${items ? `<ul>${items}</ul>` : ''}<p class="muted">A move is in percent. Checked every 15 s while a console page is open (IEX prices); one that's met fires once, under the bell and as a notification.</p></form>`;
}
document.addEventListener('click', e => {
  const t = /** @type {HTMLElement} */ (e.target), rm = /** @type {HTMLElement | null} */ (t.closest?.('[data-al-rm]')), re = /** @type {HTMLElement | null} */ (t.closest?.('[data-al-rearm]'));
  if (rm) removeAlert(rm.dataset.alRm || ''); else if (re) rearm(re.dataset.alRearm || '');
});
document.addEventListener('submit', e => {
  const f = /** @type {HTMLFormElement} */ (e.target); if (!f.matches?.('form[data-al-form]')) return;
  e.preventDefault();
  const k = /** @type {HTMLSelectElement} */ (f.elements.namedItem('k')).value, v = +/** @type {HTMLInputElement} */ (f.elements.namedItem('v')).value;
  // Off the box first, so the form is redrawn with the new alert.
  /** @type {HTMLElement} */ (document.activeElement)?.blur?.();
  addAlert(f.dataset.alForm || '', /** @type {any} */ (k), v);
});

/** @param {Alert} a @param {{ price?: number, change_pct?: number }} r */
const met = (a, r) => a.kind === 'above' ? r.price >= a.value : a.kind === 'below' ? r.price <= a.value : r.change_pct != null && Math.abs(r.change_pct * 100) >= a.value;

/** Checking, from the console's shell: source() names the snapshot service. @param {() => string | null} source */
export function watch(source) {
  async function check() {
    const armed = read().filter(a => !a.fired), svc = source();
    if (!armed.length || !svc) return;
    const syms = [...new Set(armed.map(a => a.symbol))].slice(0, 200);
    let rows;
    try {
      const r = await fetch(`/api/stream/${encodeURIComponent(svc)}/snapshots?symbols=${encodeURIComponent(syms.join(','))}`, { cache: 'no-store' });
      if (!r.ok) return;
      rows = new Map(((await r.json()).snapshots || []).map(x => [x.symbol, x]));
    } catch (_) { return; }
    // Read again: another tab may have fired some meanwhile.
    const now = read(), fired = [];
    for (const a of now) {
      const r = rows.get(a.symbol);
      if (a.fired || !r || r.price == null || !met(a, r)) continue;
      Object.assign(a, { fired: Date.now(), price: r.price, change: r.change_pct ?? null, seen: false });
      fired.push(a);
    }
    if (!fired.length) return;
    list = now; save();
    for (const a of fired) {
      try {
        if ('Notification' in window && Notification.permission === 'granted') {
          new Notification(`${a.symbol} ${a.price?.toFixed(2)}`, { body: `${describe(a)}${a.change != null ? ` (${a.change >= 0 ? '+' : '−'}${Math.abs(a.change * 100).toFixed(2)}% today)` : ''}`, tag: `tt-alert-${a.id}` });
        }
      } catch (_) {}
    }
  }
  setTimeout(check, 3000);
  return setInterval(check, EVERY);
}

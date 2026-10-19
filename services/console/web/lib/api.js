// The console's own APIs that widgets call.
export async function jobsApi(path, body) {
  const r = body === undefined ? await fetch('/api/jobs/' + path, { cache: 'no-store' })
    : await orderFetch('/api/jobs/' + path, body);
  let v = null; try { v = await r.json(); } catch (_) {}
  if (!r.ok) throw new Error(v?.error || (r.status === 404 ? 'No jobs service is configured.' : `${r.status} ${r.statusText}`));
  return v;
}
// Every write goes through the console's write gate (services/console/src/main.rs,
// WriteGate): the Tailscale login must be on CONSOLE_ORDER_LOGINS, or the
// write is refused (403). A live account's order, cancel, flatten or resume
// also carries the console's order token (CONSOLE_ORDER_TOKEN) as
// X-Order-Token; the server refuses it without one (401). Every other write
// sends no token. The token is asked for at the first live write in a tab and
// kept for that tab only (sessionStorage, or memory when storage is
// unavailable). A refused token is forgotten, so the next live write asks
// again; a write sent without one that the server says needs it (401: the
// server found the account live) asks and is sent once more, which is safe
// because the gate refuses before anything reaches a gateway. Storage can
// throw (private windows): the page still works.
const TOKEN_KEY = 'tinytrader-order-token';
let orderTok = null;
function setToken(t) {
  orderTok = t;
  try { if (t) sessionStorage.setItem(TOKEN_KEY, t); else sessionStorage.removeItem(TOKEN_KEY); } catch (_) {}
}
function getToken() {
  try { return sessionStorage.getItem(TOKEN_KEY) || orderTok; } catch (_) { return orderTok; }
}
function liveToken() {
  const have = getToken();
  if (have) return have;
  const t = (window.prompt('The console\'s order token, for a live account (kept for this tab only):') || '').trim() || null;
  setToken(t);
  return t;
}
/** A write request (`/api/orders/...`, flatten, resume, jobs, the queue, settings, the journal, ...): the response, as fetch gives it. `null` is sent as `null`; `extra` adds headers. `live` marks a write that acts on a live account, which sends the order token.
 * @param {string} path @param {any} body @param {string} [method] @param {Record<string, string>} [extra] @param {boolean} [live] @returns {Promise<Response>} */
export async function orderFetch(path, body, method = 'POST', extra = {}, live = false) {
  const go = t => fetch(path, { method, cache: 'no-store', headers: { 'content-type': 'application/json', ...extra, ...(t ? { 'x-order-token': t } : {}) }, body: JSON.stringify(body === undefined ? {} : body) });
  const t = live ? liveToken() : null;
  let r = await go(t);
  if (r.status === 401) {
    setToken(null);
    // Not marked live here, but the server found the account live: ask for the token and send once more.
    if (!t) { const again = liveToken(); if (again) { r = await go(again); if (r.status === 401) setToken(null); } }
  }
  return r;
}
/** Whether the console's write gate refused a write (nothing was sent on). @param {Response} r */
export const refused = r => r.status === 401 || r.status === 403;
/** A refused write in plain words, from the server's error. @param {Response} r @param {any} v the response's JSON */
export function refusedText(r, v) {
  const why = v?.error || (r.status === 401 ? 'the order token was refused' : 'this login may not make changes');
  return `refused by the console: ${why}${r.status === 401 ? '; the token is asked for again on the next try' : ''}`;
}
/** The console's write gate as the server reports it for this viewer: `{ writes, allowed, live_orders }`, or null when it can't be read. */
export async function writeGate() {
  try { const r = await fetch('/api/write-gate', { cache: 'no-store' }); return r.ok ? await r.json() : null; } catch (_) { return null; }
}
// The job queue (decision 0012), through the console's /api/queue proxy.
export async function queueApi(path, body, method) {
  const r = body === undefined && !method ? await fetch('/api/queue/' + path, { cache: 'no-store' })
    : await orderFetch('/api/queue/' + path, body, method || 'POST');
  let v = null; try { v = await r.json(); } catch (_) {}
  if (!r.ok) throw new Error(v?.error || (r.status === 404 ? 'No job queue is configured.' : `${r.status} ${r.statusText}`));
  return v;
}

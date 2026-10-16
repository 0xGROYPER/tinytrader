// The console's own APIs that widgets call.
export async function jobsApi(path, body) {
  const r = body === undefined ? await fetch('/api/jobs/' + path, { cache: 'no-store' })
    : await orderFetch('/api/jobs/' + path, body);
  let v = null; try { v = await r.json(); } catch (_) {}
  if (!r.ok) throw new Error(v?.error || (r.status === 404 ? 'No jobs service is configured.' : `${r.status} ${r.statusText}`));
  return v;
}
// A live account's order, cancel, flatten or resume carries the console's order
// token (CONSOLE_ORDER_TOKEN) as X-Order-Token; the server refuses it without
// one. Every other write needs only the Tailscale login, so it sends no token.
// The token is asked for at the first live write in a tab and kept for that
// tab only (sessionStorage, or memory when storage is unavailable). A refused
// token (401) is forgotten, so the next live write asks again. Storage can
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
/** A write request (`/api/orders/...`, flatten, resume, jobs, the queue, settings, ...): the response, as fetch gives it. `null` is sent as `null`; `extra` adds headers. `live` marks a write that acts on a live account, which sends the order token. */
export async function orderFetch(path, body, method = 'POST', extra = {}, live = false) {
  const t = live ? liveToken() : null;
  const r = await fetch(path, { method, headers: { 'content-type': 'application/json', ...extra, ...(t ? { 'x-order-token': t } : {}) }, body: JSON.stringify(body === undefined ? {} : body) });
  if (t && r.status === 401) setToken(null);
  return r;
}
// The job queue (decision 0012), through the console's /api/queue proxy.
export async function queueApi(path, body, method) {
  const r = body === undefined && !method ? await fetch('/api/queue/' + path, { cache: 'no-store' })
    : await orderFetch('/api/queue/' + path, body, method || 'POST');
  let v = null; try { v = await r.json(); } catch (_) {}
  if (!r.ok) throw new Error(v?.error || (r.status === 404 ? 'No job queue is configured.' : `${r.status} ${r.statusText}`));
  return v;
}

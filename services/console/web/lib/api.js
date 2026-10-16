// The console's own APIs that widgets call.
export async function jobsApi(path, body) {
  const r = body === undefined ? await fetch('/api/jobs/' + path, { cache: 'no-store' })
    : await orderFetch('/api/jobs/' + path, body);
  let v = null; try { v = await r.json(); } catch (_) {}
  if (!r.ok) throw new Error(v?.error || (r.status === 404 ? 'No jobs service is configured.' : `${r.status} ${r.statusText}`));
  return v;
}
// Writes carry the console's order token (CONSOLE_ORDER_TOKEN), which the
// server requires for the writes it gates (WriteGate). It's kept on this
// device once typed, and asked for the first time a write is sent; a refused
// token (401) is forgotten, so the next send asks again. Storage can throw
// (private windows): the page still works.
const TOKEN_KEY = 'tinytrader-order-token';
let orderTok = null;
function orderToken() {
  if (orderTok) return orderTok;
  try { orderTok = localStorage.getItem(TOKEN_KEY) || null; } catch (_) {}
  if (!orderTok) {
    orderTok = (window.prompt('The console\'s order token (kept on this device):') || '').trim() || null;
    try { if (orderTok) localStorage.setItem(TOKEN_KEY, orderTok); } catch (_) {}
  }
  return orderTok;
}
/** A write request (`/api/orders/...`, flatten, resume, jobs, the queue, ...): the response, as fetch gives it. */
export async function orderFetch(path, body, method = 'POST') {
  const t = orderToken();
  const r = await fetch(path, { method, headers: { 'content-type': 'application/json', ...(t ? { 'x-order-token': t } : {}) }, body: JSON.stringify(body ?? {}) });
  if (r.status === 401) { orderTok = null; try { localStorage.removeItem(TOKEN_KEY); } catch (_) {} }
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

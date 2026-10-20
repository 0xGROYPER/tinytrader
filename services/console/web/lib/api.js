// The console's own APIs that widgets call.
export async function jobsApi(path, body) {
  const r = body === undefined ? await fetch('/api/jobs/' + path, { cache: 'no-store' })
    : await orderFetch('/api/jobs/' + path, body);
  let v = null; try { v = await r.json(); } catch (_) {}
  if (!r.ok) throw new Error(v?.error || (r.status === 404 ? 'No jobs service is configured.' : `${r.status} ${r.statusText}`));
  return v;
}
// Every write goes through the console's write gate (services/console/src/main.rs,
// WriteGate): the request's Host must be one of CONSOLE_HOSTS and the
// login (the header CONSOLE_LOGIN_HEADER names, set by the authenticating
// proxy in front of the console) one of CONSOLE_WRITE_LOGINS, or the write is
// refused (403). The page sends no login itself.
// A live account's order, cancel, flatten or resume also carries the
// console's order token (CONSOLE_ORDER_TOKEN) as X-Order-Token, and so does a
// paper account's flatten or resume unless CONSOLE_TOKEN_FOR_PAPER_FLATTEN is
// off; the server refuses those without it (401). The gate marks its own
// refusals (X-Write-Gate: refused, and "gate": true in the body), so they can
// be told from a 401 or 403 that the jobs service, the queue or the store
// sends back through the console. The token is asked for at the first write
// that needs it in a tab and kept for that tab only (sessionStorage, or memory
// when storage is unavailable). Only one prompt is open at a time: writes sent
// together share its answer. A token the gate refuses is forgotten, so the
// next such write asks again. A write sent without the token that the gate
// refuses for want of it (the server found it needs one, e.g. the account is
// live) asks and is sent once more; the gate refuses before anything reaches
// a gateway, so the first try sent nothing. Storage can throw (private
// windows): the page still works. When the server says live writes need no
// token (CONSOLE_LIVE_NEEDS_TOKEN=false: /api/write-gate's live_token is
// false), nothing is asked up front; only a gate-marked 401 would ask.
const TOKEN_KEY = 'tinytrader-order-token';
let orderTok = null;
function setToken(t) {
  orderTok = t;
  try { if (t) sessionStorage.setItem(TOKEN_KEY, t); else sessionStorage.removeItem(TOKEN_KEY); } catch (_) {}
}
function getToken() {
  try { return sessionStorage.getItem(TOKEN_KEY) || orderTok; } catch (_) { return orderTok; }
}
/** @type {Promise<string | null> | null} The prompt in flight, shared by every write waiting for the token. */
let asking = null;
/** Ask for the token once, however many writes are waiting for it. @returns {Promise<string | null>} */
function askToken() {
  if (!asking) asking = new Promise(resolve => setTimeout(() => {
    const t = (window.prompt('The console\'s order token, for a live account or a flatten (kept for this tab only):') || '').trim() || null;
    setToken(t);
    asking = null;
    resolve(t);
  }, 0));
  return asking;
}
/** The token to send: the one kept for this tab, or the answer to the (shared) prompt. */
const liveToken = async () => asking ? asking : getToken() || askToken();
/** Forget a refused token, unless another write has already replaced it. @param {string | null} sent */
function forget(sent) { if (sent && getToken() === sent) setToken(null); }
/** Whether a live write needs the order token, as /api/write-gate last said: null until it has been read (or when it can't be), which counts as yes. @type {boolean | null} */
let tokenNeeded = null;
/** Whether the console's write gate itself refused this request (nothing was sent on). @param {Response} r */
const gateRefused = r => r.headers.get('x-write-gate') === 'refused';
/** A write request (`/api/orders/...`, flatten, resume, jobs, the queue, settings, the journal, ...): the response, as fetch gives it. `null` is sent as `null`; `extra` adds headers. `live` marks a write that needs the order token (a live account's), which sends it.
 * @param {string} path @param {any} body @param {string} [method] @param {Record<string, string>} [extra] @param {boolean} [live] @returns {Promise<Response>} */
export async function orderFetch(path, body, method = 'POST', extra = {}, live = false) {
  const go = t => fetch(path, { method, cache: 'no-store', headers: { 'content-type': 'application/json', ...extra, ...(t ? { 'x-order-token': t } : {}) }, body: JSON.stringify(body === undefined ? {} : body) });
  if (live && tokenNeeded === null) await writeGate();
  const t = !live ? null : tokenNeeded === false ? getToken() : await liveToken();
  let r = await go(t);
  // Only the gate's own 401 is about the token; a service's 401 is passed on as it is.
  if (r.status === 401 && gateRefused(r)) {
    forget(t);
    // Sent without the token, but the gate wants it: ask (or use one another write just got) and send once more.
    if (!t) {
      const again = await liveToken();
      if (again) { r = await go(again); if (r.status === 401 && gateRefused(r)) forget(again); }
    }
  }
  return r;
}
/** Whether the console's write gate refused a write (nothing was sent on). A 401 or 403 from a service behind the console is not this. @param {Response} r */
export const refused = r => (r.status === 401 || r.status === 403) && gateRefused(r);
/** A refused write in plain words, from the server's error: the gate's, or the service's when it was the service that refused. @param {Response} r @param {any} v the response's JSON */
export function refusedText(r, v) {
  if (!refused(r)) return v?.error || `${r.status} ${r.statusText}`;
  const why = v?.error || (r.status === 401 ? 'the order token was refused' : 'this login may not make changes');
  return `refused by the console: ${why}${r.status === 401 ? '; the token is asked for again on the next try' : ''}`;
}
/** Whether an account (or a gateway's routing entry) trades live: anything but `environment: 'paper'`, unknown included, as the server decides. @param {any} a */
export const isLive = a => a?.environment !== 'paper';
/** The console's write gate as the server reports it for this viewer: `{ writes, hosts, allowed, live_orders, live_token, read_only }`, or null when it can't be read. */
export async function writeGate() {
  let g = null;
  try { const r = await fetch('/api/write-gate', { cache: 'no-store' }); g = r.ok ? await r.json() : null; } catch (_) {}
  tokenNeeded = g ? g.live_token !== false : null;
  return g;
}
// The job queue (decision 0012), through the console's /api/queue proxy.
export async function queueApi(path, body, method) {
  const r = body === undefined && !method ? await fetch('/api/queue/' + path, { cache: 'no-store' })
    : await orderFetch('/api/queue/' + path, body, method || 'POST');
  let v = null; try { v = await r.json(); } catch (_) {}
  if (!r.ok) throw new Error(v?.error || (r.status === 404 ? 'No job queue is configured.' : `${r.status} ${r.statusText}`));
  return v;
}

// The deployment as the console last saw it: the topology (what runs) and
// the latest snapshot of every service (health, metrics, streams). The page
// fills these in; widgets read them (decision 0010).
import { fmt } from './format.js';
import { publish } from './channels.js';

/** @type {{ topo: Topology | null, snap: Snapshot | null }} */
export const fleet = { topo: null, snap: null };

let topoText = '';
/** Sets the topology, and publishes it when its text changed: it's read every few seconds, and mostly is the same.
 * @param {Topology} t */
export function setTopology(t) {
  const text = JSON.stringify(t);
  if (text === topoText) return;
  topoText = text; fleet.topo = t; publish('topology', t);
}

/** @type {(id: string) => ServiceState} */
export const svcState = id => fleet.snap?.services?.[id] || {};

export function metric(id, name, labels) {
  const list = svcState(id).metrics?.[name] || [];
  const hit = list.find(m => !labels || Object.entries(labels).every(([k, v]) => m.labels[k] === v));
  return hit ? hit.value : null;
}

export function statusOf(node) {
  if (node.kind === 'ext' || node.kind === 'acct') return null;
  // A relay's client is drawn only while it is connected.
  if (node.kind === 'client') return 'ok';
  if (node.kind === 'store') {
    const writer = writerOf(node.id);
    return writer ? statusOf(writer) : 'idle';
  }
  const s = svcState(node.id), c = s.container, h = s.health;
  // The same rule as libs/fleet's ServiceState::status: a restart loop or a
  // service reporting failed is down.
  if (c && ['exited', 'dead', 'restarting'].includes(c.state)) return 'crit';
  if (c && c.state && c.state !== 'running') return 'warn';
  if (h) {
    if (h.error) return c ? 'warn' : 'crit';
    if (h.body?.state === 'idle') return 'off';
    if (h.body?.state === 'starting') return 'start';
    if (h.body?.state === 'failed') return 'crit';
    if (h.body?.state && h.body.state !== 'ready') return 'warn';
    return 'ok';
  }
  return c ? 'ok' : 'idle';
}

export const statusText = { ok: ['Healthy', 'p-ok'], warn: ['Needs attention', 'p-warn'], crit: ['Down', 'p-crit'], idle: ['No data', 'p-idle'], off: ['Off schedule', 'p-idle'], start: ['Starting', 'p-idle'] };

export function writerOf(storeId) {
  const e = (fleet.topo?.edges || []).find(e => e.to === storeId && e.kind === 'flow');
  return e && nodesById()[e.from];
}

/** Every part of the deployment graph, by ID.
 * @returns {Record<string, GraphNode>} */
export function nodesById() {
  const t = /** @type {Partial<Topology>} */ (fleet.topo || {});
  /** @type {GraphNode[]} */
  const all = [
    ...(t.external || []).map(n => ({ ...n, kind: /** @type {const} */ ('ext') })),
    ...(t.services || []).map(n => ({ ...n, kind: /** @type {const} */ ('svc') })),
    ...(t.stores || []).map(n => ({ ...n, kind: /** @type {const} */ ('store') }))
  ];
  return Object.fromEntries(all.map(n => [n.id, n]));
}

export function uptimeOf(id) {
  const s = svcState(id);
  if (s.health?.body?.uptime_seconds != null) return s.health.body.uptime_seconds;
  const started = s.container?.started_at && Date.parse(s.container.started_at);
  return started ? (Date.now() - started) / 1000 : null;
}

export function subOf(node) {
  if (node.kind === 'ext') return node.sub || 'external';
  if (node.kind === 'client' || node.kind === 'acct') return node.sub || '';
  if (node.kind === 'store') {
    const v = node.metric && metric(node.metric.service, node.metric.name);
    return v != null ? fmt(v, node.metric) : (node.sub || 'store');
  }
  const s = svcState(node.id);
  if (s.health?.body?.detail) return s.health.body.detail;
  if (s.health?.error) return 'unreachable';
  if (s.container?.status) return s.container.status.toLowerCase();
  return node.sub || 'no data';
}

export function newsItems() {
  const seen = new Map();
  for (const st of Object.values(fleet.snap?.services || {})) {
    for (const n of st.news?.items || []) if (!seen.has(n.id)) seen.set(n.id, n);
  }
  return [...seen.values()].sort((a, b) => (b.created_at || '').localeCompare(a.created_at || ''));
}

// ---------- store: data and storage ----------
// Catalogs come from services that describe a `catalog` stream. Every root of
// every catalog is shown; nothing here knows about particular datasets.
/** Every root of every catalog, with the service and thresholds it came from.
 * @returns {CatalogRoot[]} */
export function catalogRoots() {
  const out = [];
  for (const [svc, st] of Object.entries(fleet.snap?.services || {})) {
    for (const r of st.catalog?.roots || []) out.push({ ...r, service: svc, thresholds: st.catalog.thresholds });
  }
  return out;
}
export const catalogStamp = () => Object.values(fleet.snap?.services || {}).map(s => s.catalog?.generated_at ?? '').join('|');

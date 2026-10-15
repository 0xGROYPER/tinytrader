// System graph: what runs where, and how data flows. Hovering a part shows
// its details in a floating card; clicking pins the card, which then adds a
// live event tail and every metric. Only what's running now is drawn.
import { defineWidget, paint } from '../lib/widgets.js';
import { fleet, svcState, metric, statusOf, statusText, writerOf, nodesById, uptimeOf, subOf } from '../lib/fleet.js';
import { esc, bytes, duration, fmt, labelText, clip, etTime } from '../lib/format.js';

// ---------- topology layout ----------
const NW = 176, NH = 50, CG = 96, RG = 26, PAD = 24, HOST_TOP = 40;
// The graph shows what is running now: services that are deliberately idle
// (outside their active hours) are left out, with their edges, and so is any
// outside source nothing active is connected to. The Services table still
// lists them.
function activeGraph() {
  const all = nodesById(), edges = fleet.topo?.edges || [];
  const off = new Set(Object.values(all).filter(n => n.kind === 'svc' && statusOf(n) === 'off').map(n => n.id));
  const kept = edges.filter(e => !off.has(e.from) && !off.has(e.to));
  const used = new Set(kept.flatMap(e => [e.from, e.to]));
  const byId = Object.fromEntries(Object.entries(all).filter(([id, n]) => !off.has(id) && (n.kind !== 'ext' || used.has(id))));
  addClients(byId, kept);
  return { byId, edges: kept };
}

// The relays' clients, in a box of their own: each named client once, fed
// by every relay it reads; unnamed ones counted per relay; and each Alpaca
// account a client says it trades, beside it.
function addClients(byId, edges) {
  const accounts = Object.values(fleet.snap?.services || {}).flatMap(s => s.accounts?.accounts || []);
  const link = (from, to, label) => { if (!edges.some(e => e.from === from && e.to === to)) edges.push({ from, to, kind: 'flow', ...(label ? { label } : {}) }); };
  for (const relay of Object.values(byId).filter(n => n.kind === 'svc')) {
    const list = svcState(relay.id).clients?.clients;
    if (!Array.isArray(list) || !list.length) continue;
    for (const c of list.filter(c => c.client)) {
      const id = 'client:' + c.client;
      const node = byId[id] ||= { id, kind: 'client', label: c.client, host: 'clients', conns: [] };
      node.conns.push({ ...c, relay: relay.label || relay.id });
      link(relay.id, id);
      if (c.account) {
        const a = accounts.find(x => x.name === c.account || x.label?.toLowerCase() === c.account.toLowerCase());
        const aid = 'account:' + c.account;
        byId[aid] ||= { id: aid, kind: 'acct', label: a ? `Alpaca ${a.label}` : c.account, host: 'clients', account: a || null, sub: a ? `${a.environment} account` : 'not an account this deployment reads' };
        link(id, aid, 'trades');
      }
    }
    const unnamed = list.filter(c => !c.client);
    if (unnamed.length) {
      const id = 'client:unnamed@' + relay.id;
      byId[id] = { id, kind: 'client', label: `${unnamed.length} unnamed`, host: 'clients', conns: unnamed.map(c => ({ ...c, relay: relay.label || relay.id })), unnamed: true };
      link(relay.id, id);
    }
  }
  for (const n of Object.values(byId)) {
    if (n.kind === 'client') n.sub = n.unnamed ? `no name given · ${n.conns[0].relay}` : `${n.conns.length} relay${n.conns.length === 1 ? '' : 's'}`;
  }
}
function layout() {
  const { byId, edges } = activeGraph();
  const ids = Object.keys(byId);
  // Column = longest chain of edges leading to a node. Self-edges and edges
  // that would close a cycle are ignored, so no topology can stretch the graph.
  const col = {}, onPath = new Set();
  const colOf = id => {
    if (col[id] !== undefined) return col[id];
    if (byId[id].kind === 'ext') return (col[id] = 0);
    onPath.add(id);
    let c = 1;
    for (const e of edges) {
      if (e.to !== id || e.from === id || !byId[e.from] || onPath.has(e.from)) continue;
      c = Math.max(c, byId[e.from].kind === 'ext' ? 1 : colOf(e.from) + 1);
    }
    onPath.delete(id);
    return (col[id] = c);
  };
  ids.forEach(colOf);
  const hosts = fleet.topo?.hosts || [{ id: 'local', label: 'local' }];
  const outside = id => byId[id].host === 'clients';
  const hostOf = n => n.kind === 'ext' || n.host === 'clients' ? null : (n.host || hosts[0].id);
  const maxCol = Math.max(1, ...ids.filter(id => !outside(id)).map(id => col[id]));
  const x = c => PAD + c * (NW + CG) + (c > 0 ? 16 : 0);
  const pos = {}, boxes = [];
  const ext = ids.filter(id => byId[id].kind === 'ext');
  ext.forEach((id, r) => { pos[id] = { x: x(0), y: PAD + HOST_TOP + r * (NH + RG), w: NW - 30, h: NH }; });
  let y = PAD;
  for (const h of hosts) {
    const members = ids.filter(id => hostOf(byId[id]) === h.id);
    if (!members.length) continue;
    const rowsByCol = {};
    for (let c = 1; c <= maxCol; c++) {
      const inCol = members.filter(id => col[id] === c);
      inCol.sort((a, b) => avgRow(a) - avgRow(b));
      inCol.forEach((id, r) => { pos[id] = { x: x(c), y: y + HOST_TOP + r * (NH + RG), w: NW, h: NH }; });
      rowsByCol[c] = inCol.length;
    }
    const rows = Math.max(1, ...Object.values(rowsByCol));
    const height = HOST_TOP + rows * (NH + RG) - RG + 18;
    boxes.push({ x: x(1) - 18, y, w: maxCol * (NW + CG) - CG + 36, h: height, label: h.label || h.id, sub: h.sub || '' });
    y += height + 28;
  }
  // Clients of the relays, and the accounts they trade, in a box to the
  // right: each client level with the relays feeding it.
  let right = x(maxCol) + NW;
  const cl = ids.filter(id => byId[id].kind === 'client'), ac = ids.filter(id => byId[id].kind === 'acct');
  if (cl.length) {
    const cx = right + CG, place = (list, px) => {
      let next = PAD + HOST_TOP;
      list.sort((a, b) => avgRow(a) - avgRow(b)).forEach(id => {
        const want = Math.max(next, Math.min(avgRow(id), 1e8));
        pos[id] = { x: px, y: want, w: NW, h: NH }; next = want + NH + RG;
      });
      return next;
    };
    const bottom = Math.max(place(cl, cx + 18), ac.length ? place(ac, cx + 18 + NW + CG / 2) : 0);
    const w = (ac.length ? 2 * NW + CG / 2 : NW) + 36;
    const top = Math.min(...[...cl, ...ac].map(id => pos[id].y)) - HOST_TOP;
    boxes.push({ x: cx, y: top, w, h: bottom - top - RG + 18, label: 'clients', sub: 'of the relays' });
    right = cx + w;
    y = Math.max(y, bottom + 28);
  }
  const width = right + PAD + 2;
  return { byId, edges, pos, boxes, width, height: Math.max(y - 28 + PAD, PAD * 2 + HOST_TOP + ext.length * (NH + RG)) };

  function avgRow(id) {
    const preds = edges.filter(e => e.to === id && pos[e.from]).map(e => pos[e.from].y);
    return preds.length ? preds.reduce((a, b) => a + b, 0) / preds.length : 1e9;
  }
}

function topology(g) {
  if (!fleet.topo) return '<section><div class="empty">Loading topology…</div></section>';
  const L = layout();
  const anchor = (p, side) => side === 'r' ? [p.x + p.w, p.y + p.h / 2] : [p.x, p.y + p.h / 2];
  const edges = L.edges.filter(e => L.pos[e.from] && L.pos[e.to]).map(e => {
    const a = anchor(L.pos[e.from], 'r'), b = anchor(L.pos[e.to], 'l');
    const k = Math.max(40, (b[0] - a[0]) / 2);
    const d = `M${a}C${a[0] + k},${a[1]} ${b[0] - k},${b[1]} ${b}`;
    const mid = [(a[0] + b[0]) / 2, (a[1] + b[1]) / 2];
    const from = L.byId[e.from], to = L.byId[e.to];
    const live = e.kind === 'flow' && statusOf(from.kind === 'ext' ? to : from) === 'ok';
    return `<path class="e e-${esc(e.kind)}${live ? ' on' : ''}" d="${d}" marker-end="url(#arr)"/>` + (e.label ? `<text class="e-label" x="${mid[0]}" y="${mid[1] - 7}" text-anchor="middle">${esc(e.label)}</text>` : '');
  }).join('');
  const boxes = L.boxes.map(b => `<rect class="host-box" x="${b.x}" y="${b.y}" width="${b.w}" height="${b.h}" rx="10"/><text class="host-name" x="${b.x + 14}" y="${b.y + 24}">${esc(b.label)}</text><text class="host-sub" x="${b.x + 14 + b.label.length * 7.4 + 10}" y="${b.y + 24}">${esc(b.sub)}</text>`).join('');
  const nodes = Object.values(L.byId).filter(n => L.pos[n.id]).map(n => {
    const p = L.pos[n.id], st = statusOf(n);
    return `<g class="node k-${n.kind}${st ? ' s-' + st : ''}${g.node === n.id ? ' sel' : ''}" data-node="${esc(n.id)}" tabindex="0" role="button" aria-label="${esc(n.label || n.id)}">
      <rect x="${p.x}" y="${p.y}" width="${p.w}" height="${p.h}" rx="6"/>
      ${st ? `<circle class="sdot" cx="${p.x + p.w - 12}" cy="${p.y + 13}" r="4"/>` : ''}
      <text class="n-label" x="${p.x + 12}" y="${p.y + 20}">${esc(n.label || n.id)}</text>
      <text class="n-sub" x="${p.x + 12}" y="${p.y + 37}">${esc(clip(subOf(n), 24))}</text></g>`;
  }).join('');
  return `<section>
    <div class="sec-head"><h2>Deployed system</h2><span class="legend"><span><i class="lg"></i>data</span><span><i class="lg lg-read"></i>observation</span></span></div>
    <div class="topo-wrap"><svg class="topo" viewBox="0 0 ${L.width} ${L.height}" style="min-width:${Math.min(L.width, 760)}px" role="group" aria-label="Deployed system">
      <defs><marker id="arr" viewBox="0 0 10 10" refX="9" refY="5" markerWidth="7" markerHeight="7" orient="auto-start-reverse"><path d="M0,1 L9,5 L0,9 Z" class="arrhead"/></marker></defs>
      ${boxes}${edges}${nodes}
    </svg></div>
  </section>`;
}

function detail(node, g, preview) {
  const st = statusOf(node), s = svcState(node.id), c = s.container, h = s.health?.body;
  const facts = [];
  if (node.kind === 'svc') {
    facts.push(['Container', c ? `${c.state} · ${c.status}` : 'not reported']);
    if (c?.image) facts.push(['Image', c.image]);
    facts.push(['Uptime', duration(uptimeOf(node.id))]);
    if (c) facts.push(['Restarts', c.restart_count ?? '—']);
    if (c?.cpu_percent != null) facts.push(['CPU', c.cpu_percent.toFixed(1) + '%']);
    if (c?.memory?.usage_bytes != null) facts.push(['Memory', bytes(c.memory.usage_bytes)]);
    if (h) facts.push(['Health', `${h.state}${h.detail ? ' · ' + h.detail : ''}`], ['Build', h.build || '—']);
    if (s.health?.error) facts.push(['Health', 'unreachable: ' + s.health.error]);
    if (h?.last_error) facts.push(['Last error' + (h.last_error_at ? ` (${duration(Date.now() / 1000 - h.last_error_at)} ago)` : ''), h.last_error]);
  } else if (node.kind === 'client') {
    for (const c of node.conns) facts.push([c.relay, `${c.replay ? 'replaying, then live' : 'live'}${c.conditions ? ', with trade conditions' : ''} · for ${duration(Date.now() / 1000 - c.since)} · from ${c.peer}`]);
    const accts = [...new Set(node.conns.map(c => c.account).filter(Boolean))];
    if (accts.length) facts.push(['Trades', accts.join(', ')]);
    if (node.unnamed) facts.push(['Name', 'none given: a client names itself with "client" in its join request']);
  } else if (node.kind === 'acct') {
    const a = node.account?.account;
    if (a) facts.push(['Equity', `$${Number(a.equity).toLocaleString(undefined, { minimumFractionDigits: 2, maximumFractionDigits: 2 })}`], ['Buying power', `$${Number(a.buying_power).toLocaleString(undefined, { minimumFractionDigits: 2, maximumFractionDigits: 2 })}`], ['Positions', String(node.account.positions?.length ?? 0)]);
    else facts.push(['Account', node.sub]);
  } else if (node.kind === 'store') {
    const w = writerOf(node.id);
    facts.push(['Written by', w ? w.label || w.id : '—']);
    if (node.metric) facts.push([node.metric.label || node.metric.name, fmt(metric(node.metric.service, node.metric.name), { ...node.metric, prefix: '', suffix: '' })]);
  }
  const metrics = s.metrics ? Object.entries(s.metrics).flatMap(([name, list]) => list.map(m => /** @type {[string, number]} */ ([name + labelText(m.labels), m.value]))) : [];
  return `<div class="detail">
    <div class="d-head"><b>${esc(node.label || node.id)}</b>${st ? `<span class="pill ${statusText[st][1]}">${statusText[st][0]}</span>` : ''}<span class="muted" style="margin-left:auto">${esc(node.kind === 'ext' ? 'external' : node.kind === 'client' ? 'client' : node.kind === 'acct' ? 'account' : (node.host || 'local'))}</span>${preview ? '' : '<button type="button" class="tile-btn" data-pop-close title="Close (Esc)" aria-label="Close">✕</button>'}</div>
    ${preview && node.kind === 'svc' && node.events ? '<div class="muted" style="font-size:12px">Click for live events and every metric.</div>' : ''}
    ${node.role ? `<div class="muted">${esc(node.role)}</div>` : ''}
    ${facts.length ? `<div class="facts">${facts.map(([k, v]) => `<div><div class="k">${esc(k)}</div><div class="v">${esc(v)}</div></div>`).join('')}</div>` : ''}
    ${node.kind === 'svc' && node.events && !preview ? eventsPanel(node, g) : ''}
    ${metrics.length && !preview ? `<details${g.metricsOpen ? ' open' : ''} data-metrics><summary>All metrics (${metrics.length})</summary><div class="scroll" style="margin-top:8px"><table class="metrics-tbl"><tbody>${metrics.map(([k, v]) => `<tr><td class="mono">${esc(k)}</td><td class="mono">${esc(v == null ? 'NaN' : +v.toFixed(6))}</td></tr>`).join('')}</tbody></table></div></details>` : ''}
  </div>`;
}
// ---------- live events ----------
// A service that declares an `events` URL in the topology gets a live tail here.
function eventsPanel(node, g) {
  if (g.ev.service !== node.id) Object.assign(g.ev, { service: node.id, after: 0, rows: [], open: new Set(), times: [], fresh: new Set() });
  return `<div class="events">
    <div class="ev-head"><b>Live events</b><span class="muted mono" data-ev-rate style="font-size:13px">${evRate(g)}</span>
      <button type="button" class="btn-sm" data-ev-pause aria-pressed="${g.ev.paused}">${g.ev.paused ? 'Resume' : 'Pause'}</button></div>
    <div class="ev-list" data-ev-list aria-live="off">${evRows(g)}</div>
  </div>`;
}
function evRows(g) {
  if (!g.ev.rows.length) return '<div class="empty">Waiting for events…</div>';
  return g.ev.rows.map(e => `<details class="ev-row${g.ev.fresh.has(e.seq) ? ' new' : ''}" data-seq="${e.seq}"${g.ev.open.has(e.seq) ? ' open' : ''}>
    <summary><span class="ev-t">${etTime(e.received_ms, true)}.${String(e.received_ms % 1000).padStart(3, '0')}</span><span class="ev-s">${esc(e.summary)}</span></summary>
    <pre>${esc(prettyRaw(e.raw))}</pre></details>`).join('');
}
function prettyRaw(raw) { try { return JSON.stringify(JSON.parse(raw), null, 2); } catch (_) { return raw; } }
function evRate(g) {
  const now = Date.now(); g.ev.times = g.ev.times.filter(t => now - t[0] < 10000);
  const n = g.ev.times.reduce((a, t) => a + t[1], 0);
  return g.ev.rows.length ? `${(n / 10).toFixed(1)} msg/s` : '';
}

// Live instances, so another part of the console can open a node's card.
const live = new Set();
export function showNode(id) { for (const g of live) g.pin(id); }

async function pollEvents(g) {
  const node = nodesById()[g.node];
  if (!node || node.kind !== 'svc' || !node.events || g.ev.paused || g.ev.service !== node.id || !g.pop) return;
  try {
    const r = await fetch(`/api/events/${encodeURIComponent(node.id)}?after=${g.ev.after}&limit=100`, { cache: 'no-store' });
    if (!r.ok) return;
    const body = await r.json();
    if (g.ev.service !== node.id) return;
    const fresh = body.events || [];
    if (body.next < g.ev.after) g.ev.rows = []; // service restarted
    g.ev.after = body.next ?? g.ev.after;
    g.ev.fresh = new Set(fresh.map(e => e.seq));
    if (fresh.length) { g.ev.times.push([Date.now(), fresh.length]); g.ev.rows = [...fresh.reverse(), ...g.ev.rows].slice(0, 200); }
    const list = g.pop?.querySelector('[data-ev-list]');
    if (list) { const top = list.scrollTop; list.innerHTML = evRows(g); list.scrollTop = top; }
    const rate = g.pop?.querySelector('[data-ev-rate]'); if (rate) rate.textContent = evRate(g);
  } catch (_) {}
}

defineWidget('overview', {
  title: 'System graph',
  mount(el) {
    const g = { node: null, hover: null, metricsOpen: false, pop: null, hideT: 0,
      ev: { service: null, after: 0, rows: [], paused: false, open: new Set(), times: [], fresh: new Set() } };
    const nodeEl = id => el.querySelector(`[data-node="${CSS.escape(id)}"]`);
    // The card floats beside the node, inside the window.
    function place(id) {
      const n = nodeEl(id); if (!n || !g.pop) return;
      const r = n.getBoundingClientRect(), w = g.pop.offsetWidth, h = g.pop.offsetHeight;
      const left = r.right + 10 + w <= innerWidth - 8 ? r.right + 10 : Math.max(8, r.left - 10 - w);
      g.pop.style.left = left + 'px';
      g.pop.style.top = Math.max(8, Math.min(r.top, innerHeight - h - 8)) + 'px';
    }
    function card() {
      const id = g.node || g.hover, node = id && activeGraph().byId[id];
      if (!node) return close();
      if (!g.pop) { g.pop = document.createElement('div'); g.pop.className = 'node-pop'; document.body.appendChild(g.pop); wirePop(g.pop); }
      g.pop.classList.toggle('pinned', !!g.node);
      const list = g.pop.querySelector('[data-ev-list]'), top = list?.scrollTop ?? 0;
      g.pop.innerHTML = detail(node, g, !g.node);
      const l2 = g.pop.querySelector('[data-ev-list]'); if (l2) l2.scrollTop = top;
      place(id);
    }
    function close() { g.pop?.remove(); g.pop = null; g.node = null; g.hover = null; draw(); }
    function wirePop(pop) {
      pop.addEventListener('click', e => {
        if (e.target.closest('[data-pop-close]')) { close(); return; }
        const p = e.target.closest('[data-ev-pause]');
        if (p) { g.ev.paused = !g.ev.paused; p.textContent = g.ev.paused ? 'Resume' : 'Pause'; p.setAttribute('aria-pressed', g.ev.paused); }
      });
      pop.addEventListener('toggle', e => {
        if (e.target.matches('[data-metrics]')) { g.metricsOpen = e.target.open; return; }
        const row = e.target.closest('.ev-row'); if (!row) return;
        const seq = +row.dataset.seq; if (row.open) g.ev.open.add(seq); else g.ev.open.delete(seq);
      }, true);
      pop.addEventListener('pointerleave', () => { if (!g.node) hideSoon(); });
      pop.addEventListener('pointerenter', () => clearTimeout(g.hideT));
    }
    function hideSoon() { clearTimeout(g.hideT); g.hideT = setTimeout(() => { if (!g.node) { g.hover = null; g.pop?.remove(); g.pop = null; } }, 150); }
    function draw() {
      paint(el, topology(g));
    }
    g.pin = id => { if (!activeGraph().byId[id]) return; g.node = id; g.ev.fresh = new Set(); draw(); card(); };
    const over = e => {
      const n = e.target.closest('[data-node]'); if (!n) return;
      clearTimeout(g.hideT);
      if (g.node) return;
      if (g.hover !== n.dataset.node) { g.hover = n.dataset.node; card(); }
    };
    const out = e => { const n = e.target.closest('[data-node]'); if (n && !n.contains(e.relatedTarget) && !g.node) hideSoon(); };
    const click = e => {
      const n = e.target.closest('[data-node]');
      if (!n) { if (g.node) close(); return; }
      if (g.node === n.dataset.node) close(); else g.pin(n.dataset.node);
    };
    const key = e => {
      const n = e.target.closest('[data-node]');
      if (n && (e.key === 'Enter' || e.key === ' ')) { e.preventDefault(); n.dispatchEvent(new MouseEvent('click', { bubbles: true })); }
    };
    const esc_ = e => { if (e.key === 'Escape' && g.pop) close(); };
    const reposition = () => { if (g.pop) place(g.node || g.hover); };
    el.addEventListener('pointerover', over); el.addEventListener('pointerout', out);
    el.addEventListener('click', click); el.addEventListener('keydown', key);
    document.addEventListener('keydown', esc_, true);
    el.addEventListener('scroll', reposition); window.addEventListener('resize', reposition);
    const timer = setInterval(() => pollEvents(g), 700);
    live.add(g);
    draw();
    return {
      update() { g.ev.fresh = new Set(); draw(); if (g.pop) card(); },
      dispose() {
        clearInterval(timer); live.delete(g); g.pop?.remove();
        document.removeEventListener('keydown', esc_, true); window.removeEventListener('resize', reposition);
      },
    };
  },
});

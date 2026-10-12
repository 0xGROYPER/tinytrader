// Schedule: one line per bundle (decision 0011: services sharing hours, as
// the deployment defines them), which opens to its members, then one line per
// other service (relays on the same feed and hours share one). Each has its
// state, a strip of today in New York time and what else it runs (every 30s,
// on events, yields to capture). Details are in the tooltips. The strip: a
// bar for active hours (bright while running), hatched for a pause, a thin
// line for always running.
import { defineWidget, paint } from '../lib/widgets.js';
import { fleet, svcState, statusOf } from '../lib/fleet.js';
import { esc } from '../lib/format.js';

const mins = t => { const [h, m] = t.split(':').map(Number); return h * 60 + m; };
const pct = m => (m / 1440 * 100).toFixed(2) + '%';
const every = s => s >= 3600 ? `${s / 3600}h` : s >= 60 ? `${s / 60}m` : `${s}s`;
// A line's state is its worst member's.
const rank = { crit: 0, warn: 1, start: 2, ok: 3, off: 4, idle: 5 };

function draw(my) {
  const svcs = (fleet.topo?.services || []).filter(s => Array.isArray(s.schedule));
  if (!svcs.length) return '<p class="muted">No service describes a schedule yet.</p>';
  const bundles = (fleet.topo?.bundles || []).map(b => ({ ...b, svcs: svcs.filter(s => (b.members || []).includes(s.id)) })).filter(b => b.svcs.length);
  const inBundle = id => bundles.some(b => b.svcs.some(s => s.id === id));
  // Lines for a set of services, relays on the same feed and hours as one.
  const linesOf = list => {
  const lines = [];
  for (const s of list) {
    const feed = s.schedule.find(x => x.feed);
    const key = feed ? `${feed.feed}|${feed.hours}` : s.id;
    let l = lines.find(x => x.key === key);
    if (!l) lines.push(l = { key, label: feed ? feed.feed : (s.label || s.id), members: [], symbols: [], items: s.schedule });
    l.members.push(s); if (feed) l.symbols.push(...(feed.symbols || []));
  }
  return lines;
  };
  const ny = new Date().toLocaleString('en-GB', { timeZone: 'America/New_York', hour: '2-digit', minute: '2-digit', hour12: false, weekday: 'short' });
  const [wd, hm] = ny.split(' '), now = mins(hm);
  const cal = svcs.map(s => svcState(s.id).calendar).find(c => c?.days?.length);
  const session = cal?.days.find(d => d.date === new Date().toLocaleDateString('en-CA', { timeZone: 'America/New_York' }));
  const trading = cal ? !!session : !['Sat', 'Sun'].includes(wd);
  // Hours are set against the 09:30–16:00 session; an early close ends them early by as much.
  const shift = session && session.close < '16:00' ? mins(session.close) - 960 : 0;
  const close = session ? mins(session.close) : 960;

  const row = (l, indent) => {
    const st = l.members.map(s => statusOf({ ...s, kind: 'svc' })).sort((a, b) => (rank[a] ?? 9) - (rank[b] ?? 9))[0];
    const detail = svcState(l.members[0].id).health?.body?.detail || '';
    const bars = [], marks = [];
    let next = '';
    for (const x of l.items) {
      if ((x.kind === 'window' || x.kind === 'pause') && x.hours) {
        const [a, b0] = x.hours.split('-').map(mins), b = b0 <= 17 * 60 ? b0 + shift : b0;
        const inside = trading && now >= a && now < b;
        const cls = x.kind === 'pause' ? 'pause' : st === 'ok' ? 'live' : '';
        bars.push(`<i class="sc-bar ${cls}${trading ? '' : ' off'}" style="left:${pct(a)};width:${pct(b - a)}" title="${esc(x.name)}: ${esc(x.when)}"></i>`);
        const resume = /until \w+ (\d\d:\d\d)/.exec(detail)?.[1];
        if (x.kind === 'window') next = inside && st === 'ok' ? `until ${x.hours.split('-')[1]}` : resume ? `next ${resume}` : '';
        if (x.kind === 'pause' && inside) next = 'paused';
      } else if (x.kind === 'always') bars.push(`<i class="sc-always" title="${esc(x.name)}: ${esc(x.when || 'always')}"></i>`);
      else if (x.kind === 'yield') {
        // Where it expects to yield: the capture hours, hatched.
        if (x.hours) {
          const [a, b0] = x.hours.split('-').map(mins), b = b0 <= 17 * 60 ? b0 + shift : b0;
          bars.push(`<i class="sc-bar pause${trading ? '' : ' off'}" style="left:${pct(a)};width:${pct(b - a)}" title="${esc(x.when)}"></i>`);
        }
        marks.push(`<span title="${esc(x.when)}">yields to capture</span>`);
        if (/^yielding/.test(detail)) next = 'yielding';
      } else if (x.kind === 'interval') marks.push(`<span title="${esc(x.name)}: ${esc(x.when)}">${x.every_seconds ? 'every ' + every(x.every_seconds) : 'repeats'}</span>`);
      else marks.push(`<span title="${esc(x.name)}: ${esc(x.when)}">on events</span>`);
    }
    const title = [l.label, l.symbols.join(', '), l.members.length > 1 ? `${l.members.length} relays` : '', detail].filter(Boolean).join(' · ');
    return `<div class="sc-row${indent ? ' sc-member' : ''}"><i class="sc-dot ${st || 'idle'}"></i><span class="sc-name" title="${esc(title)}">${esc(l.label)}${l.members.length > 1 ? `<em>${l.members.length} relays</em>` : ''}</span>
      <span class="sc-track">${bars.join('')}<i class="sc-now" style="left:${pct(now)}"></i></span>
      <span class="sc-marks">${marks.join('')}</span><span class="sc-next">${esc(next)}</span></div>`;
  };
  const order = l => l.items.some(x => x.kind === 'window') ? 0 : l.items.some(x => x.kind === 'pause' || x.kind === 'yield') ? 1 : 2;
  const sorted = ls => ls.sort((a, b) => order(a) - order(b) || a.label.localeCompare(b.label));
  // A bundle's line: live if any member is, its members' hours on the strip,
  // and the earliest next start among them.
  const bundleRow = b => {
    const sts = b.svcs.map(s => statusOf({ ...s, kind: 'svc' }));
    const st = sts.find(x => x === 'crit') || sts.find(x => x === 'warn') || (sts.includes('ok') ? 'ok' : sts.includes('start') ? 'start' : 'off');
    const hours = [...new Set(b.svcs.flatMap(s => s.schedule.filter(x => x.kind === 'window' && x.hours).map(x => x.hours)))];
    const bars = hours.map(h => {
      const [a, b0] = h.split('-').map(mins), e = b0 <= 17 * 60 ? b0 + shift : b0;
      return `<i class="sc-bar${st === 'ok' ? ' live' : ''}${trading ? '' : ' off'}" style="left:${pct(a)};width:${pct(e - a)}" title="${esc(h)} ET on trading days"></i>`;
    }).join('');
    const nexts = b.svcs.map(s => /until \w+ (\d\d:\d\d)/.exec(svcState(s.id).health?.body?.detail || '')?.[1]).filter(Boolean).sort();
    const open = my.open.has(b.name);
    return `<div class="sc-row sc-bundle" data-bundle="${esc(b.name)}"><i class="sc-dot ${st}"></i><span class="sc-name" title="${esc(b.note || '')}"><span class="sc-tw">${open ? '▾' : '▸'}</span>${esc(b.name)}<em>${esc(b.class || '')} · ${b.svcs.length}</em></span>
      <span class="sc-track">${bars}<i class="sc-now" style="left:${pct(now)}"></i></span><span class="sc-marks"></span><span class="sc-next">${st === 'ok' ? 'live' : nexts[0] ? 'next ' + esc(nexts[0]) : ''}</span></div>`
      + (open ? sorted(linesOf(b.svcs)).map(l => row(l, true)).join('') : '');
  };
  const axis = [0, 6, 12, 18, 24].map(h => `<span style="left:${pct(h * 60)}">${String(h).padStart(2, '0')}</span>`).join('');
  return `<div class="sc">
    <div class="sc-row sc-axis"><i></i><span class="sc-name">${trading ? `trading day${shift ? ', early close' : ''}` : 'market closed'}</span>
      <span class="sc-track"><i class="sc-session" style="left:${pct(570)};width:${pct(close - 570)}" title="Regular session"></i>${axis}<i class="sc-now" style="left:${pct(now)}" title="Now, ${esc(hm)} ET"></i></span><span class="sc-marks"></span><span class="sc-next">now ${esc(hm)} ET</span></div>
    ${bundles.map(bundleRow).join('')}
    ${sorted(linesOf(svcs.filter(s => !inBundle(s.id)))).map(l => row(l, false)).join('')}
  </div>`;
}

defineWidget('schedule', {
  title: 'Schedule',
  mount(el) {
    const my = { open: new Set() };
    const update = () => { paint(el, draw(my)); };
    const click = e => { const b = e.target.closest('[data-bundle]'); if (!b) return; const n = b.dataset.bundle; my.open.has(n) ? my.open.delete(n) : my.open.add(n); update(); };
    el.addEventListener('click', click);
    update();
    return { update, dispose: () => el.removeEventListener('click', click) };
  },
});

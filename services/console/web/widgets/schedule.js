// Schedule: what every service runs and when, from each service's
// `/describe`. The deployment graph shows the present; this shows intent:
// active hours on a day timeline (New York time), then every schedule.
import { defineWidget, redrawing } from '../lib/widgets.js';
import { fleet, svcState, statusOf } from '../lib/fleet.js';
import { esc } from '../lib/format.js';

function draw() {
  const svcs = (fleet.topo?.services || []).filter(s => Array.isArray(s.schedule));
  const head = '<div><h1>Schedule</h1><p class="muted" style="margin:0">What each service runs and when, in New York time.</p></div>';
  if (!svcs.length) return head + '<p class="muted">No service describes a schedule yet.</p>';
  // Services carrying the same feed on the same hours (the relays) are one row.
  const rows = [];
  for (const s of svcs) for (const x of s.schedule) {
    const same = x.feed && rows.find(r => r.x.feed === x.feed && r.x.hours === x.hours && r.x.kind === x.kind);
    if (same) { same.group.push(s); same.symbols.push(...(x.symbols || [])); }
    else rows.push({ s, x, group: [s], symbols: [...(x.symbols || [])] });
  }
  const who = r => r.x.feed ? r.x.feed : (r.s.label || r.s.id);
  const what = r => r.x.feed ? `${r.x.name}: ${r.symbols.join(', ')}${r.group.length > 1 ? ` (${r.group.length} relays)` : ''}` : r.x.name;
  // A group's state is its worst member's.
  const rank = { crit: 0, warn: 1, start: 2, ok: 3, off: 4, idle: 5 };
  const worst = r => r.group.slice().sort((a, b) => (rank[statusOf({ ...a, kind: 'svc' })] ?? 9) - (rank[statusOf({ ...b, kind: 'svc' })] ?? 9))[0];
  const mins = t => { const [h, m] = t.split(':').map(Number); return h * 60 + m; };
  const nyNow = new Date().toLocaleString('en-GB', { timeZone: 'America/New_York', hour: '2-digit', minute: '2-digit', hour12: false, weekday: 'short' });
  const [wd, hm] = nyNow.split(' '), nowMin = mins(hm), pctNow = (nowMin / 1440 * 100).toFixed(2);
  const cal = svcs.map(s => svcState(s.id).calendar).find(c => c?.days?.length);
  const todayNy = new Date().toLocaleDateString('en-CA', { timeZone: 'America/New_York' });
  const session = cal?.days.find(d => d.date === todayNy);
  const trading = cal ? !!session : !['Sat', 'Sun'].includes(wd);
  // A window is set relative to the 09:30–16:00 session: an early close ends it early by as much.
  const shift = session && session.close < '16:00' ? mins(session.close) - 16 * 60 : 0;
  const windows = rows.filter(r => r.x.kind === 'window' && r.x.hours);
  const ticks = [0, 3, 6, 9, 12, 15, 18, 21, 24].map(h => `<span style="left:${(h / 24 * 100).toFixed(2)}%">${String(h).padStart(2, '0')}</span>`).join('');
  const session0 = `<div class="tl-session" style="left:${(570 / 1440 * 100).toFixed(2)}%;width:${(((session ? mins(session.close) : 960) - 570) / 1440 * 100).toFixed(2)}%" title="Regular session"></div>`;
  const timeline = windows.length ? `<div class="tl">
      <div class="tl-row tl-axis"><span class="tl-label">${trading ? 'Today, a trading day' : 'Today, markets closed'}</span><div class="tl-track">${ticks}</div></div>
      ${windows.map(r => {
        const { x } = r, s = worst(r);
        const [a, b] = x.hours.split('-').map(mins), end = b <= 16 * 60 + 60 ? b + shift : b;
        const st = statusOf({ ...s, kind: 'svc' });
        return `<div class="tl-row"><span class="tl-label" title="${esc(what(r))}">${esc(who(r))}</span><div class="tl-track">${session0}
          <div class="tl-bar${trading ? '' : ' off'}${st === 'ok' ? ' live' : ''}" style="left:${(a / 1440 * 100).toFixed(2)}%;width:${((end - a) / 1440 * 100).toFixed(2)}%" title="${esc(x.when)}"></div>
          <div class="tl-now" style="left:${pctNow}%"></div></div></div>`;
      }).join('')}
      <div class="tl-legend"><span><i class="tl-key bar"></i>active hours</span><span><i class="tl-key session"></i>regular session</span><span><i class="tl-key now"></i>now, ${esc(hm)} ET</span></div>
    </div>` : '';
  const order = { window: 0, always: 1, interval: 2, event: 3 };
  const nowText = (s, x) => {
    const h = svcState(s.id).health?.body || {}, st = statusOf({ ...s, kind: 'svc' });
    if (st === 'crit') return '<span class="pill p-crit">Down</span>';
    if (x.kind === 'window') {
      const until = /until (.+)$/.exec(h.detail || '')?.[1];
      return h.state === 'idle' ? `<span class="muted">next ${esc(until || 'session')}</span>` : '<span class="pill p-ok">Running</span>';
    }
    if (x.kind === 'always') return st === 'warn' ? '<span class="pill p-warn">Needs attention</span>' : '<span class="pill p-ok">Running</span>';
    if (x.kind === 'interval') return h.state === 'idle' ? '<span class="muted">paused off-hours</span>' : '<span class="muted">repeating</span>';
    return '<span class="muted">on event</span>';
  };
  const table = rows.slice().sort((p, q) => order[p.x.kind] - order[q.x.kind] || who(p).localeCompare(who(q))).map(r =>
    `<tr><td class="mono" style="color:var(--fg)">${esc(who(r))}</td><td>${esc(what(r))}</td><td class="mono">${esc(r.x.when)}</td><td>${nowText(worst(r), r.x)}</td></tr>`).join('');
  return head + timeline + `<div class="box scroll"><table class="sched"><thead><tr><th>Service or feed</th><th>What</th><th>When</th><th>Now</th></tr></thead><tbody>${table}</tbody></table></div>`;
}

defineWidget('schedule', { title: 'Schedule', mount: redrawing(draw) });

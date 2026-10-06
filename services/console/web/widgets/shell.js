// The tinytrader shell: a terminal for read-only commands over this
// deployment's state. It runs nothing on the machine itself.
import { defineWidget } from '../lib/widgets.js';
import { fleet, svcState, catalogRoots, newsItems } from '../lib/fleet.js';
import { jobsApi } from '../lib/api.js';
import { esc, bytes, ago } from '../lib/format.js';

const termLine = l => `<div class="${l.k}">${esc(l.t)}</div>`;
const pad = (v, n) => String(v ?? '').padEnd(n);
const row = s => { const h = svcState(s.id).health?.body || {}; return pad(s.id, 20) + pad(h.state || '?', 10) + (h.detail || ''); };
const SHELL = {
  help: () => ['help                 this list', 'status               every service and its state', 'relays               market data relays and their hours', 'accounts             equity and positions of each account', 'store                drives, free space and status', 'jobs                 provider jobs not yet on disk', 'news [n]             the latest n stories (5)', 'calendar             the next trading sessions', 'clear                clear the screen'].join('\n'),
  status: () => (fleet.topo?.services || []).map(row).join('\n') || 'no services',
  relays: () => (fleet.topo?.services || []).filter(s => s.id.startsWith('relay')).map(row).join('\n') || 'no relays',
  accounts: () => (fleet.topo?.services || []).flatMap(s => svcState(s.id).accounts?.accounts || []).map(a => pad(a.label, 10) + pad(a.environment, 8) + pad('$' + Number(a.account?.equity ?? NaN).toLocaleString(undefined, { minimumFractionDigits: 2, maximumFractionDigits: 2 }), 14) + (a.positions?.length ? a.positions.map(p => `${p.symbol} ${p.qty}`).join(', ') : 'no positions')).join('\n') || 'no accounts',
  store: () => catalogRoots().map(r => { const v = r.volume || {}; return pad(r.name, 10) + (r.present ? pad(`${((v.used_fraction ?? 0) * 100).toFixed(0)}% used`, 11) + pad(bytes(v.available_bytes) + ' free', 14) + pad(v.status, 9) + (r.writable ? 'writable' : 'read-only') : 'MISSING'); }).join('\n') || 'no store',
  jobs: async () => { const v = await jobsApi('jobs'); const w = (v.jobs || []).filter(j => !j.held); return w.length ? w.map(j => pad(j.id, 26) + pad(`${j.symbols} ${j.schema}`, 24) + pad(j.state, 12) + (j.bytes ? bytes(j.bytes) : '')).join('\n') : 'nothing waiting: every job is on disk'; },
  news: n => newsItems().slice(0, +n || 5).map(x => `${pad(ago(x.created_at), 10)} ${x.headline}`).join('\n') || 'no news',
  calendar: () => { const cal = (fleet.topo?.services || []).map(s => svcState(s.id).calendar).find(c => c?.days?.length); const today = new Date().toISOString().slice(0, 10); return cal ? cal.days.filter(d => d.date >= today).slice(0, 5).map(d => `${d.date}  ${d.open}–${d.close} ET`).join('\n') : 'no calendar'; },
};
async function runShell(sh, out, line) {
  const [cmd, ...args] = line.trim().split(/\s+/);
  if (!cmd) return;
  sh.hist.push(line); sh.at = sh.hist.length;
  if (cmd === 'clear') sh.lines = [];
  else {
    sh.lines.push({ k: 'cmd', t: line });
    const f = SHELL[cmd];
    try { sh.lines.push(f ? { k: 'out', t: await f(...args) } : { k: 'err', t: `${cmd}: no such command (try help)` }); }
    catch (err) { sh.lines.push({ k: 'err', t: String(err.message || err) }); }
    sh.lines = sh.lines.slice(-400);
  }
  out.innerHTML = sh.lines.map(termLine).join(''); out.scrollTop = out.scrollHeight;
}

defineWidget('shell', {
  title: 'Shell',
  mount(el) {
    const sh = { lines: [{ k: 'sys', t: 'tinytrader shell: read-only commands over this deployment. Type help.' }], hist: [], at: 0 };
    el.innerHTML = `<div class="term"><div class="term-out">${sh.lines.map(termLine).join('')}</div><form class="term-in" autocomplete="off"><span>tinytrader$</span><input spellcheck="false" aria-label="Command" autocapitalize="off"></form></div>`;
    const out = el.querySelector('.term-out'), form = el.querySelector('form'), input = form.querySelector('input');
    form.addEventListener('submit', e => { e.preventDefault(); const v = input.value; input.value = ''; runShell(sh, out, v); });
    input.addEventListener('keydown', e => {
      if (!['ArrowUp', 'ArrowDown'].includes(e.key)) return;
      e.preventDefault(); sh.at = Math.max(0, Math.min(sh.hist.length, sh.at + (e.key === 'ArrowUp' ? -1 : 1)));
      input.value = sh.hist[sh.at] ?? '';
    });
    return {};
  },
});

// Data: coverage by dataset and symbol, merged across drives. Pick a row
// (or a schema chip) to see that schema's days on a calendar; the filter
// narrows the rows as you type.
import { defineWidget } from '../lib/widgets.js';
import { catalogRoots, catalogStamp } from '../lib/fleet.js';
import { esc, bytes, ago } from '../lib/format.js';

function coverage() {
  const m = new Map();
  for (const r of catalogRoots()) for (const d of r.datasets || []) {
    const key = d.id + ' ' + (d.symbol || '');
    const e = m.get(key) || { key, id: d.id, symbol: d.symbol, schemas: {}, drives: new Set(), sources: [] };
    e.drives.add(r.name); e.sources.push(...(d.sources || []).map(x => `${r.name}: ${x}`));
    for (const [name, s] of Object.entries(d.schemas)) {
      const t = e.schemas[name] || (e.schemas[name] = { bytes: 0, days: {}, where: {}, encodings: new Set() });
      t.bytes += s.bytes; (s.encodings || []).forEach(x => t.encodings.add(x));
      for (const [day, b] of Object.entries(s.days || {})) { t.days[day] = (t.days[day] || 0) + b; (t.where[day] ||= new Set()).add(r.name); }
    }
    m.set(key, e);
  }
  return [...m.values()].sort((a, b) => a.id.localeCompare(b.id) || (a.symbol || '').localeCompare(b.symbol || ''));
}
const coveredSize = e => Object.values(e.schemas).reduce((a, s) => a + s.bytes, 0);
function dataView(my) {
  const roots = catalogRoots();
  const head = `<div><h1>Data</h1><p class="muted" style="margin:0">What the store's drives hold, by dataset and symbol, across drives.</p></div>`;
  if (!roots.length) return head + '<p class="muted">The store is not reachable.</p>';
  const summary = `<div class="drive-sum">${roots.filter(r => r.present).map(r => `<div><b>${esc(r.name)}</b><span>${bytes(r.dataset_bytes || 0)} in datasets</span><span>${bytes(r.other_bytes || 0)} other</span><span>${r.volume?.available_bytes != null ? bytes(r.volume.available_bytes) + ' free' : ''}</span></div>`).join('')}</div>`;
  const rows = coverage();
  if (!rows.some(e => e.key === my.ds)) my.ds = rows[0]?.key;
  const sel = rows.find(e => e.key === my.ds);
  const table = rows.map(e => {
    const chips = Object.entries(e.schemas).sort((a, b) => b[1].bytes - a[1].bytes).map(([name, s]) =>
      `<button type="button" class="cov-chip" data-cov="${esc(e.key)}" data-schema="${esc(name)}"${e === sel && name === my.schema ? ' aria-pressed="true"' : ''}>${esc(name)} <span>${Object.keys(s.days).length}d</span></button>`).join('');
    return `<tr class="ds-row${e === sel ? ' sel' : ''}" data-cov="${esc(e.key)}" data-text="${esc((e.id + ' ' + (e.symbol || '') + ' ' + Object.keys(e.schemas).join(' ')).toLowerCase())}">
      <td class="mono" style="color:var(--fg)">${esc(e.symbol || '—')}</td><td class="mono">${esc(e.id)}</td><td>${chips}</td>
      <td class="mono num">${bytes(coveredSize(e))}</td><td class="mono">${[...e.drives].map(esc).join(', ')}</td></tr>`;
  }).join('');
  const other = roots.flatMap(r => (r.other || []).map(o => ({ r, o }))).sort((x, y) => y.o.bytes - x.o.bytes);
  const otherTable = other.length ? `<section><div class="sec-head"><h2>Other data on disk</h2><span class="muted" style="font-size:13px">Folders the store doesn't recognise as datasets, largest first</span></div>
    <div class="box scroll"><table><thead><tr><th>Folder</th><th>Drive</th><th style="text-align:right">Size</th><th style="text-align:right">Files</th><th>Last changed</th></tr></thead><tbody>${other.map(({ r, o }) =>
      `<tr><td class="mono" style="color:var(--fg)">${esc(o.name)}</td><td class="mono">${esc(r.name)}</td><td class="mono num">${bytes(o.bytes)}</td><td class="mono num">${(o.files || 0).toLocaleString()}</td><td class="muted">${o.modified ? esc(ago(new Date(o.modified * 1000).toISOString())) : '—'}</td></tr>`).join('')}</tbody></table></div></section>` : '';
  return head + summary + `<input class="ds-filter" data-ds-filter placeholder="Filter: XSP, SPY.OPT, cmbp-1…" value="${esc(my.filter || '')}" autocomplete="off" spellcheck="false">
    <div class="box scroll"><table class="ds-tbl"><thead><tr><th>Symbol</th><th>Dataset</th><th>Schemas (days with data)</th><th style="text-align:right">Size</th><th>Drives</th></tr></thead><tbody>${table}</tbody></table></div>
    ${sel ? coveragePanel(sel, my) : ''}${otherTable}`;
}
function coveragePanel(e, my) {
  const names = Object.keys(e.schemas);
  if (!names.includes(my.schema)) my.schema = names.includes('cmbp-1') ? 'cmbp-1' : names.sort((a, b) => e.schemas[b].bytes - e.schemas[a].bytes)[0];
  const s = e.schemas[my.schema], keys = Object.keys(s.days).sort();
  const first = keys[0], last = keys[keys.length - 1];
  // Weekdays in the range with no data (market holidays count among them).
  const missing = [];
  for (let t = Date.parse(first + 'T12:00:00Z'); t <= Date.parse(last + 'T12:00:00Z'); t += 864e5) {
    const d = new Date(t), k = d.toISOString().slice(0, 10), wd = d.getUTCDay();
    if (wd !== 0 && wd !== 6 && !s.days[k]) missing.push(k);
  }
  const perDrive = {}; for (const set of Object.values(s.where)) for (const n of set) perDrive[n] = (perDrive[n] || 0) + 1;
  let cal = '';
  const start = Date.parse(first + 'T00:00:00Z') - ((new Date(first + 'T00:00:00Z').getUTCDay() + 6) % 7) * 864e5;
  for (let t = start; t <= Date.parse(last + 'T00:00:00Z') + 6 * 864e5; t += 864e5) {
    const k = new Date(t).toISOString().slice(0, 10), wd = new Date(t).getUTCDay();
    const out = k < first || k > last, has = s.days[k];
    const cls = out ? 'out' : has ? 'has' : (wd === 0 || wd === 6) ? 'weekend' : '';
    cal += `<i class="${cls}" title="${k}${has ? ` · ${bytes(has)} · ${[...s.where[k]].join(', ')}` : cls === '' ? ' · no data' : ''}"></i>`;
  }
  return `<section class="panel">
    <div class="sec-head" style="margin:0"><h2 class="mono" style="font-weight:500">${esc(e.symbol || e.id)} <span class="muted" style="font-size:13px;font-family:var(--ui)">${esc(e.symbol ? e.id : '')}</span></h2></div>
    <div class="chips">${names.map(x => `<button type="button" class="chip-btn" data-cov="${esc(e.key)}" data-schema="${esc(x)}" aria-pressed="${x === my.schema}">${esc(x)}</button>`).join('')}</div>
    <div class="facts">
      <div><div class="k">Days with data</div><div class="v">${keys.length}</div></div>
      <div><div class="k">First → last</div><div class="v">${first} → ${last}</div></div>
      <div><div class="k">Weekdays without data</div><div class="v">${missing.length}${missing.length ? ` <span class="muted" style="font-size:12px">(holidays included)</span>` : ''}</div></div>
      <div><div class="k">Size</div><div class="v">${bytes(s.bytes)}</div></div>
      <div><div class="k">Drives</div><div class="v">${Object.entries(perDrive).map(([n, c]) => `${esc(n)} (${c}d)`).join(', ')}</div></div>
    </div>
    <div class="cal-wrap"><div class="cal" aria-label="Days with data">${cal}</div></div>
    <div class="cal-legend"><span><i style="background:var(--accent-mark)"></i>data</span><span><i style="background:var(--line-soft)"></i>weekday without data</span><span><i style="outline:1px dashed var(--line-soft)"></i>weekend</span></div>
    ${missing.length && missing.length <= 40 ? `<div class="muted" style="font-size:12px;font-family:var(--mono);overflow-wrap:anywhere">Missing: ${missing.join(', ')}</div>` : ''}
  </section>`;
}

defineWidget('data', {
  title: 'Data',
  mount(el) {
    const my = { ds: null, schema: null, filter: '', stamp: null };
    const filter = () => {
      const q = my.filter.toLowerCase().split(/\s+/).filter(Boolean);
      el.querySelectorAll('.ds-tbl tr[data-text]').forEach(tr => { tr.hidden = !q.every(w => tr.dataset.text.includes(w)); });
    };
    // Redrawn when the catalog changes or on a pick, never under typing.
    const draw = () => {
      const typing = document.activeElement?.matches?.('[data-ds-filter]') && el.contains(document.activeElement);
      const pos = typing ? document.activeElement.selectionStart : 0, top = el.scrollTop;
      my.stamp = catalogStamp(); el.innerHTML = dataView(my); filter(); el.scrollTop = top;
      if (typing) { const i = el.querySelector('[data-ds-filter]'); i.focus(); i.setSelectionRange(pos, pos); }
    };
    const click = e => {
      const c = e.target.closest('[data-cov]'); if (!c) return;
      my.ds = c.dataset.cov; if (c.dataset.schema) my.schema = c.dataset.schema; draw();
    };
    const input = e => { if (e.target.matches('[data-ds-filter]')) { my.filter = e.target.value; filter(); } };
    el.addEventListener('click', click); el.addEventListener('input', input);
    draw();
    return {
      update() { if (catalogStamp() !== my.stamp) draw(); },
      dispose() { el.removeEventListener('click', click); el.removeEventListener('input', input); },
    };
  },
});

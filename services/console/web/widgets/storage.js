// Storage: the store's drives as one pool (decision 0007): the pool's free
// space first, then each drive against the store's thresholds, then any new
// drive that could be approved for the store.
import { defineWidget } from '../lib/widgets.js';
import { fleet, catalogRoots, catalogStamp } from '../lib/fleet.js';
import { esc, bytes } from '../lib/format.js';
import { orderFetch } from '../lib/api.js';

// What fills a drive: each dataset (by its path on the drive) and every
// other folder, largest first.
function contents(r) {
  const items = (r.datasets || []).map(d => {
    const src = (d.sources || [])[0]?.replace(/^[^:]*:\s*/, '') || '';
    let path = src.startsWith(r.path) ? src.slice(r.path.length).replace(/^\//, '') : (src || d.id);
    if (d.symbol && !path.endsWith(d.symbol)) path += '/' + d.symbol;
    return { path, bytes: Object.values(d.schemas).reduce((a, x) => a + x.bytes, 0), kind: 'data', note: Object.keys(d.schemas).join(', ') };
  });
  for (const o of r.other || []) items.push({ path: o.name, bytes: o.bytes, kind: 'other', note: `${(o.files || 0).toLocaleString()} files` });
  return items.sort((a, b) => b.bytes - a.bytes);
}

function fill(r) {
  const v = /** @type {Partial<Volume>} */ (r.volume || {}), total = v.total_bytes;
  if (!total) return '';
  const items = contents(r).filter(x => x.bytes > 0);
  const known = items.reduce((a, x) => a + x.bytes, 0), used = total - (v.available_bytes ?? 0);
  const seg = (w, cls, title) => w > 0 ? `<i class="${cls}" style="width:${(w / total * 100).toFixed(2)}%" title="${esc(title)}"></i>` : '';
  const bar = items.map(x => seg(x.bytes, x.kind, `${x.path}: ${bytes(x.bytes)}`)).join('')
    + seg(Math.max(0, used - known), 'rest', `everything else on the drive: ${bytes(Math.max(0, used - known))}`)
    + seg(v.available_bytes ?? 0, 'free', `free: ${bytes(v.available_bytes)}`);
  const rows = items.map(x => `<tr><td class="mono" style="color:var(--fg)"><i class="sw ${x.kind}"></i>${esc(x.path)}</td><td class="muted">${esc(x.note)}</td><td class="mono num">${bytes(x.bytes)}</td></tr>`).join('');
  return `<div class="fillbar">${bar}</div>
    <div class="scroll"><table class="fill-tbl"><tbody>${rows}</tbody></table></div>`;
}

const statusPill = { ok: ['OK', 'p-ok'], warning: ['Warning', 'p-warn'], critical: ['Critical', 'p-crit'], floor: ['Full', 'p-crit'], unknown: ['Space unknown', 'p-idle'] };

// Drives the store found that it doesn't use yet.
const candidates = () => Object.values(fleet.snap?.services || {}).flatMap(s => s.catalog?.candidates || []);

function storageView(my) {
  const roots = catalogRoots();
  const t = /** @type {Partial<Thresholds>} */ (roots[0]?.thresholds || {});
  // The pool: what new data can still use, below the critical threshold, on writable drives present.
  const live = roots.filter(r => r.present && r.writable && r.volume?.total_bytes);
  const room = live.reduce((a, r) => a + Math.max(0, r.volume.available_bytes - r.volume.total_bytes * (1 - (t.critical ?? 90) / 100)), 0);
  const total = live.reduce((a, r) => a + r.volume.total_bytes, 0), free = live.reduce((a, r) => a + r.volume.available_bytes, 0);
  const pool = live.length ? `<div class="pool"><b>${bytes(room)}</b><span>room for new data across ${live.length} drive${live.length === 1 ? '' : 's'}</span><span class="muted">${bytes(free)} free of ${bytes(total)}; each drive fills to ${t.critical ?? 90}%</span></div>` : '';
  const cards = roots.map(r => {
    const v = /** @type {Partial<Volume>} */ (r.volume || {}), used = (v.used_fraction ?? 0) * 100, st = statusPill[v.status] || ['Missing', 'p-crit'];
    const marks = [t.warning, t.critical, t.floor].filter(Boolean).map(x => `<em style="left:${x}%" title="${x}%"></em>`).join('');
    return `<article class="panel">
      <div class="d-head"><b>${esc(r.name)}</b><span class="pill ${r.present ? st[1] : 'p-crit'}">${r.present ? st[0] : 'Missing'}</span>${r.writable === false ? '<span class="muted" style="font-size:12px">read-only</span>' : ''}<span class="muted mono" style="margin-left:auto;font-size:12px">${esc(r.path)}</span></div>
      ${r.present ? `${v.status === 'unknown' ? `<p class="muted" style="margin:0;font-size:14px">${esc(v.reason || 'Disk space is not available for this root.')}</p>` : `<div class="usage"><b class="${esc(v.status)}" style="width:${used.toFixed(1)}%"></b>${marks}</div>`}
      <div class="facts">
        ${v.status === 'unknown' ? '' : `<div><div class="k">Used</div><div class="v">${used.toFixed(0)}% of ${bytes(v.total_bytes)}</div></div>
        <div><div class="k">Free</div><div class="v">${bytes(v.available_bytes)}</div></div>`}
        <div><div class="k">In datasets</div><div class="v">${bytes(r.dataset_bytes)} · ${(r.datasets || []).length} datasets</div></div>
      </div>
      ${fill(r)}`
      : '<p class="muted" style="margin:0">This drive is not available: it may be unplugged. Its files are listed as unavailable until it returns.</p>'}
    </article>`;
  }).join('');
  const found = candidates();
  const fresh = found.length ? `<section><div class="sec-head"><h2>New drives</h2><span class="muted" style="font-size:13px">Mounted, not used by the store</span></div>
    <div class="roots">${found.map(c => {
      const asking = my.asking === c.path;
      return `<article class="panel" data-drive="${esc(c.path)}">
        <div class="d-head"><b>${esc(c.label)}</b><span class="muted mono" style="margin-left:auto;font-size:12px">${esc(c.path)}</span></div>
        <div class="facts"><div><div class="k">Size</div><div class="v">${bytes(c.total_bytes)}</div></div><div><div class="k">Free</div><div class="v">${bytes(c.available_bytes)}</div></div></div>
        ${asking
          ? `<p style="margin:0;font-size:14px">This writes one small file, <code>.tinytrader-volume</code>, at the top of ${esc(c.label)}, naming it <b>${esc(my.name)}</b>. The store then places new data there when it has the most room. Nothing on the drive is moved or changed.</p>
             <div class="drive-acts"><button type="button" class="btn" data-drive-cancel>Cancel</button><button type="button" class="btn primary" data-drive-approve>Approve ${esc(c.label)}</button></div>`
          : `<div class="drive-acts"><label class="field" style="flex:1"><span class="k">Name in the store</span><input data-drive-name value="${esc(my.names[c.path] ?? c.label.toLowerCase().replace(/[^a-z0-9._-]+/g, '-'))}" spellcheck="false" autocomplete="off"></label><button type="button" class="btn" data-drive-use>Use for the store…</button></div>`}
        ${my.err[c.path] ? `<p class="msg-err" style="margin:0">${esc(my.err[c.path])}</p>` : ''}
      </article>`;
    }).join('')}</div></section>` : '';
  return `<div><h1>Storage</h1><p class="muted" style="margin:0">The store's drives as one pool. Marks show the thresholds: ${t.warning ?? 80}% warn, ${t.critical ?? 90}% no new data on that drive, ${t.floor ?? 97}% stop capture. Nothing is ever deleted automatically.</p></div>
    ${pool}<div class="roots">${cards || '<p class="muted">No data roots configured.</p>'}</div>${fresh}`;
}

defineWidget('storage', {
  title: 'Storage',
  mount(el) {
    const my = { asking: null, name: '', names: {}, err: {}, stamp: null, found: '' };
    const draw = () => { my.stamp = catalogStamp(); my.found = JSON.stringify(candidates()); const top = el.scrollTop; el.innerHTML = storageView(my); el.scrollTop = top; };
    const input = e => { const d = e.target.closest('[data-drive]'); if (d && e.target.matches('[data-drive-name]')) my.names[d.dataset.drive] = e.target.value; };
    const click = async e => {
      const d = e.target.closest('[data-drive]'); if (!d) return;
      const path = d.dataset.drive;
      if (e.target.closest('[data-drive-use]')) {
        my.name = (d.querySelector('[data-drive-name]').value || '').trim(); my.names[path] = my.name; delete my.err[path];
        if (!/^[A-Za-z0-9][A-Za-z0-9._-]*$/.test(my.name)) { my.err[path] = 'A name uses letters, digits, ".", "_" and "-".'; draw(); return; }
        my.asking = path; draw(); return;
      }
      if (e.target.closest('[data-drive-cancel]')) { my.asking = null; draw(); return; }
      if (e.target.closest('[data-drive-approve]')) {
        e.target.disabled = true;
        try {
          const r = await orderFetch('/api/store/volumes/approve', { path, name: my.name });
          const v = await r.json().catch(() => ({}));
          if (!r.ok) throw new Error(v.error || `${r.status} ${r.statusText}`);
          my.asking = null;
        } catch (err) { my.err[path] = err.message; my.asking = null; }
        draw();
      }
    };
    el.addEventListener('input', input); el.addEventListener('click', click);
    draw();
    return {
      // Redrawn when the catalog or the drives found change, never mid-question or under typing.
      update() {
        if (my.asking || el.contains(document.activeElement)) return;
        if (catalogStamp() !== my.stamp || JSON.stringify(candidates()) !== my.found) draw();
      },
      dispose() { el.removeEventListener('input', input); el.removeEventListener('click', click); },
    };
  },
});

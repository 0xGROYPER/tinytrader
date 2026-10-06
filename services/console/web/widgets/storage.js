// Storage: each data root and the volume it lives on, against the store's
// thresholds.
import { defineWidget } from '../lib/widgets.js';
import { catalogRoots, catalogStamp } from '../lib/fleet.js';
import { esc, bytes } from '../lib/format.js';

function storageView() {
  const roots = catalogRoots();
  const statusPill = { ok: ['OK', 'p-ok'], warning: ['Warning', 'p-warn'], critical: ['Critical', 'p-crit'], floor: ['Full', 'p-crit'], unknown: ['Space unknown', 'p-idle'] };
  const t = roots[0]?.thresholds || {};
  const cards = roots.map(r => {
    const v = r.volume || {}, used = (v.used_fraction ?? 0) * 100, st = statusPill[v.status] || ['Missing', 'p-crit'];
    const marks = [t.warning, t.critical, t.floor].filter(Boolean).map(x => `<em style="left:${x}%" title="${x}%"></em>`).join('');
    return `<article class="panel">
      <div class="d-head"><b>${esc(r.name)}</b><span class="pill ${r.present ? st[1] : 'p-crit'}">${r.present ? st[0] : 'Missing'}</span><span class="muted mono" style="margin-left:auto;font-size:12px">${esc(r.path)}</span></div>
      ${r.present ? `${v.status === 'unknown' ? `<p class="muted" style="margin:0;font-size:14px">${esc(v.reason || 'Disk space is not available for this root.')}</p>` : `<div class="usage"><b class="${esc(v.status)}" style="width:${used.toFixed(1)}%"></b>${marks}</div>`}
      <div class="facts">
        ${v.status === 'unknown' ? '' : `<div><div class="k">Used</div><div class="v">${used.toFixed(0)}% of ${bytes(v.total_bytes)}</div></div>
        <div><div class="k">Free</div><div class="v">${bytes(v.available_bytes)}</div></div>`}
        <div><div class="k">In datasets</div><div class="v">${bytes(r.dataset_bytes)} · ${(r.datasets || []).length} datasets</div></div>
      </div>
      ${(r.unrecognised || []).length ? `<div><div class="k" style="font-size:12px;color:var(--muted);margin-bottom:6px">Not in the store layout</div><div class="chips">${r.unrecognised.map(x => `<span class="tag">${esc(x)}</span>`).join('')}</div></div>` : ''}`
      : '<p class="muted" style="margin:0">This root\'s path is not available: the drive may be unplugged.</p>'}
    </article>`;
  }).join('');
  return `<div><h1>Storage</h1><p class="muted" style="margin:0">Each data root and the volume it lives on. Marks show the thresholds: ${t.warning ?? 80}% warn, ${t.critical ?? 90}% pause non-capture writers, ${t.floor ?? 97}% stop capture. Nothing is ever deleted automatically.</p></div>
    <div class="roots">${cards || '<p class="muted">No data roots configured.</p>'}</div>`;
}

defineWidget('storage', {
  title: 'Storage',
  mount(el) {
    let stamp = null;
    const draw = () => { stamp = catalogStamp(); const top = el.scrollTop; el.innerHTML = storageView(); el.scrollTop = top; };
    draw();
    return { update() { if (catalogStamp() !== stamp) draw(); } };
  },
});

// Line charts for series a service serves: any element with data-chart
// "service|stream|key" is drawn from the latest snapshot, with a hover
// crosshair and tooltip. Drawn after each refresh and when panels resize.
import { svcState } from './fleet.js';
import { esc, fmt, etStamp } from './format.js';

// ---------- charts ----------
function tipAt(el, html, px, py) {
  let tip = el.querySelector('.tip'); if (!tip) { tip = document.createElement('div'); tip.className = 'tip'; el.appendChild(tip); }
  tip.innerHTML = html; tip.hidden = false;
  const w = tip.offsetWidth;
  tip.style.left = Math.min(Math.max(px - w / 2, 0), el.clientWidth - w) + 'px';
  tip.style.top = (py - tip.offsetHeight - 10) + 'px';
}

export function drawCharts() {
  document.querySelectorAll('[data-chart]').forEach((/** @type {HTMLElement} */ el) => {
    const [id, source, key] = el.dataset.chart.split('|');
    const decimals = +el.dataset.decimals;
    const pts = (svcState(id)[source] || {})[key] || [];
    if (pts.length < 2) { el.innerHTML = '<div class="empty" style="padding:8px 0">Collecting…</div>'; return; }
    const W = el.clientWidth || 320, H = +(el.dataset.height || 110), m = { l: 0, r: 6, t: 8, b: 8 };
    // Unchanged series keep their object between polls: skip a redraw that would draw the same.
    const e = /** @type {any} */ (el);
    if (e.ttPts === pts && e.ttW === W && el.firstElementChild) return;
    e.ttPts = pts; e.ttW = W;
    const xs = pts.map(p => p[0]), ys = pts.map(p => p[1]);
    const x0 = xs[0], x1 = xs[xs.length - 1], lo = Math.min(...ys), hi = Math.max(...ys), pad = (hi - lo) * 0.12 || hi * 0.0005 || 1;
    const X = x => m.l + (x - x0) / (x1 - x0 || 1) * (W - m.l - m.r), Y = y => m.t + (1 - (y - (lo - pad)) / (hi - lo + 2 * pad)) * (H - m.t - m.b);
    const d = pts.map((p, i) => (i ? 'L' : 'M') + X(p[0]).toFixed(1) + ',' + Y(p[1]).toFixed(1)).join('');
    const tone = ys[ys.length - 1] >= ys[0] ? 'up' : 'down', last = pts[pts.length - 1];
    el.innerHTML = `<svg width="${W}" height="${H}" role="img" aria-label="${esc(key)}">
      <line class="c-ref" x1="0" x2="${W}" y1="${Y(ys[0])}" y2="${Y(ys[0])}"/>
      <path class="c-area ${tone}" d="${d}L${X(x1)},${H}L${X(x0)},${H}Z"/><path class="c-line ${tone}" d="${d}"/>
      <circle class="c-dot ${tone}" cx="${X(last[0])}" cy="${Y(last[1])}" r="4"/>
      <g class="hover" visibility="hidden"><line class="c-cross" y1="0" y2="${H}"/><circle class="c-dot ${tone}" r="4"/></g>
      <rect class="c-hit" x="0" y="0" width="${W}" height="${H}"/></svg>`;
    const svg = el.querySelector('svg'), hov = svg.querySelector('.hover'), cross = hov.querySelector('line'), dot = hov.querySelector('circle');
    const move = e => {
      const r = svg.getBoundingClientRect(), xv = x0 + (e.clientX - r.left) / W * (x1 - x0);
      let best = pts[0]; for (const p of pts) if (Math.abs(p[0] - xv) < Math.abs(best[0] - xv)) best = p;
      const cx = X(best[0]), cy = Y(best[1]);
      cross.setAttribute('x1', String(cx)); cross.setAttribute('x2', String(cx)); dot.setAttribute('cx', String(cx)); dot.setAttribute('cy', String(cy));
      hov.setAttribute('visibility', 'visible');
      tipAt(el, `${fmt(best[1], { decimals })}<span class="muted">${etStamp(best[0], true)}</span>`, cx, cy);
    };
    const hit = svg.querySelector('.c-hit');
    hit.addEventListener('pointermove', move); hit.addEventListener('pointerdown', move);
    hit.addEventListener('pointerleave', () => { hov.setAttribute('visibility', 'hidden'); const t = /** @type {HTMLElement | null} */ (el.querySelector('.tip')); if (t) t.hidden = true; });
  });
}

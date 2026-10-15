// Relationships: line charts drawn after matplotlib's look (plain axes with
// ticks, a light grid, the tab10 colours, a legend), from daily closes
// (lib/trend.js). Two kinds:
//   - Correlation: the rolling correlation of daily returns for pairs (stocks
//     and bonds, stocks and the dollar, oil and energy stocks, …), over 20,
//     60 or 120 sessions, from −1 to +1: how the relationships the market
//     leans on are holding or breaking.
//   - Performance: each symbol indexed to 100 at the start, over 3 months,
//     6 months or a year.
// Pairs and symbols are edited and kept in this browser. Hover for the
// values on a day. Read when shown, and again once ten minutes old.
import { defineWidget, paint } from '../lib/widgets.js';
import { fleet } from '../lib/fleet.js';
import { esc } from '../lib/format.js';
import { closes, label, isYield } from '../lib/trend.js';

const STALE = 10 * 60e3;
// Matplotlib's tab10, in its order.
const COLOURS = ['#1f77b4', '#ff7f0e', '#2ca02c', '#d62728', '#9467bd', '#8c564b', '#e377c2', '#7f7f7f'];
const PAIRS = [['SPY', 'TLT'], ['SPY', 'UUP'], ['SPY', 'GLD'], ['USO', 'XLE'], ['QQQ', 'BTC/USD']];
const PERF = ['SPY', 'TLT', 'GLD', 'USO', 'UUP', 'BTC/USD'];
const store = (k, v) => { try { if (v === undefined) return localStorage.getItem(k); localStorage.setItem(k, v); } catch (_) { /* private mode */ } return null; };
const json = (k, d) => { try { const v = JSON.parse(store(k) || 'null'); return v ?? d; } catch (_) { return d; } };
const sym = s => /^[A-Z0-9.]{1,10}$/.test(s) || /^[A-Z]{2,6}\/[A-Z]{2,6}$/.test(s);
const day = t => new Date(t * 1000).toLocaleDateString('en-CA', { timeZone: 'America/New_York' });

/** Daily changes on the dates both have: returns, or point changes for a yield. */
function aligned(a, b, ya, yb) {
  const mb = new Map(b.map(([t, c]) => [day(t), c])), out = [], ch = (y, c, p) => y ? c - p : Math.log(c / p);
  let pa = null, pb = null;
  for (const [t, c] of a) {
    const cb = mb.get(day(t));
    if (cb == null) continue;
    if (pa != null) out.push([t, ch(ya, c, pa), ch(yb, cb, pb)]);
    pa = c; pb = cb;
  }
  return out;
}

/** The rolling correlation of two series' daily changes over `w` sessions: [t, r][]. */
function rolling(a, b, w, ya, yb) {
  const r = aligned(a, b, ya, yb);
  const out = [];
  for (let i = w; i <= r.length; i++) {
    const win = r.slice(i - w, i), n = win.length;
    let sx = 0, sy = 0, sxx = 0, syy = 0, sxy = 0;
    for (const [, x, y] of win) { sx += x; sy += y; sxx += x * x; syy += y * y; sxy += x * y; }
    const vx = sxx - sx * sx / n, vy = syy - sy * sy / n;
    if (vx > 0 && vy > 0) out.push([win[n - 1][0], (sxy - sx * sy / n) / Math.sqrt(vx * vy)]);
  }
  return out;
}

const W = 760, H = 300, PAD = { l: 46, r: 96, t: 14, b: 30 };
function chart(lines, yLo, yHi, fmt, zero, hover, endFmt = fmt) {
  const xs = lines.flatMap(l => l.pts.map(p => p[0]));
  if (!xs.length) return '<p class="muted">Not enough history for these.</p>';
  const x0 = Math.min(...xs), x1 = Math.max(...xs), w = W - PAD.l - PAD.r, h = H - PAD.t - PAD.b;
  const X = t => PAD.l + (t - x0) / (x1 - x0 || 1) * w, Y = v => PAD.t + (yHi - v) / (yHi - yLo) * h;
  // Ticks: y at round steps; x at month starts.
  const span = yHi - yLo, step = [0.1, 0.2, 0.25, 0.5, 1, 2, 5, 10, 20, 25, 50, 100].find(s => span / s <= 6) || 100;
  const yt = []; for (let v = Math.ceil(yLo / step) * step; v <= yHi + 1e-9; v += step) yt.push(+v.toFixed(6));
  const xt = []; const d = new Date(x0 * 1000); d.setUTCDate(1); d.setUTCHours(12);
  while (d.getTime() / 1000 <= x1) { d.setUTCMonth(d.getUTCMonth() + 1); const t = d.getTime() / 1000; if (t >= x0 && t <= x1) xt.push(t); }
  const every = Math.ceil(xt.length / 7);
  const path = pts => pts.map((p, i) => `${i ? 'L' : 'M'}${X(p[0]).toFixed(1)},${Y(p[1]).toFixed(1)}`).join('');
  // Direct labels at the right end, nudged apart.
  const ends = lines.filter(l => l.pts.length).map(l => ({ l, y: Y(l.pts[l.pts.length - 1][1]) })).sort((a, b) => a.y - b.y);
  for (let i = 1; i < ends.length; i++) if (ends[i].y - ends[i - 1].y < 13) ends[i].y = ends[i - 1].y + 13;
  const hov = hover != null ? (() => {
    const t = hover, rows = lines.map(l => { let best = null; for (const p of l.pts) if (!best || Math.abs(p[0] - t) < Math.abs(best[0] - t)) best = p; return best ? { l, p: best } : null; }).filter(Boolean);
    return `<line class="mp-cross" x1="${X(t).toFixed(1)}" x2="${X(t).toFixed(1)}" y1="${PAD.t}" y2="${PAD.t + h}"/>${rows.map(({ l, p }) => `<circle cx="${X(p[0]).toFixed(1)}" cy="${Y(p[1]).toFixed(1)}" r="3.5" fill="${l.colour}" class="mp-dot"/>`).join('')}`;
  })() : '';
  return `<svg class="mp" viewBox="0 0 ${W} ${H}" role="img" aria-label="${esc(lines.map(l => l.name).join(', '))}">
    ${yt.map(v => `<line class="mp-grid" x1="${PAD.l}" x2="${PAD.l + w}" y1="${Y(v).toFixed(1)}" y2="${Y(v).toFixed(1)}"/><line class="mp-tick" x1="${PAD.l - 4}" x2="${PAD.l}" y1="${Y(v).toFixed(1)}" y2="${Y(v).toFixed(1)}"/><text class="mp-t" x="${PAD.l - 7}" y="${(Y(v) + 3.5).toFixed(1)}" text-anchor="end">${fmt(v)}</text>`).join('')}
    ${xt.filter((_, i) => i % every === 0).map(t => `<line class="mp-tick" x1="${X(t).toFixed(1)}" x2="${X(t).toFixed(1)}" y1="${PAD.t + h}" y2="${PAD.t + h + 4}"/><text class="mp-t" x="${X(t).toFixed(1)}" y="${PAD.t + h + 16}" text-anchor="middle">${new Date(t * 1000).toLocaleDateString(undefined, { timeZone: 'UTC', month: 'short', year: '2-digit' })}</text>`).join('')}
    ${zero != null && zero >= yLo && zero <= yHi ? `<line class="mp-zero" x1="${PAD.l}" x2="${PAD.l + w}" y1="${Y(zero).toFixed(1)}" y2="${Y(zero).toFixed(1)}"/>` : ''}
    <rect class="mp-frame" x="${PAD.l}" y="${PAD.t}" width="${w}" height="${h}"/>
    ${lines.map(l => `<path d="${path(l.pts)}" stroke="${l.colour}" class="mp-line"/>`).join('')}
    ${ends.map(({ l, y }) => `<text class="mp-end" x="${PAD.l + w + 6}" y="${(y + 3.5).toFixed(1)}" fill="${l.colour}">${esc(l.short)} ${endFmt(l.pts[l.pts.length - 1][1])}</text>`).join('')}
    ${hov}
    <rect class="mp-hit" x="${PAD.l}" y="${PAD.t}" width="${w}" height="${h}" data-x0="${x0}" data-x1="${x1}"/>
  </svg>`;
}

function draw(my) {
  const kinds = [['corr', 'Correlation'], ['perf', 'Performance']];
  const opts = my.kind === 'corr' ? [[20, '20 days'], [60, '60 days'], [120, '120 days']] : [[63, '3 months'], [126, '6 months'], [252, '1 year']];
  const pick = my.kind === 'corr' ? my.win : my.span;
  const head = `<div class="mp-head"><span class="chips">${kinds.map(([k, t]) => `<button type="button" class="chip-btn" data-rl-kind="${k}" aria-pressed="${my.kind === k}">${t}</button>`).join('')}</span>
    <span class="chips">${opts.map(([v, t]) => `<button type="button" class="chip-btn" data-rl-opt="${v}" aria-pressed="${pick === v}">${t}</button>`).join('')}</span>
    <button type="button" class="btn small" data-rl="edit">Edit</button></div>`;
  if (my.edit) {
    const v = my.kind === 'corr' ? my.pairs.map(p => p.join(' ')).join(', ') : my.syms.join(', ');
    return head + `<form class="ca-edit" data-rl-form><label><span>${my.kind === 'corr' ? 'Pairs' : 'Symbols'}</span><input name="v" class="wl-in" value="${esc(v)}" spellcheck="false" autocomplete="off"></label>
      <p class="muted" style="margin:0;font-size:12px">${my.kind === 'corr' ? 'Pairs of symbols, a space between the two and a comma between pairs: SPY TLT, USO XLE.' : 'Symbols, commas between: SPY, TLT, GLD.'} Stocks, ETFs, crypto pairs (BTC/USD) and Cboe's indexes (VIX, TNX).</p>
      <div class="wl-bar"><button class="btn small primary">Save</button><button type="button" class="btn small" data-rl="cancel">Cancel</button><button type="button" class="btn small" data-rl="reset">Back to the starting ones</button></div></form>`;
  }
  if (!my.series) return head + `<p class="muted">${my.err ? esc(my.err) : 'Reading daily closes…'}</p>`;
  const get = s => my.series.get(s)?.closes || [];
  let lines, svg;
  if (my.kind === 'corr') {
    lines = my.pairs.map(([a, b], i) => ({ name: `${label(a)} and ${label(b)}`, short: `${a.split('/')[0]}·${b.split('/')[0]}`, colour: COLOURS[i % COLOURS.length], pts: rolling(get(a), get(b), my.win, isYield(a), isYield(b)) }));
    svg = chart(lines, -1, 1, v => v.toFixed(1), 0, my.hover, v => v.toFixed(2));
  } else {
    lines = my.syms.map((s, i) => {
      const c = get(s).slice(-my.span - 1), base = c[0]?.[1];
      return { name: label(s), short: s.split('/')[0], colour: COLOURS[i % COLOURS.length], pts: base ? c.map(([t, v]) => [t, v / base * 100]) : [] };
    });
    const vals = lines.flatMap(l => l.pts.map(p => p[1])), lo = Math.min(100, ...vals), hi = Math.max(100, ...vals), pad = (hi - lo) * 0.06 || 1;
    svg = chart(lines, lo - pad, hi + pad, v => v.toFixed(0), 100, my.hover);
  }
  // The legend, and the values on the day hovered.
  const at = my.hover;
  const value = l => { if (!l.pts.length) return ''; if (at == null) { const b = l.pts[l.pts.length - 1]; return ` <b class="mono">${my.kind === 'corr' ? b[1].toFixed(2) : b[1].toFixed(1)}</b>`; } let b = l.pts[0]; for (const p of l.pts) if (Math.abs(p[0] - at) < Math.abs(b[0] - at)) b = p; return ` <b class="mono">${my.kind === 'corr' ? b[1].toFixed(2) : b[1].toFixed(1)}</b>`; };
  const legend = `<div class="mp-legend">${at != null ? `<span class="mono muted">${day(at)}</span>` : ''}${lines.map(l => `<span><i style="background:${l.colour}"></i>${esc(l.name)}${value(l)}</span>`).join('')}</div>`;
  const note = my.kind === 'corr' ? `Correlation of daily returns over the last ${my.win} sessions shared by each pair (−1 move opposite, +1 together).` : `Each indexed to 100 ${my.span} sessions back (calendar days for crypto); a yield by its level.`;
  return head + legend + `<div class="mp-wrap">${svg}</div><p class="muted mp-note">${note}</p>`;
}

defineWidget('relations', {
  title: 'Relationships',
  mount(el) {
    const my = {
      kind: store('tt-rl-kind') === 'perf' ? 'perf' : 'corr', win: +(store('tt-rl-win') || 60), span: +(store('tt-rl-span') || 252),
      pairs: /** @type {string[][]} */ (json('tt-rl-pairs', PAIRS)), syms: /** @type {string[]} */ (json('tt-rl-syms', PERF)),
      series: /** @type {Map<string, any> | null} */ (null), err: '', at: 0, edit: false, hover: /** @type {number | null} */ (null), alive: true,
    };
    const update = () => { paint(el, draw(my)); };
    async function load() {
      const r = await closes(fleet, [...new Set([...my.pairs.flat(), ...my.syms])]);
      if (!my.alive) return;
      my.series = r.series; my.err = r.error; my.wait = r.wait; my.at = Date.now(); update();
    }
    const click = e => {
      const t = /** @type {HTMLElement} */ (e.target), d = (/** @type {HTMLElement | null} */ (t.closest('[data-rl-kind],[data-rl-opt],[data-rl]')))?.dataset;
      if (!d) return;
      if (d.rlKind) { my.kind = d.rlKind; store('tt-rl-kind', my.kind); }
      else if (d.rlOpt) { if (my.kind === 'corr') { my.win = +d.rlOpt; store('tt-rl-win', d.rlOpt); } else { my.span = +d.rlOpt; store('tt-rl-span', d.rlOpt); } }
      else if (d.rl === 'edit') my.edit = true;
      else if (d.rl === 'cancel') my.edit = false;
      else if (d.rl === 'reset') { my.edit = false; if (my.kind === 'corr') { my.pairs = PAIRS; store('tt-rl-pairs', JSON.stringify(PAIRS)); } else { my.syms = PERF; store('tt-rl-syms', JSON.stringify(PERF)); } load(); }
      my.hover = null; update();
    };
    const submit = e => {
      const f = /** @type {HTMLFormElement} */ (e.target); if (!f.matches('[data-rl-form]')) return;
      e.preventDefault();
      const v = /** @type {HTMLInputElement} */ (f.elements.namedItem('v')).value.toUpperCase();
      if (my.kind === 'corr') {
        const pairs = v.split(',').map(p => p.trim().split(/\s+/)).filter(p => p.length === 2 && p.every(sym)).slice(0, 8);
        if (pairs.length) { my.pairs = pairs; store('tt-rl-pairs', JSON.stringify(pairs)); }
      } else {
        const syms = [...new Set(v.split(/[\s,]+/).filter(sym))].slice(0, 8);
        if (syms.length) { my.syms = syms; store('tt-rl-syms', JSON.stringify(syms)); }
      }
      my.edit = false; update(); load();
    };
    // Hover: the day under the mouse, from the plot's own scale.
    const move = e => {
      const hit = /** @type {SVGRectElement | null} */ (/** @type {Element} */ (e.target).closest?.('.mp-hit'));
      if (!hit) { if (my.hover != null && !(/** @type {Element} */ (e.target).closest?.('.mp'))) { my.hover = null; update(); } return; }
      const r = hit.getBoundingClientRect(), f = Math.max(0, Math.min(1, (e.clientX - r.left) / r.width));
      const x0 = +(hit.dataset.x0 || 0), x1 = +(hit.dataset.x1 || 0), t = x0 + f * (x1 - x0);
      if (my.hover == null || Math.abs(t - my.hover) > (x1 - x0) / 300) { my.hover = t; update(); }
    };
    const out = () => { if (my.hover != null) { my.hover = null; update(); } };
    el.addEventListener('click', click); el.addEventListener('submit', submit); el.addEventListener('mousemove', move); el.addEventListener('mouseleave', out);
    update(); load();
    return {
      update() { if (my.at && Date.now() - my.at > (my.wait || STALE)) load(); },
      dispose() { my.alive = false; el.removeEventListener('click', click); el.removeEventListener('submit', submit); el.removeEventListener('mousemove', move); el.removeEventListener('mouseleave', out); },
    };
  },
});

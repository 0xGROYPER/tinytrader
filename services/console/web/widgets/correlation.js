// Correlation: how the workspace theme's symbols move together. Pearson
// correlation of daily log returns over the last 20, 60, 120 or 250 trading
// days, from daily bars (a service with bars for any symbol, alpaca-data),
// shown as a matrix coloured blue (-1) through white (0) to red (+1). Each
// row also gives the symbol's annualised realised volatility over the window.
import { defineWidget, paint } from '../lib/widgets.js';
import { serviceFor } from '../lib/fleet.js';
import { esc } from '../lib/format.js';
import { theme } from '../lib/theme.js';
import { get } from '../lib/cache.js';

const WINDOWS = [20, 60, 120, 250];
const source = () => serviceFor('bars', 'snapshots') || null;

/** Daily closes by date for one symbol. @returns {Promise<Map<number, number>>} */
async function closes(svc, symbol, days) {
  const v = await get(`/api/bars/${encodeURIComponent(svc)}?symbol=${encodeURIComponent(symbol)}&days=${days}&interval=86400`, { fresh: 60e3 }).catch(e => { throw new Error(`${symbol}: ${e.message}`); });
  return new Map((v.bars || []).map(b => [b[0], b[4]]));
}

/** Log returns on the dates every series shares, in date order. @param {Map<number, number>[]} series */
function aligned(series) {
  const dates = [...series[0].keys()].filter(t => series.every(s => s.has(t))).sort((a, b) => a - b);
  return series.map(s => dates.slice(1).map((t, i) => Math.log(s.get(t) / s.get(dates[i]))));
}

/** @param {number[]} a @param {number[]} b */
function pearson(a, b) {
  const n = a.length, ma = a.reduce((x, y) => x + y, 0) / n, mb = b.reduce((x, y) => x + y, 0) / n;
  let sab = 0, saa = 0, sbb = 0;
  for (let i = 0; i < n; i++) { const da = a[i] - ma, db = b[i] - mb; sab += da * db; saa += da * da; sbb += db * db; }
  return saa && sbb ? sab / Math.sqrt(saa * sbb) : NaN;
}

/** Blue (-1) through white (0) to red (+1). @param {number} r */
function colour(r) {
  const t = Math.min(1, Math.abs(r)), [cr, cg, cb] = r < 0 ? [49, 92, 196] : [212, 52, 44];
  const mix = (c) => Math.round(255 + (c - 255) * t);
  return { bg: `rgb(${mix(cr)} ${mix(cg)} ${mix(cb)})`, fg: t > 0.55 ? '#fff' : '#111' };
}

function draw(my) {
  const t = theme();
  const bar = `<div class="chips">${WINDOWS.map(w => `<button type="button" class="chip-btn" data-cor-w="${w}" aria-pressed="${my.window === w}">${w}d</button>`).join('')}</div>`;
  if (!t || t.symbols.length < 2) return '<p class="muted">Needs a workspace theme with at least two symbols (right-click the workspace number, Theme…).</p>';
  if (!source()) return '<p class="muted">No service serves daily bars for any symbol (alpaca-data).</p>';
  if (my.err) return bar + `<p class="muted">${esc(my.err)}</p>`;
  if (!my.m) return bar + '<p class="muted">Loading daily bars…</p>';
  const { syms, m, vol, n } = my.m;
  const head = `<tr><th></th>${syms.map(s => `<th>${esc(s)}</th>`).join('')}<th class="cor-vol">Vol</th></tr>`;
  const rows = syms.map((s, i) => `<tr><th>${esc(s)}</th>${syms.map((_, j) => {
    const r = m[i][j], c = colour(r);
    return `<td style="background:${c.bg};color:${c.fg}" title="${esc(`${s} / ${syms[j]}: ${r.toFixed(2)}`)}">${i === j ? '' : r.toFixed(2)}</td>`;
  }).join('')}<td class="cor-vol">${(vol[i] * 100).toFixed(0)}%</td></tr>`).join('');
  return bar + `<p class="muted cor-meta">Correlation of daily returns over ${n} trading days. Vol: annualised realised volatility.</p>
    <div class="cor-wrap"><table class="cor">${head}${rows}</table></div>`;
}

defineWidget('correlation', {
  title: 'Correlation',
  mount(el) {
    const my = { window: 60, m: null, err: null, alive: true, seq: 0 };
    const update = () => { paint(el, draw(my)); };
    async function load() {
      const t = theme(), svc = source(), seq = ++my.seq;
      if (!t || t.symbols.length < 2 || !svc) return update();
      my.m = null; my.err = null; update();
      try {
        // A year of daily bars covers every window; they're shared with other panels for a minute.
        const series = await Promise.all(t.symbols.map(s => closes(svc, s, 260)));
        // Symbols with history (a new listing has little).
        const keep = t.symbols.map((_, i) => i).filter(i => series[i].size > 2);
        const ok = keep.map(i => ({ s: t.symbols[i], x: series[i] }));
        const rets = aligned(ok.map(o => o.x)).map(r => r.slice(-my.window));
        const m = rets.map(a => rets.map(b => pearson(a, b)));
        const vol = rets.map(r => { const mu = r.reduce((x, y) => x + y, 0) / r.length; return Math.sqrt(r.reduce((x, y) => x + (y - mu) ** 2, 0) / (r.length - 1) * 252); });
        if (seq === my.seq) my.m = { syms: ok.map(o => o.s), m, vol, n: rets[0]?.length || 0 };
      } catch (e) { if (seq === my.seq) my.err = `Daily bars unavailable: ${e.message}`; }
      if (my.alive && seq === my.seq) update();
    }
    const click = e => { const b = e.target.closest('[data-cor-w]'); if (b) { my.window = +b.dataset.corW; load(); } };
    el.addEventListener('click', click);
    document.addEventListener('tt-theme', load);
    load();
    return { update() {}, dispose() { my.alive = false; el.removeEventListener('click', click); document.removeEventListener('tt-theme', load); } };
  },
});

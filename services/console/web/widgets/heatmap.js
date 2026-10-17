// Sector heatmap: the screener's large companies as a treemap, after
// Finviz's map. Each sector is a block sized by its companies' market
// capitalisation and headed by its capitalisation-weighted change; inside,
// each company is a tile sized by its own, coloured by its change over the
// period picked (1D, the day; 1W and 1M, against the close 5 and 21 sessions
// back) from red through neutral to green, saturating at ±3%, ±6% or ±10%.
// Hover a tile for the company; click it to point the workspace's charts at
// it. Rows come from the service serving the options screener (alpaca-data),
// read when the map is shown (and again once it's ten minutes old); it covers
// the companies the screener scans, not every listed stock.
import { defineWidget, paint } from '../lib/widgets.js';
import { fleet } from '../lib/fleet.js';
import { esc, etTime } from '../lib/format.js';
import { theme, setFocus } from '../lib/theme.js';
import { get, stream } from '../lib/cache.js';

// Periods: the row's field and the change at which colour saturates.
const PERIODS = { '1D': ['change_pct', 0.03], '1W': ['change_5d', 0.06], '1M': ['change_21d', 0.10] };
const STALE = 10 * 60e3;
const store = (k, v) => { try { if (v === undefined) return localStorage.getItem(k); localStorage.setItem(k, v); } catch (_) { /* private mode */ } return null; };
const HEAD = 18; // a sector's header strip, in pixels
const service = () => (fleet.topo?.services || []).find(s => s.screener)?.id || null;
const cap = v => !v ? '' : v >= 1e12 ? `$${(v / 1e12).toFixed(2)}T` : v >= 1e9 ? `$${Math.round(v / 1e9)}B` : `$${Math.round(v / 1e6)}M`;
const pct = v => v == null ? '—' : `${v > 0 ? '+' : v < 0 ? '−' : ''}${Math.abs(v * 100).toFixed(2)}%`;

/**
 * Squarified treemap (Bruls, Huizing and van Wijk): items, largest first, laid
 * into rows along the shorter side, each row kept while it makes the tiles
 * squarer.
 * @template T @param {{ v: number, item: T }[]} items
 * @returns {{ item: T, x: number, y: number, w: number, h: number }[]}
 */
function squarify(items, x, y, w, h) {
  const out = [], total = items.reduce((s, i) => s + i.v, 0);
  if (!total || w <= 0 || h <= 0) return out;
  const scale = w * h / total, rest = items.map(i => ({ item: i.item, a: i.v * scale }));
  const worst = (row, side) => { const s = row.reduce((t, r) => t + r.a, 0), mx = Math.max(...row.map(r => r.a)), mn = Math.min(...row.map(r => r.a)); return Math.max(side * side * mx / (s * s), s * s / (side * side * mn)); };
  while (rest.length) {
    const side = Math.min(w, h); let row = [rest.shift()];
    while (rest.length && worst([...row, rest[0]], side) <= worst(row, side)) row.push(rest.shift());
    const s = row.reduce((t, r) => t + r.a, 0);
    if (w >= h) { const cw = s / h; let yy = y; for (const r of row) { const rh = r.a / cw; out.push({ item: r.item, x, y: yy, w: cw, h: rh }); yy += rh; } x += cw; w -= cw; }
    else { const rh = s / w; let xx = x; for (const r of row) { const rw = r.a / rh; out.push({ item: r.item, x: xx, y, w: rw, h: rh }); xx += rw; } y += rh; h -= rh; }
  }
  return out;
}

// The tile's colour: neutral at no change, red down and green up, mixed in proportion.
const tone = (c, sat) => {
  if (c == null) return 'var(--hm-none)';
  const k = Math.min(1, Math.abs(c) / sat) * 100;
  return `color-mix(in oklab, ${c >= 0 ? 'var(--hm-up)' : 'var(--hm-down)'} ${k.toFixed(0)}%, var(--hm-mid))`;
};

// The line over the map: the hovered company, else what the map covers.
function readout(my) {
  const rows = (my.rows || []).filter(r => r.market_cap > 0), h = my.hover && rows.find(r => r.symbol === my.hover);
  if (h) return `<b class="mono">${esc(h.symbol)}</b> ${esc(h.name || '')} · <span class="mono">${pct(h[PERIODS[my.period][0]])} ${my.period}</span> · ${esc(h.sector || 'Other')} · ${esc(cap(h.market_cap))}${h.price ? ` · $${h.price.toFixed(2)}` : ''}`;
  return `${rows.length} companies in ${new Set(rows.map(r => r.sector || 'Other')).size} sectors${my.pending ? ` (${my.pending} more being read)` : ''}${my.at ? ` · as of ${etTime(my.at)} ET` : ''}`;
}

function draw(my, W, H) {
  const head = '<div><h1>Sector heatmap</h1><p class="muted" style="margin:0">Large companies by sector, sized by market value and coloured by the day\'s change.</p></div>';
  if (!service()) return head + '<p class="muted">No service serves the options screener (alpaca-data).</p>';
  if (my.err) return head + `<p class="msg-err">${esc(my.err)}</p>`;
  if (!my.rows) return head + '<p class="muted">Loading…</p>';
  const [field, SAT] = PERIODS[my.period], chg = r => r[field] ?? null;
  const rows = my.rows.filter(r => r.market_cap > 0);
  const by = new Map();
  for (const r of rows) { const s = r.sector || 'Other'; (by.get(s) || by.set(s, []).get(s)).push(r); }
  const sectors = [...by].map(([name, list]) => {
    list.sort((a, b) => b.market_cap - a.market_cap);
    const v = list.reduce((t, r) => t + r.market_cap, 0), known = list.filter(r => chg(r) != null);
    const kv = known.reduce((t, r) => t + r.market_cap, 0);
    return { name, list, v, chg: kv ? known.reduce((t, r) => t + chg(r) * r.market_cap, 0) / kv : null };
  }).sort((a, b) => b.v - a.v);
  const watched = new Set(theme()?.symbols || []);
  const tiles = [];
  for (const s of squarify(sectors.map(x => ({ v: x.v, item: x })), 0, 0, W, H)) {
    const sec = s.item, hd = s.h > HEAD * 2.5 && s.w > 60 ? HEAD : 0;
    tiles.push(`<div class="hm-sec" style="left:${s.x.toFixed(1)}px;top:${s.y.toFixed(1)}px;width:${s.w.toFixed(1)}px;height:${s.h.toFixed(1)}px">${hd ? `<div class="hm-sh"><b>${esc(sec.name)}</b> <span>${pct(sec.chg)}</span></div>` : ''}</div>`);
    for (const t of squarify(sec.list.map(r => ({ v: r.market_cap, item: r })), s.x + 1, s.y + hd + 1, s.w - 2, s.h - hd - 2)) {
      const r = t.item, big = t.w > 46 && t.h > 30, mid = t.w > 30 && t.h > 16;
      const fs = Math.max(10, Math.min(22, Math.sqrt(t.w * t.h) / 5));
      tiles.push(`<button type="button" class="hm-t${watched.has(r.symbol) ? ' hm-watch' : ''}" data-sym="${esc(r.symbol)}" style="left:${t.x.toFixed(1)}px;top:${t.y.toFixed(1)}px;width:${Math.max(0, t.w - 1).toFixed(1)}px;height:${Math.max(0, t.h - 1).toFixed(1)}px;background:${tone(chg(r), SAT)};font-size:${fs.toFixed(0)}px" aria-label="${esc(`${r.symbol} ${r.name || ''} ${pct(chg(r))}`)}">${mid ? `<b>${esc(r.symbol)}</b>` : ''}${big ? `<span>${pct(chg(r))}</span>` : ''}</button>`);
    }
  }
  const lim = `${Math.round(SAT * 100)}%`, missing = rows.filter(r => chg(r) == null).length;
  return head + `<div class="hm-bar"><span class="chips hm-periods">${Object.keys(PERIODS).map(k => `<button type="button" class="chip-btn" data-hm-period="${k}" aria-pressed="${k === my.period}">${k}</button>`).join('')}</span><span class="hm-read">${readout(my)}</span><span class="hm-key" aria-hidden="true"><span>−${lim}</span><i style="background:${tone(-SAT, SAT)}"></i><i style="background:${tone(-SAT / 3, SAT)}"></i><i style="background:${tone(0, SAT)}"></i><i style="background:${tone(SAT / 3, SAT)}"></i><i style="background:${tone(SAT, SAT)}"></i><span>+${lim}</span></span></div>
    ${missing && my.period !== '1D' ? `<p class="muted hm-note">${missing} of ${rows.length} companies have no ${my.period} change yet (the screener is still reading their daily bars); they're grey. Showing it again later fills them in.</p>` : ''}
    <div class="hm-map" style="height:${H}px">${tiles.join('')}</div>`;
}

defineWidget('heatmap', {
  title: 'Sector heatmap',
  mount(el) {
    const saved = store('tt-hm-period');
    const my = { rows: /** @type {any[] | null} */ (null), err: '', at: 0, tried: 0, pending: 0, hover: /** @type {string | null} */ (null), alive: true, period: saved && PERIODS[saved] ? saved : '1D', loading: false };
    const size = () => {
      const W = Math.max(240, el.clientWidth - 32);
      // The map takes what the panel has below its header, at least a third as tall as wide.
      const H = Math.max(Math.round(W / 3), el.clientHeight - 110, 220);
      return [W, Math.min(H, 2000)];
    };
    const update = () => { const [W, H] = size(); paint(el, draw(my, W, H)); };
    async function load() {
      const svc = service(); if (!svc) { update(); return; }
      if (my.loading) return; my.loading = true; my.tried = Date.now();
      try {
        const v = await get(stream(svc, 'screener'), { fresh: 10e3 });
        if (!my.alive) return;
        my.rows = v.rows || []; my.at = Date.now(); my.err = ''; my.pending = v.pending || 0;
      } catch (e) { my.err = `The screener couldn't be read: ${e.message}`; }
      my.loading = false; update();
    }
    const over = e => {
      const s = /** @type {HTMLElement | null} */ (e.target.closest?.('[data-sym]'))?.dataset.sym || null;
      if (s !== my.hover) { my.hover = s; const read = el.querySelector('.hm-read'); if (read) read.innerHTML = readout(my); }
    };
    const click = e => {
      const p = /** @type {HTMLElement | null} */ (e.target.closest?.('[data-hm-period]'))?.dataset.hmPeriod;
      if (p && PERIODS[p]) { my.period = p; store('tt-hm-period', p); update(); return; }
      const s = /** @type {HTMLElement | null} */ (e.target.closest?.('[data-sym]'))?.dataset.sym;
      if (s) setFocus(s);
    };
    el.addEventListener('mouseover', over); el.addEventListener('click', click);
    const resized = new ResizeObserver(() => update()); resized.observe(el);
    load();
    return {
      // Shown again (or refreshed while shown): read again once ten minutes
      // old; every minute while the screener is still filling in (after a
      // restart); and, until a read has worked (the service down, or not yet
      // known), every 15 seconds.
      update() {
        const now = Date.now();
        if (!my.at) { if (now - my.tried > 15e3) load(); }
        else if (now - my.at > (my.pending ? 60e3 : STALE)) load();
      },
      dispose() { my.alive = false; resized.disconnect(); el.removeEventListener('mouseover', over); el.removeEventListener('click', click); },
    };
  },
});

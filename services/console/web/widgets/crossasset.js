// Cross-asset: a board of what moved, beyond US stocks: rates (Treasury
// yields from Cboe, in basis points, and bond ETFs), the dollar and FX,
// commodities, volatility (VIX and its terms, with the term structure's
// shape), world markets, crypto and US indexes, as ETF proxies. Each row:
// the level, the change over a day, a week and a month, whether it's above
// (▲) or below (▼) its 20-, 50- and 200-day averages, and where it sits in
// its 52-week range. The rows are edited per group and kept in this browser.
// Daily closes (lib/trend.js) are read when the board is shown, and again
// once ten minutes old. Clicking a row points the workspace's charts at it.
import { defineWidget, paint } from '../lib/widgets.js';
import { fleet } from '../lib/fleet.js';
import { esc } from '../lib/format.js';
import { setFocus } from '../lib/theme.js';
import { board, setBoard, closes, analyse, label, fmtChange, fmtLevel, isCrypto } from '../lib/trend.js';

const STALE = 10 * 60e3;
const tone = v => v == null ? '' : v > 0 ? 'pos' : v < 0 ? 'neg' : '';
// Symbols for the board's editor: stock symbols, crypto pairs (BTC/USD) and Cboe's indexes.
const parseRows = text => [...new Set(text.toUpperCase().split(/[\s,]+/).filter(s => /^[A-Z0-9.]{1,10}$/.test(s) || /^[A-Z]{2,6}\/[A-Z]{2,6}$/.test(s)))].slice(0, 30);

function row(a, s) {
  if (!a) return `<tr><td class="ca-name">${esc(label(s))}<span class="mono">${esc(s)}</span></td><td colspan="6" class="muted">no data</td></tr>`;
  const arrows = [20, 50, 200].map(k => { const x = a.avg[k]; return x ? `<i class="${x.above ? 'up' : 'down'}" title="${x.above ? 'Above' : 'Below'} its ${k}-day average (${fmtLevel({ ...a, last: x.v, value: a.yield ? x.v / 10 : x.v })}) for ${x.run} session${x.run === 1 ? '' : 's'}${x.since ? `, since ${x.since}` : ''}">${x.above ? '▲' : '▼'}</i>` : '<i class="na">·</i>'; }).join('');
  const span = a.hi52 - a.lo52, pos = span > 0 ? (a.last - a.lo52) / span : 0.5;
  const range = `<span class="ca-range" title="52-week range ${fmtLevel({ ...a, last: a.lo52, value: a.yield ? a.lo52 / 10 : a.lo52 })} – ${fmtLevel({ ...a, last: a.hi52, value: a.yield ? a.hi52 / 10 : a.hi52 })}"><b style="left:${(pos * 100).toFixed(0)}%"></b></span>`;
  const flags = [a.newHi52 ? '52w high' : a.newHi20 ? '20d high' : '', a.newLo52 ? '52w low' : a.newLo20 ? '20d low' : ''].filter(Boolean).join(' ');
  return `<tr data-sym="${esc(s)}"><td class="ca-name">${esc(label(s))}<span class="mono">${esc(s)}</span>${flags ? `<em>${esc(flags)}</em>` : ''}</td>
    <td class="num mono">${fmtLevel(a)}</td><td class="num mono ${tone(a.d1)}">${fmtChange(a.d1, a.yield)}</td><td class="num mono ${tone(a.w1)}">${fmtChange(a.w1, a.yield)}</td><td class="num mono ${tone(a.m1)}">${fmtChange(a.m1, a.yield)}</td>
    <td class="ca-trend" aria-label="Against the 20-, 50- and 200-day averages">${arrows}</td><td class="ca-r">${range}</td></tr>`;
}

// The VIX term structure's shape, from whichever of its terms are on the board.
function term(by) {
  const v9 = by.get('VIX9D'), v = by.get('VIX'), v3 = by.get('VIX3M');
  if (!v || (!v9 && !v3)) return '';
  const parts = [v9 && `9-day ${v9.last.toFixed(2)}`, `30-day ${v.last.toFixed(2)}`, v3 && `3-month ${v3.last.toFixed(2)}`].filter(Boolean).join(' · ');
  const inverted = (v9 && v9.last > v.last) || (v3 && v.last > v3.last);
  return `<p class="ca-note">${parts}: <b class="${inverted ? 'neg' : ''}">${inverted ? 'inverted (the near term priced above the far: stress)' : 'upward sloping (calm)'}</b></p>`;
}

function draw(my) {
  const groups = board();
  if (my.edit) {
    return `<form class="ca-edit" data-ca-form>${groups.map((g, i) => `<label><span>${esc(g.name)}</span><input name="g${i}" class="wl-in" value="${esc(g.rows.map(r => r.s).join(', '))}" spellcheck="false" autocomplete="off"></label>`).join('')}
      <p class="muted" style="margin:0;font-size:12px">Stock and ETF symbols, crypto pairs (BTC/USD), and Cboe's indexes: VIX, VIX9D, VIX3M, and the Treasury yields IRX (13-week), FVX, TNX and TYX.</p>
      <div class="wl-bar"><button class="btn small primary">Save</button><button type="button" class="btn small" data-ca="cancel">Cancel</button><button type="button" class="btn small" data-ca="reset">Back to the starting board</button></div></form>`;
  }
  if (my.err && !my.series) return `<p class="msg-err">${esc(my.err)}</p>`;
  if (!my.series) return '<p class="muted">Loading the board…</p>';
  const by = new Map(), dates = new Set();
  for (const g of groups) for (const r of g.rows) { const k = my.series.get(r.s), a = k && analyse(r.s, k.closes); by.set(r.s, a); if (a && !isCrypto(r.s)) dates.add(a.date); }
  const body = groups.map(g => `<tbody><tr class="ca-g"><th colspan="7">${esc(g.name)}</th></tr>${g.rows.map(r => row(by.get(r.s), r.s)).join('')}${g.rows.some(r => r.s.startsWith('VIX')) ? `<tr><td colspan="7">${term(by)}</td></tr>` : ''}</tbody>`).join('');
  const asOf = [...dates].sort().pop();
  return `<div class="ca-head"><span class="muted">${asOf ? `Closes as of ${esc(asOf)} (crypto to today), up to 16 minutes late` : ''}${my.err ? ` · ${esc(my.err)}` : ''}</span><button type="button" class="btn small" data-ca="edit">Edit rows</button></div>
    <div class="scroll"><table class="ca"><thead><tr><th></th><th class="num">Last</th><th class="num">1D</th><th class="num">1W</th><th class="num">1M</th><th title="Against the 20-, 50- and 200-day averages">20 50 200</th><th title="Where today’s close sits in its 52-week range: the low at the left end, the high at the right; the tick is today. Hover a bar for the low and high.">52 weeks</th></tr></thead>${body}</table></div>`;
}

defineWidget('crossasset', {
  title: 'Cross-asset',
  mount(el) {
    const my = { series: /** @type {Map<string, any> | null} */ (null), err: '', at: 0, edit: false, alive: true };
    const update = () => { paint(el, draw(my)); };
    async function load() {
      const syms = board().flatMap(g => g.rows.map(r => r.s));
      const r = await closes(fleet, syms);
      if (!my.alive) return;
      my.series = r.series; my.err = r.error; my.at = Date.now(); update();
    }
    const click = e => {
      const t = /** @type {HTMLElement} */ (e.target);
      const act = /** @type {HTMLElement | null} */ (t.closest('[data-ca]'))?.dataset.ca;
      if (act === 'edit') { my.edit = true; update(); return; }
      if (act === 'cancel') { my.edit = false; update(); return; }
      if (act === 'reset') { my.edit = false; setBoard(null); return; }
      const s = /** @type {HTMLElement | null} */ (t.closest('[data-sym]'))?.dataset.sym;
      if (s && !s.includes('/') && !/^(VIX|IRX|FVX|TNX|TYX)/.test(s)) { setFocus(s); document.dispatchEvent(new CustomEvent('tt-focus', { detail: s })); }
    };
    const submit = e => {
      const f = /** @type {HTMLFormElement} */ (e.target); if (!f.matches('[data-ca-form]')) return;
      e.preventDefault();
      const groups = board().map((g, i) => ({ name: g.name, rows: parseRows(/** @type {HTMLInputElement} */ (f.elements.namedItem(`g${i}`)).value).map(s => ({ s })) }));
      my.edit = false; setBoard(groups);
    };
    const changed = () => { update(); load(); };
    el.addEventListener('click', click); el.addEventListener('submit', submit);
    document.addEventListener('tt-board', changed);
    update(); load();
    return {
      update() { if (my.at && Date.now() - my.at > STALE) load(); },
      dispose() { my.alive = false; el.removeEventListener('click', click); el.removeEventListener('submit', submit); document.removeEventListener('tt-board', changed); },
    };
  },
});

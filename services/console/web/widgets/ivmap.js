// IV heatmap: an underlying's implied volatility by strike (rows) and expiry
// (columns), from a service serving option chains (alpaca-data). Each cell is
// the out-of-the-money contract's IV (puts below spot, calls above), coloured
// on the usual heatmap scale, blue (low) through green and yellow to red (high); the first rows are the term structure:
// at-the-money IV and the straddle's implied move per expiry. The
// underlying is the one picked in a watchlist, else the workspace theme's
// first symbol, else one typed in; refreshed every 30 seconds.
import { defineWidget } from '../lib/widgets.js';
import { serviceFor } from '../lib/fleet.js';
import { esc } from '../lib/format.js';
import { theme, focused } from '../lib/theme.js';
import { get, stream } from '../lib/cache.js';
import { onChange } from '../lib/channels.js';

const EVERY = 30_000;
/** Strikes shown either side of spot. */
const SIDE = 14;

const source = () => serviceFor('chain') || null;
const pct = x => x == null ? '' : `${(x * 100).toFixed(1)}`;
// The heatmap scale: blue, cyan, green, yellow, orange, red.
const STOPS = [[49, 92, 196], [41, 166, 214], [72, 183, 116], [240, 210, 70], [242, 140, 48], [212, 52, 44]];
/** A colour for f in [0, 1], and whether dark text reads on it. @param {number} f */
function heat(f) {
  const x = Math.min(1, Math.max(0, f)) * (STOPS.length - 1), i = Math.min(STOPS.length - 2, Math.floor(x)), t = x - i;
  const [r, g, b] = STOPS[i].map((v, k) => Math.round(v + (STOPS[i + 1][k] - v) * t));
  return { rgb: `rgb(${r} ${g} ${b})`, dark: 0.299 * r + 0.587 * g + 0.114 * b > 150 };
}
const short = d => new Date(d + 'T12:00:00Z').toLocaleDateString(undefined, { timeZone: 'UTC', month: 'short', day: 'numeric' });

/** @param {any} c the chain answer */
function grid(c) {
  const spot = c.spot;
  const expiries = c.expiries.map(e => e.expiry);
  // Strikes nearest spot, SIDE each way, from those listed for any expiry.
  const all = [...new Set(c.contracts.map(x => x.strike))].sort((a, b) => a - b);
  const at = spot == null ? Math.floor(all.length / 2) : all.reduce((best, k, i) => Math.abs(k - spot) < Math.abs(all[best] - spot) ? i : best, 0);
  const strikes = all.slice(Math.max(0, at - SIDE), at + SIDE + 1).reverse();
  const cell = new Map();
  for (const x of c.contracts) {
    const otm = spot == null ? x.type === 'call' : (x.strike >= spot) === (x.type === 'call');
    if (otm && x.iv != null) cell.set(`${x.expiry}|${x.strike}`, x);
  }
  const ivs = [...cell.values()].filter(x => strikes.includes(x.strike)).map(x => x.iv).sort((a, b) => a - b);
  // Shade between the 5th and 95th percentiles, so one far strike doesn't wash out the rest.
  const lo = ivs[Math.floor(ivs.length * 0.05)] ?? 0, hi = ivs[Math.ceil(ivs.length * 0.95) - 1] ?? 1;
  const shade = iv => heat((iv - lo) / Math.max(1e-9, hi - lo));
  const head = `<tr><th class="iv-k">Strike</th>${expiries.map(e => `<th>${esc(short(e))}</th>`).join('')}</tr>`;
  const term = `<tr class="iv-term"><th class="iv-k">ATM IV</th>${c.expiries.map(e => `<td>${esc(pct(e.atm_iv))}</td>`).join('')}</tr>
    <tr class="iv-term"><th class="iv-k">Move ±</th>${c.expiries.map(e => `<td>${e.implied_move == null ? '' : esc(`${(e.implied_move * 100).toFixed(1)}%`)}</td>`).join('')}</tr>`;
  const rows = strikes.map(k => {
    const atm = spot != null && strikes.reduce((b, s) => Math.abs(s - spot) < Math.abs(b - spot) ? s : b) === k;
    return `<tr${atm ? ' class="iv-atm"' : ''}><th class="iv-k">${esc(String(k))}</th>${expiries.map(e => {
      const x = cell.get(`${e}|${k}`);
      if (!x) return '<td class="iv-none"></td>';
      const tip = `${x.symbol}\nIV ${pct(x.iv)}%  delta ${x.delta?.toFixed(2) ?? '?'}\nbid ${x.bid ?? '?'}  ask ${x.ask ?? '?'}`;
      const h = shade(x.iv);
      return `<td style="background:${h.rgb};color:${h.dark ? '#111' : '#fff'}" title="${esc(tip)}">${esc(pct(x.iv))}</td>`;
    }).join('')}</tr>`;
  }).join('');
  const legend = `<span class="iv-legend">${esc(pct(lo))}% <i style="background:linear-gradient(90deg, ${STOPS.map(c => `rgb(${c.join(' ')})`).join(', ')})"></i> ${esc(pct(hi))}%</span>`;
  return { table: `<table class="ivm"><thead>${head}</thead><tbody>${term}${rows}</tbody></table>`, legend };
}

function draw(my) {
  const svc = source();
  const t = theme(), picks = [...new Set([...(t?.symbols || []), my.symbol].filter(Boolean))].slice(0, 16);
  const bar = `<form class="iv-bar"><input class="input iv-input" name="sym" value="${esc(my.symbol || '')}" placeholder="Symbol" aria-label="Underlying" autocomplete="off" spellcheck="false">
    <span class="chips">${picks.map(s => `<button type="button" class="chip-btn" data-iv-pick="${esc(s)}" aria-pressed="${s === my.symbol}">${esc(s)}</button>`).join('')}</span></form>`;
  if (!svc) return bar + '<p class="muted">No service serves option chains (alpaca-data).</p>';
  if (!my.symbol) return bar + '<p class="muted">Pick or type an underlying.</p>';
  if (my.err) return bar + `<p class="muted">${esc(my.err)}</p>`;
  if (!my.chain) return bar + '<p class="muted">Loading the chain…</p>';
  if (!my.chain.contracts.length) return bar + `<p class="muted">No options found for ${esc(my.symbol)}.</p>`;
  const g = grid(my.chain);
  return bar + `<p class="muted iv-meta">${esc(my.symbol)} ${my.chain.spot ?? ''} · IV of out-of-the-money contracts, % · ${esc(my.chain.feed)} quotes · ${g.legend}</p><div class="iv-wrap">${g.table}</div>`;
}

defineWidget('ivmap', {
  title: 'IV heatmap',
  mount(el) {
    const my = { symbol: focused() || theme()?.symbols[0] || null, chain: null, err: null, alive: true };
    const update = () => {
      const typing = el.querySelector('.iv-input') === document.activeElement;
      if (typing) return; // don't redraw under the reader's cursor
      const wrap = el.querySelector('.iv-wrap'), top = wrap?.scrollTop ?? 0, left = wrap?.scrollLeft ?? 0;
      el.innerHTML = draw(my);
      const w2 = el.querySelector('.iv-wrap'); if (w2) { w2.scrollTop = top; w2.scrollLeft = left; }
    };
    async function load() {
      const svc = source(), sym = my.symbol;
      if (!my.alive || !svc || !sym) return;
      try {
        const v = await get(stream(svc, 'chain', { underlying: sym }), { fresh: 10e3 });
        if (sym !== my.symbol) return;
        my.chain = v; my.err = null;
      } catch (e) { if (sym === my.symbol) my.err = `Chain unavailable: ${e.message}`; }
      if (my.alive) update();
    }
    const pick = sym => { my.symbol = sym; my.chain = null; my.err = null; update(); load(); };
    const click = e => { const b = e.target.closest('[data-iv-pick]'); if (b) pick(b.dataset.ivPick); };
    const submit = e => {
      e.preventDefault();
      const v = String(new FormData(e.target).get('sym') || '').trim().toUpperCase();
      if (/^[A-Z0-9.]{1,10}$/.test(v)) { e.target.querySelector('input')?.blur(); pick(v); }
    };
    el.addEventListener('click', click);
    el.addEventListener('submit', submit);
    onChange('focus', s => { if (typeof s === 'string') pick(s); });
    // The chain service comes with the topology: a chain that wasn't there is read once it is.
    onChange('topology', () => { if (!my.chain) load(); });
    const timer = setInterval(load, EVERY);
    update(); load();
    return {
      update() {},
      dispose() { my.alive = false; clearInterval(timer); el.removeEventListener('click', click); el.removeEventListener('submit', submit); },
    };
  },
});

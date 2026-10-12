// Candles: candlesticks and volume for a symbol, from a service with bars for
// any symbol (alpaca-data: consolidated history with pre-market and after-
// hours, 04:00-20:00 New York, and its latest minutes), or from a relay's own
// bars where no such service runs (a relay has only its session hours).
// Pre-market and after-hours candles are drawn faded. Symbols: the theme's,
// the relays', or any typed into the ticker box.
// Times are New York time. Drawn with TradingView's Lightweight Charts
// (Apache-2.0), loaded the first time a chart opens.
import { defineWidget } from '../lib/widgets.js';
import { fleet } from '../lib/fleet.js';
import { esc } from '../lib/format.js';
import { theme, focused } from '../lib/theme.js';

const LIB = '/vendor/lightweight-charts-5.2.1.min.js';
/** @type {Promise<any> | null} */
let loading = null;
function library() {
  loading ||= new Promise((resolve, reject) => {
    const s = document.createElement('script');
    s.src = LIB; s.onload = () => resolve(globalThis.LightweightCharts); s.onerror = () => { loading = null; reject(new Error(`${LIB} did not load`)); };
    document.head.appendChild(s);
  });
  return loading;
}

/** Every symbol some relay serves bars for: [{ service, symbol }]. */
function relayed() {
  return (fleet.topo?.services || []).filter(s => s.bars).flatMap(s => {
    const symbols = [...new Set((s.schedule || []).flatMap(x => x.symbols || []))];
    return symbols.map(symbol => ({ service: s.id, symbol }));
  });
}

/** What to offer: the workspace theme's symbols when it has one (from a relay
 * that carries the symbol, else a service with bars for any symbol), or else
 * every relayed symbol. */
function choices() {
  const t = theme(), live = relayed(), f = focused();
  const any = (fleet.topo?.services || []).find(s => s.bars && s.snapshots)?.id;
  // Without a theme, the relays' symbols, and the one picked elsewhere (an earnings report).
  if (!t?.symbols.length) return f && any && !live.some(c => c.symbol === f) ? [...live, { service: any, symbol: f }] : live;
  return t.symbols.map(symbol => live.find(c => c.symbol === symbol) || (any ? { service: any, symbol } : null)).filter(Boolean);
}

// Lightweight Charts shows times as UTC; shift each bar by New York's offset
// on its day so the axis reads in New York time.
const offsets = new Map();
function nyOffset(t) {
  const day = Math.floor(t / 86400);
  if (!offsets.has(day)) {
    const d = new Date(t * 1000);
    const ny = new Date(d.toLocaleString('en-US', { timeZone: 'America/New_York' }));
    const utc = new Date(d.toLocaleString('en-US', { timeZone: 'UTC' }));
    offsets.set(day, Math.round((ny.getTime() - utc.getTime()) / 1000));
  }
  return offsets.get(day);
}

const css = name => getComputedStyle(document.documentElement).getPropertyValue(name).trim();
// A token's colour, faded for volume (tokens are #rrggbb).
const faded = c => /^#[0-9a-f]{6}$/i.test(c) ? c + '66' : c;
// Ranges (in trading days) and the bar size each starts with, as on TradingView.
/** @type {[string, number, number][]} */
const RANGES = [['1D', 1, 60], ['5D', 5, 900], ['1M', 21, 3600], ['6M', 126, 86400], ['1Y', 252, 86400], ['5Y', 1260, 86400]];
// Bar sizes, and the longest range (trading days) each is offered for.
/** @type {[string, number, number][]} */
const SIZES = [['1m', 60, 21], ['5m', 300, 42], ['15m', 900, 84], ['1h', 3600, 504], ['1D', 86400, 1260]];
/** Minute bars combined into bars of `size` seconds (aligned to New York time). @param {number[][]} bars @param {number} size */
function combine(bars, size) {
  /** @type {number[][]} */
  const out = [];
  for (const [t, o, h, l, c, v] of bars) {
    const k = t - ((t + nyOffset(t)) % size + size) % size, last = out[out.length - 1];
    if (last && last[0] === k) { last[2] = Math.max(last[2], h); last[3] = Math.min(last[3], l); last[4] = c; last[5] += v; }
    else out.push([k, o, h, l, c, v]);
  }
  return out;
}

// The implied move over the chart's own range: how far the options market
// expects the price to move over the next 1, 5, 21, 126, 252 or 1260 trading
// days. Each expiry's at-the-money straddle over the price (from a service
// serving option chains, alpaca-data) gives the expected move to that date;
// as total variance, (move / 0.798)^2, it is interpolated linearly in time
// between the expiries either side of the horizon (from zero before the
// first, and at the last expiry's rate beyond it), so an earnings report
// inside the horizon is included through the expiries after it. A second pair
// of lines marks the first expiry after the company's next report.
const chainService = () => (fleet.topo?.services || []).find(s => s.chain)?.id || null;
const nextReport = sym => {
  const e = Object.values(fleet.snap?.services || {}).map(s => s?.earnings).find(x => Array.isArray(x?.reports));
  const today = new Date().toISOString().slice(0, 10);
  return (e?.reports || []).filter(r => r.symbol === sym && r.date >= today).sort((a, b) => a.date.localeCompare(b.date))[0] || null;
};
const K = Math.sqrt(2 / Math.PI); // a straddle's value over sigma * sqrt(T): 0.798
/** Calendar days from now to an expiry's 16:00 New York close. */
const daysTo = d => (Date.parse(`${d}T20:00:00Z`) - Date.now()) / 86400e3;
async function impliedMoves(sym, tradingDays) {
  const svc = chainService();
  if (!svc) return null;
  const horizon = tradingDays * 365 / 252; // calendar days
  const to = new Date(Date.now() + Math.min(Math.max(horizon + 30, 60), 800) * 86400e3).toISOString().slice(0, 10);
  const r = await fetch(`/api/stream/${encodeURIComponent(svc)}/chain?underlying=${encodeURIComponent(sym)}&to=${to}`, { cache: 'no-store' });
  if (!r.ok) return null;
  const v = await r.json().catch(() => null);
  const exps = (v?.expiries || []).filter(x => x.implied_move > 0 && daysTo(x.expiry) > 0.05)
    .map(x => ({ ...x, t: daysTo(x.expiry), w: (x.implied_move / K) ** 2 })).sort((a, b) => a.t - b.t);
  if (!v?.spot || !exps.length) return null;
  // Total variance at the horizon.
  let w, beyond = false;
  const after = exps.findIndex(x => x.t >= horizon);
  if (after === 0) w = exps[0].w * horizon / exps[0].t;
  else if (after > 0) { const a = exps[after - 1], b = exps[after]; w = a.w + (b.w - a.w) * (horizon - a.t) / (b.t - a.t); }
  else { const last = exps[exps.length - 1]; w = last.w * horizon / last.t; beyond = true; }
  const moves = [{ kind: 'range', implied_move: K * Math.sqrt(Math.max(w, 0)), days: tradingDays, beyond }];
  // 0DTE: the straddle expiring today prices the move from now to today's close.
  const todayNY = new Date().toLocaleDateString('en-CA', { timeZone: 'America/New_York' });
  const zero = exps.find(x => x.expiry === todayNY);
  if (zero) moves.push({ ...zero, kind: '0dte' });
  const rep = nextReport(sym);
  if (rep) {
    const e = exps.find(x => rep.when === 'after-close' ? x.expiry > rep.date : x.expiry >= rep.date);
    if (e) moves.push({ ...e, kind: 'earnings' });
  }
  return { spot: v.spot, moves, report: rep };
}

defineWidget('candles', {
  title: 'Candles',
  mount(el) {
    const my = { typed: [],
      pick: null, days: 1, size: 60, err: null, note: null, last: '', timer: 0, chart: null, candles: null, volume: null, alive: true, fit: true,
      im: true, imLines: [], imFor: '', imAt: 0, imText: '' };
    el.innerHTML = '<div class="cd"><div class="cd-top"><form class="cd-search"><input class="cd-ticker mono" name="ticker" placeholder="Ticker" maxlength="10" autocomplete="off" spellcheck="false" aria-label="Show a ticker"></form><div class="cd-bar"></div></div><div class="cd-chart"></div><p class="cd-msg muted"></p></div>';
    const bar = el.querySelector('.cd-bar'), box = /** @type {HTMLElement} */ (el.querySelector('.cd-chart')), msg = el.querySelector('.cd-msg');
    const anyService = () => (fleet.topo?.services || []).find(s => s.bars && s.snapshots)?.id;
    const offered = () => {
      const list = choices(), any = anyService();
      for (const symbol of my.typed) if (any && !list.some(c => c.symbol === symbol)) list.push({ service: any, symbol });
      return list;
    };
    const head = () => {
      const list = offered();
      if (!my.pick || !list.some(c => c.service === my.pick.service && c.symbol === my.pick.symbol)) my.pick = list.find(c => c.symbol === focused()) || list[0] || null;
      bar.innerHTML = list.length
        ? `<span class="chips">${list.map(c => `<button type="button" class="chip-btn" data-cd-pick="${esc(c.service)}|${esc(c.symbol)}" aria-pressed="${my.pick?.service === c.service && my.pick?.symbol === c.symbol}">${esc(c.symbol)}</button>`).join('')}</span>
           <span class="chips">${RANGES.map(([l, d]) => `<button type="button" class="chip-btn" data-cd-days="${d}" aria-pressed="${my.days === d}">${l}</button>`).join('')}</span>
           <span class="chips">${SIZES.filter(([, , most]) => my.days <= most).map(([l, z]) => `<button type="button" class="chip-btn" data-cd-size="${z}" aria-pressed="${my.size === z}">${l}</button>`).join('')}</span>
           <span class="chips"><button type="button" class="chip-btn" data-cd-im aria-pressed="${my.im}" title="Implied move from the option chain: over the chart’s range, to today’s close (0DTE), and to the first expiry after an earnings report">IM</button></span>
           <span class="cd-last mono">${esc(my.last)}${my.im && my.imText ? ` · <span class="cd-im">${esc(my.imText)}</span>` : ''}</span>`
        : '';
      msg.textContent = !list.length ? (theme() ? 'No service serves bars for the theme’s symbols.' : 'No relay serves bars yet.') : my.err ? my.err : my.note || '';
    };
    const colours = () => ({
      layout: { background: { color: 'transparent' }, textColor: css('--muted'), fontFamily: css('--mono') || 'monospace', fontSize: 11 },
      grid: { vertLines: { color: css('--line-soft') }, horzLines: { color: css('--line-soft') } },
      rightPriceScale: { borderColor: css('--line') }, timeScale: { borderColor: css('--line'), timeVisible: true, secondsVisible: false },
    });
    async function load() {
      if (!my.pick || !my.alive) return;
      const { symbol } = my.pick, days = my.days, size = my.size;
      // Minutes come from the pick's source (a relay's are live); longer bars
      // from a service with bars for any symbol, else combined from minutes.
      // History from a service with bars for any symbol (with extended hours);
      // a relay's own bars only when there is none.
      const any = anyService();
      const service = any || my.pick.service, ask = service === any ? size : 60;
      try {
        if (ask === 60 && size >= 86400) throw new Error('Daily bars need a service with history for any symbol (alpaca-data).');
        const r = await fetch(`/api/bars/${encodeURIComponent(service)}?symbol=${encodeURIComponent(symbol)}&days=${days}&interval=${ask}`, { cache: 'no-store' });
        const v = await r.json().catch(() => ({}));
        if (!r.ok) throw new Error(v.error || `${r.status} ${r.statusText}`);
        if (!my.alive || my.pick?.symbol !== symbol || my.days !== days || my.size !== size) return;
        if (ask < size) v.bars = combine(v.bars || [], size);
        const L = await library();
        if (!my.chart) {
          my.chart = L.createChart(box, { autoSize: true, ...colours() });
          my.chart.timeScale().applyOptions({ timeVisible: size < 86400 });
          my.candles = my.chart.addSeries(L.CandlestickSeries, { upColor: css('--ok'), downColor: css('--crit'), wickUpColor: css('--ok'), wickDownColor: css('--crit'), borderVisible: false });
          my.volume = my.chart.addSeries(L.HistogramSeries, { priceScaleId: 'volume', priceFormat: { type: 'volume' }, color: css('--line') });
          my.chart.priceScale('volume').applyOptions({ scaleMargins: { top: 0.8, bottom: 0 } });
        } else my.chart.applyOptions(colours());
        const bars = v.bars || [];
        // Outside 09:30-16:00 New York (intraday bars only), faded.
        const extended = t => { if (size >= 86400) return false; const m = Math.floor(((t + nyOffset(t)) % 86400 + 86400) % 86400 / 60); return m < 570 || m >= 960; };
        my.candles.setData(bars.map(([t, o, h, l, c]) => {
          const bar = { time: t + nyOffset(t), open: o, high: h, low: l, close: c };
          if (!extended(t)) return bar;
          const col = faded(c >= o ? css('--ok') : css('--crit'));
          return { ...bar, color: col, wickColor: col, borderColor: col };
        }));
        my.volume.setData(bars.map(([t, o, , , c, vol]) => ({ time: t + nyOffset(t), value: vol, color: faded(c >= o ? css('--ok') : css('--crit')) })));
        // Show the whole range when one is picked; later refreshes keep the reader's zoom.
        if (my.fit) { my.chart.timeScale().fitContent(); my.fit = false; }
        const lastBar = bars[bars.length - 1];
        my.last = !lastBar ? 'no trades yet' : size >= 86400
          ? `${lastBar[4].toFixed(2)} · ${new Date((lastBar[0] + 43200) * 1000).toLocaleDateString(undefined, { timeZone: 'UTC', month: 'short', day: 'numeric' })}`
          : `${lastBar[4].toFixed(2)} · ${new Date(lastBar[0] * 1000).toLocaleTimeString('en-GB', { timeZone: 'America/New_York', hour: '2-digit', minute: '2-digit' })} ET`;
        my.err = null; my.note = v.note || (bars.length ? '' : `No bars for ${symbol} in this range yet.`);
        await drawMoves(L, symbol);
      } catch (e) { my.err = e.message; }
      head();
    }
    // Implied-move lines: refetched when the symbol changes and every 5 minutes.
    async function drawMoves(L, symbol) {
      const clear = () => { for (const l of my.imLines) my.candles?.removePriceLine(l); my.imLines = []; };
      if (!my.im) { clear(); my.imText = ''; return; }
      const key = `${symbol}|${my.days}`;
      if (my.imFor === key && Date.now() - my.imAt < 300e3 && my.imLines.length) return;
      let im = null;
      try { im = await impliedMoves(symbol, my.days); } catch { im = null; }
      if (!my.alive || my.pick?.symbol !== symbol || !my.candles) return;
      clear();
      my.imFor = key; my.imAt = Date.now();
      if (!im) { my.imText = ''; return; }
      const day = d => new Date(d + 'T12:00:00Z').toLocaleDateString(undefined, { timeZone: 'UTC', month: 'short', day: 'numeric' });
      const span = n => n === 1 ? 'next day' : n === 5 ? 'next 5 days' : n === 21 ? 'next month' : n === 126 ? 'next 6 months' : n === 252 ? 'next year' : `next ${Math.round(n / 252)} years`;
      const pct = m => `±${(m.implied_move * 100).toFixed(m.implied_move < 0.01 ? 2 : 1)}%`;
      const text = m => m.kind === 'earnings' ? `earnings ${day(im.report.date)}: ${pct(m)} to ${day(m.expiry)}`
        : m.kind === '0dte' ? `0DTE ${pct(m)} to today's close`
        : `${pct(m)} ${span(m.days)}${m.beyond ? ' (beyond the listed expiries)' : ''}`;
      for (const m of im.moves) {
        const color = m.kind === 'earnings' ? css('--warn') || '#d98a1c' : m.kind === '0dte' ? css('--ok') || '#2f9e6b' : css('--accent') || '#5b6fd6';
        for (const sign of [1, -1]) {
          my.imLines.push(my.candles.createPriceLine({
            price: im.spot * (1 + sign * m.implied_move), color, lineWidth: m.kind === 'range' ? 2 : 1, lineStyle: L.LineStyle.Dashed,
            axisLabelVisible: true, title: sign > 0 ? text(m) : '',
          }));
        }
      }
      my.imText = im.moves.map(text).join(' · ');
    }
    const fresh = () => { my.chart?.remove(); my.chart = my.candles = my.volume = null; my.imLines = []; my.imFor = ''; my.last = ''; my.fit = true; head(); load(); };
    // The ticker box: any symbol, from the service with bars for any symbol.
    el.addEventListener('submit', e => {
      e.preventDefault();
      const input = /** @type {HTMLInputElement} */ (e.target.closest('form')?.querySelector('.cd-ticker'));
      const symbol = (input?.value || '').trim().toUpperCase();
      const any = anyService();
      if (!/^[A-Z][A-Z0-9.]{0,9}$/.test(symbol)) return;
      if (!any) { my.err = 'Typing a ticker needs a service with bars for any symbol (alpaca-data).'; head(); return; }
      if (!my.typed.includes(symbol)) my.typed.push(symbol);
      my.pick = offered().find(c => c.symbol === symbol) || { service: any, symbol };
      fresh();
    });
    const click = e => {
      const p = e.target.closest('[data-cd-pick]');
      if (p) { const [service, symbol] = p.dataset.cdPick.split('|'); my.pick = { service, symbol }; fresh(); return; }
      const d = e.target.closest('[data-cd-days]');
      if (d) { my.days = +d.dataset.cdDays; my.size = RANGES.find(r => r[1] === my.days)?.[2] ?? my.size; fresh(); return; }
      const z = e.target.closest('[data-cd-size]');
      if (z) { my.size = +z.dataset.cdSize; fresh(); return; }
      if (e.target.closest('[data-cd-im]')) { my.im = !my.im; my.imFor = ''; head(); load(); }
    };
    el.addEventListener('click', click);
    // The symbol picked in a watchlist, or a new theme.
    const follow = (/** @type {Event} */ e) => {
      const sym = /** @type {CustomEvent} */ (e).detail, any = anyService();
      if (typeof sym === 'string' && any && !offered().some(x => x.symbol === sym)) my.typed.push(sym);
      const c = typeof sym === 'string' && offered().find(x => x.symbol === sym);
      if (c) { my.pick = c; fresh(); } else if (e.type === 'tt-theme') { my.pick = null; fresh(); }
    };
    document.addEventListener('tt-focus', follow);
    document.addEventListener('tt-theme', follow);
    head(); load();
    // Minute bars change once a minute: refresh them every 5 s, longer ones every minute.
    let ticks = 0;
    my.timer = setInterval(() => { if (my.size === 60 || ++ticks % 12 === 0) load(); }, 5000);
    return {
      // The fleet's polls only matter for which symbols are offered.
      update() { if (!my.pick) { head(); load(); } },
      dispose() { my.alive = false; clearInterval(my.timer); my.chart?.remove(); el.removeEventListener('click', click); document.removeEventListener('tt-focus', follow); document.removeEventListener('tt-theme', follow); },
    };
  },
});

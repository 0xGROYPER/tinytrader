// Candles: one-minute candlesticks and volume for a symbol, from a relay's
// bars (the relay builds them from the trades it relays, and fills earlier
// minutes from its source's history where it keeps one, as Alpaca does), or
// in a themed workspace for the theme's symbols, from a service with bars
// for any symbol (alpaca-data) where no relay carries them.
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
  const t = theme(), live = relayed();
  if (!t?.symbols.length) return live;
  const any = (fleet.topo?.services || []).find(s => s.bars && s.snapshots)?.id;
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

defineWidget('candles', {
  title: 'Candles',
  mount(el) {
    const my = { pick: null, days: 1, size: 60, err: null, note: null, last: '', timer: 0, chart: null, candles: null, volume: null, alive: true, fit: true };
    el.innerHTML = '<div class="cd"><div class="cd-bar"></div><div class="cd-chart"></div><p class="cd-msg muted"></p></div>';
    const bar = el.querySelector('.cd-bar'), box = /** @type {HTMLElement} */ (el.querySelector('.cd-chart')), msg = el.querySelector('.cd-msg');
    const head = () => {
      const list = choices();
      if (!my.pick || !list.some(c => c.service === my.pick.service && c.symbol === my.pick.symbol)) my.pick = list.find(c => c.symbol === focused()) || list[0] || null;
      bar.innerHTML = list.length
        ? `<span class="chips">${list.map(c => `<button type="button" class="chip-btn" data-cd-pick="${esc(c.service)}|${esc(c.symbol)}" aria-pressed="${my.pick?.service === c.service && my.pick?.symbol === c.symbol}">${esc(c.symbol)}</button>`).join('')}</span>
           <span class="chips">${RANGES.map(([l, d]) => `<button type="button" class="chip-btn" data-cd-days="${d}" aria-pressed="${my.days === d}">${l}</button>`).join('')}</span>
           <span class="chips">${SIZES.filter(([, , most]) => my.days <= most).map(([l, z]) => `<button type="button" class="chip-btn" data-cd-size="${z}" aria-pressed="${my.size === z}">${l}</button>`).join('')}</span>
           <span class="cd-last mono">${esc(my.last)}</span>`
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
      const any = (fleet.topo?.services || []).find(s => s.bars && s.snapshots)?.id;
      const service = size > 60 && any ? any : my.pick.service, ask = service === any ? size : 60;
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
        my.candles.setData(bars.map(([t, o, h, l, c]) => ({ time: t + nyOffset(t), open: o, high: h, low: l, close: c })));
        my.volume.setData(bars.map(([t, o, , , c, vol]) => ({ time: t + nyOffset(t), value: vol, color: faded(c >= o ? css('--ok') : css('--crit')) })));
        // Show the whole range when one is picked; later refreshes keep the reader's zoom.
        if (my.fit) { my.chart.timeScale().fitContent(); my.fit = false; }
        const lastBar = bars[bars.length - 1];
        my.last = !lastBar ? 'no trades yet' : size >= 86400
          ? `${lastBar[4].toFixed(2)} · ${new Date((lastBar[0] + 43200) * 1000).toLocaleDateString(undefined, { timeZone: 'UTC', month: 'short', day: 'numeric' })}`
          : `${lastBar[4].toFixed(2)} · ${new Date(lastBar[0] * 1000).toLocaleTimeString('en-GB', { timeZone: 'America/New_York', hour: '2-digit', minute: '2-digit' })} ET`;
        my.err = null; my.note = v.note || (bars.length ? '' : `No bars for ${symbol} in this range yet.`);
      } catch (e) { my.err = e.message; }
      head();
    }
    const fresh = () => { my.chart?.remove(); my.chart = my.candles = my.volume = null; my.last = ''; my.fit = true; head(); load(); };
    const click = e => {
      const p = e.target.closest('[data-cd-pick]');
      if (p) { const [service, symbol] = p.dataset.cdPick.split('|'); my.pick = { service, symbol }; fresh(); return; }
      const d = e.target.closest('[data-cd-days]');
      if (d) { my.days = +d.dataset.cdDays; my.size = RANGES.find(r => r[1] === my.days)?.[2] ?? my.size; fresh(); return; }
      const z = e.target.closest('[data-cd-size]');
      if (z) { my.size = +z.dataset.cdSize; fresh(); }
    };
    el.addEventListener('click', click);
    // The symbol picked in a watchlist, or a new theme.
    const follow = (/** @type {Event} */ e) => {
      const sym = /** @type {CustomEvent} */ (e).detail, c = typeof sym === 'string' && choices().find(x => x.symbol === sym);
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

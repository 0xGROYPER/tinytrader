// Candles: candlesticks and volume for a symbol, from a service with bars for
// any symbol (alpaca-data: consolidated history with pre-market and after-
// hours, 04:00-20:00 New York, and its latest minutes), or from a relay's own
// bars where no such service runs (a relay has only its session hours).
// Pre-market and after-hours candles are drawn faded. Symbols: the theme's,
// the relays', or any typed into the ticker box. Pick a bar size; scroll or
// zoom back for more history, loaded a chunk at a time and kept in the page
// (lib/bars.js). Indicators (the expected move to a chosen horizon) are
// listed over the chart's top left, each with its settings.
// With the Spread builder's "Show on chart", the position is drawn too: its
// strikes and breakevens as lines, where it profits or loses at expiry shaded
// across the chart, and its payoff sideways at the right edge on the same
// prices. Legs are changed in the builder.
// Times are New York time. Drawn with TradingView's Lightweight Charts
// (Apache-2.0), loaded the first time a chart opens.
import { defineWidget } from '../lib/widgets.js';
import { fleet } from '../lib/fleet.js';
import { esc } from '../lib/format.js';
import { theme, focused } from '../lib/theme.js';
import { position } from '../lib/position.js';
import { held, latest, older, exhausted } from '../lib/bars.js';

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
/** A compact OCC option symbol, such as SPY261012C00777000: [match, root]. */
const OCC = /^([A-Z]{1,6})\d{6}[CP]\d{8}$/;
/** The relay carrying an option's root (SPY.OPT for SPY options), which charts today's mid from Databento's replay. */
function optionRelay(symbol) {
  const root = OCC.exec(symbol)?.[1];
  return root ? (fleet.topo?.services || []).find(s => s.bars && (s.schedule || []).some(x => (x.symbols || []).includes(`${root}.OPT`)))?.id || null : null;
}

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
// Bar sizes. History has no range to pick: the chart opens on the latest bars
// and loads older ones as it is scrolled or zoomed back (lib/bars.js).
/** @type {[string, number][]} */
const SIZES = [['1m', 60], ['5m', 300], ['15m', 900], ['1h', 3600], ['1D', 86400], ['1W', 604800]];
// Bars in view when a chart opens, and how near the left edge the view gets
// before the previous chunk is loaded.
const OPEN_BARS = 150, EDGE = 20;
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

// Indicators, drawn over the chart's top left as TradingView does: one row
// each, with its values, show or hide, settings and remove. One kind so far,
// the expected move: how far the options market expects the price to move
// by a horizon, whatever the chart's zoom: N trading days ahead, to a listed
// expiry, to the first expiry after the next earnings report, or to today's
// close (0DTE). Each expiry's at-the-money straddle over the price (from a
// service serving option chains, alpaca-data) gives the expected move to that
// date; for N days, as total variance, (move / 0.798)^2, it is interpolated
// linearly in time between the expiries either side (from zero before the
// first, at the last expiry's rate beyond it), so a report inside the horizon
// is included through the expiries after it. Shared by every Candles panel.
// A band setting draws, instead of ±the straddle, the 68% or 95% range of the
// expiry's implied distribution (fitted to the whole chain by alpaca-data:
// asymmetric where the smile is skewed); for N days the range of the expiry
// after the horizon, scaled by the square root of the variance ratio. A chain
// too thin to fit falls back to the straddle, saying so.
const IND_KEY = 'tinytrader-candles-indicators';
const COLOURS = ['--accent', '--warn-mark', '--ok', '#4f7fd9', '#a865c9', '#2a9d8f'];
/**
 * @typedef {{ type: 'days', n: number } | { type: 'expiry', expiry: string } | { type: 'earnings' } | { type: '0dte' }} Horizon
 * @typedef {'straddle' | '68' | '95'} Band
 * @typedef {{ id: string, kind: 'em', h: Horizon, colour: number, on: boolean, band?: Band }} Indicator
 */
/** @returns {Indicator[]} */
function loadIndicators() {
  try { const v = JSON.parse(localStorage.getItem(IND_KEY) || 'null'); if (Array.isArray(v)) return v; } catch (_) {}
  return [{ id: 'em1', kind: 'em', h: { type: 'days', n: 5 }, colour: 0, on: true }, { id: 'em2', kind: 'em', h: { type: 'earnings' }, colour: 1, on: true }];
}
const saveIndicators = (/** @type {Indicator[]} */ list) => { try { localStorage.setItem(IND_KEY, JSON.stringify(list)); } catch (_) {} };
const colourOf = (/** @type {number} */ i) => { const c = COLOURS[i % COLOURS.length]; return c.startsWith('--') ? css(c) || '#888' : c; };

const chainService = () => (fleet.topo?.services || []).find(s => s.chain)?.id || null;
const nextReport = sym => {
  const e = Object.values(fleet.snap?.services || {}).map(s => s?.earnings).find(x => Array.isArray(x?.reports));
  const today = new Date().toISOString().slice(0, 10);
  return (e?.reports || []).filter(r => r.symbol === sym && r.date >= today).sort((a, b) => a.date.localeCompare(b.date))[0] || null;
};
const K = Math.sqrt(2 / Math.PI); // a straddle's value over sigma * sqrt(T): 0.798
/** Calendar days from now to an expiry's 16:00 New York close. */
const daysTo = d => (Date.parse(`${d}T20:00:00Z`) - Date.now()) / 86400e3;
const isoIn = days => new Date(Date.now() + days * 86400e3).toISOString().slice(0, 10);
const dayLabel = d => new Date(d + 'T12:00:00Z').toLocaleDateString(undefined, { timeZone: 'UTC', month: 'short', day: 'numeric' });
// Option chains by underlying, reused for 5 minutes while they reach far enough.
/** @type {Map<string, { at: number, to: string, got: Promise<any> }>} */
const chains = new Map();
function chainFor(sym, to) {
  const c = chains.get(sym), svc = chainService();
  if (!svc) return Promise.resolve(null);
  if (c && c.to >= to && Date.now() - c.at < 300e3) return c.got;
  const got = fetch(`/api/stream/${encodeURIComponent(svc)}/chain?underlying=${encodeURIComponent(sym)}&to=${to}`, { cache: 'no-store' })
    .then(r => r.ok ? r.json() : null).catch(() => null);
  chains.set(sym, { at: Date.now(), to, got });
  return got;
}
/** How far ahead (a date) the chain must reach for these horizons. */
function reach(sym, /** @type {Indicator[]} */ list) {
  let days = 60;
  for (const { h } of list) {
    if (h.type === 'days') days = Math.max(days, h.n * 365 / 252 + 30);
    if (h.type === 'expiry') days = Math.max(days, daysTo(h.expiry) + 1);
    if (h.type === 'earnings') { const r = nextReport(sym); if (r) days = Math.max(days, daysTo(r.date) + 30); }
  }
  return isoIn(Math.min(days, 800));
}
const pct = m => `±${(m * 100).toFixed(m < 0.01 ? 2 : 1)}%`;
/** One horizon's expected move from a chain: { move, name, short } or { none }. */
function expected(v, /** @type {Horizon} */ h, sym) {
  const exps = (v?.expiries || []).filter(x => x.implied_move > 0 && daysTo(x.expiry) > 0.05)
    .map(x => ({ ...x, t: daysTo(x.expiry), w: (x.implied_move / K) ** 2 })).sort((a, b) => a.t - b.t);
  if (!v?.spot || !exps.length) return { none: 'no option chain' };
  if (h.type === 'days') {
    const horizon = h.n * 365 / 252;
    let w, beyond = false;
    const after = exps.findIndex(x => x.t >= horizon);
    if (after === 0) w = exps[0].w * horizon / exps[0].t;
    else if (after > 0) { const a = exps[after - 1], b = exps[after]; w = a.w + (b.w - a.w) * (horizon - a.t) / (b.t - a.t); }
    else { const last = exps[exps.length - 1]; w = last.w * horizon / last.t; beyond = true; }
    // The distribution's shape from the expiry after the horizon (or the last), scaled to it.
    const ref = exps[after >= 0 ? after : exps.length - 1];
    return { move: K * Math.sqrt(Math.max(w, 0)), name: `${h.n} trading day${h.n === 1 ? '' : 's'}${beyond ? ' (past the listed expiries)' : ''}`, short: `${h.n}d`,
      ref, scale: Math.sqrt(Math.max(w, 0) / ref.w), shape: ref.expiry };
  }
  if (h.type === 'expiry') {
    const e = exps.find(x => x.expiry === h.expiry);
    return e ? { move: e.implied_move, name: `to ${dayLabel(e.expiry)}`, short: dayLabel(e.expiry), ref: e, scale: 1 } : { none: `${dayLabel(h.expiry)} not listed` };
  }
  if (h.type === 'earnings') {
    const rep = nextReport(sym);
    if (!rep) return { none: 'no report ahead' };
    const e = exps.find(x => rep.when === 'after-close' ? x.expiry > rep.date : x.expiry >= rep.date);
    return e ? { move: e.implied_move, name: `earnings ${dayLabel(rep.date)}, to ${dayLabel(e.expiry)}`, short: 'earnings', ref: e, scale: 1 } : { none: 'no expiry after the report' };
  }
  const todayNY = new Date().toLocaleDateString('en-CA', { timeZone: 'America/New_York' });
  const z = exps.find(x => x.expiry === todayNY);
  return z ? { move: z.implied_move, name: "to today's close (0DTE)", short: '0DTE', ref: z, scale: 1 } : { none: 'no expiry today' };
}
/** A band's range from the implied distribution: { lo, hi, p } or { fallback }. */
function range(e, /** @type {Band | undefined} */ band, spot) {
  if (!band || band === 'straddle') return null;
  const d = e.ref?.distribution, q = d?.quantiles;
  if (!d || d.quality === 'poor' || !q) return { fallback: d?.reason || 'chain too thin to fit' };
  const [a, b] = band === '95' ? [q.p2_5, q.p97_5] : [q.p16, q.p84];
  if (!(a > 0 && b > 0)) return { fallback: 'no range' };
  const at = x => spot * Math.exp(Math.log(x / spot) * (e.scale ?? 1));
  return { lo: at(a), hi: at(b), p: band, quality: d.quality };
}

defineWidget('candles', {
  title: 'Candles',
  mount(el) {
    const my = { typed: [],
      pick: null, size: 60, err: null, note: null, last: '', timer: 0, chart: null, candles: null, volume: null, alive: true,
      drawn: { key: '', first: 0, len: 0 }, backing: false,
      ind: loadIndicators(), open: '', expiries: /** @type {string[]} */ ([]), em: /** @type {Record<string, any>} */ ({}), emLines: [], spot: 0, emAt: 0, emFor: '',
      L: null, posLines: [], over: null, raf: 0, sig: '', hoverY: null };
    el.innerHTML = '<div class="cd"><div class="cd-top"><form class="cd-search"><input class="cd-ticker mono" name="ticker" placeholder="Ticker or option" maxlength="21" autocomplete="off" spellcheck="false" aria-label="Show a ticker"></form><div class="cd-bar"></div></div><div class="cd-chart"><div class="cd-legend"></div></div><p class="cd-msg muted"></p></div>';
    const bar = el.querySelector('.cd-bar'), box = /** @type {HTMLElement} */ (el.querySelector('.cd-chart')), msg = el.querySelector('.cd-msg');
    const leg = /** @type {HTMLElement} */ (el.querySelector('.cd-legend'));
    const anyService = () => (fleet.topo?.services || []).find(s => s.bars && s.snapshots)?.id;
    const offered = () => {
      const list = choices(), any = anyService();
      for (const symbol of my.typed) {
        if (list.some(c => c.symbol === symbol)) continue;
        const relay = optionRelay(symbol);
        if (relay) list.push({ service: relay, symbol }); else if (any && !OCC.test(symbol)) list.push({ service: any, symbol });
      }
      return list;
    };
    // Where bars come from: a service with bars for any symbol (history at
    // every size), else the pick's relay (minutes, combined into larger bars).
    // An option comes from its relay: today's minutes, combined into larger bars.
    const source = () => { const any = anyService(), service = OCC.test(my.pick.symbol) ? my.pick.service : any || my.pick.service; return { service, ask: service === any ? my.size : 60 }; };
    const head = () => {
      const list = offered();
      if (!my.pick || !list.some(c => c.service === my.pick.service && c.symbol === my.pick.symbol)) my.pick = list.find(c => c.symbol === focused()) || list[0] || null;
      bar.innerHTML = list.length
        ? `<span class="chips">${list.map(c => `<button type="button" class="chip-btn" data-cd-pick="${esc(c.service)}|${esc(c.symbol)}" aria-pressed="${my.pick?.service === c.service && my.pick?.symbol === c.symbol}">${esc(c.symbol)}</button>`).join('')}</span>
           <span class="chips">${SIZES.map(([l, z]) => `<button type="button" class="chip-btn" data-cd-size="${z}" aria-pressed="${my.size === z}">${l}</button>`).join('')}</span>`
        : '';
      msg.textContent = !list.length ? (theme() ? 'No service serves bars for the theme’s symbols.' : 'No relay serves bars yet.') : my.err ? my.err : my.note || '';
    };
    const colours = () => ({
      layout: { background: { color: 'transparent' }, textColor: css('--muted'), fontFamily: css('--mono') || 'monospace', fontSize: 11 },
      grid: { vertLines: { color: css('--line-soft') }, horzLines: { color: css('--line-soft') } },
      rightPriceScale: { borderColor: css('--line') }, timeScale: { borderColor: css('--line'), timeVisible: true, secondsVisible: false },
    });
    function makeChart(L) {
      my.chart = L.createChart(box, { autoSize: true, ...colours() });
      my.chart.timeScale().applyOptions({ timeVisible: my.size < 86400 });
      my.candles = my.chart.addSeries(L.CandlestickSeries, { upColor: css('--ok'), downColor: css('--crit'), wickUpColor: css('--ok'), wickDownColor: css('--crit'), borderVisible: false });
      my.volume = my.chart.addSeries(L.HistogramSeries, { priceScaleId: 'volume', priceFormat: { type: 'volume' }, color: css('--line') });
      my.chart.priceScale('volume').applyOptions({ scaleMargins: { top: 0.8, bottom: 0 } });
      // Keep a shown position's strikes and breakevens in view.
      my.candles.applyOptions({ autoscaleInfoProvider: (/** @type {() => any} */ orig) => {
        const r = orig(), ks = shownHere() ? [...position.legs.map(l => l.strike), ...position.be] : [];
        return r && ks.length ? { ...r, priceRange: { minValue: Math.min(r.priceRange.minValue, ...ks), maxValue: Math.max(r.priceRange.maxValue, ...ks) } } : r;
      } });
      my.chart.subscribeCrosshairMove((/** @type {any} */ p) => { my.hoverY = p?.point ? p.point.y : null; });
      // Near the left edge: load the previous chunk.
      my.chart.timeScale().subscribeVisibleLogicalRangeChange((/** @type {any} */ r) => { if (r && r.from < EDGE) back(); });
      my.over = document.createElement('canvas'); my.over.className = 'cd-over'; box.appendChild(my.over);
      my.L = L;
    }
    // Draws a series: in place when only its end moved (the reader's view
    // stays), else whole, keeping the same bars in view when older ones were
    // added at the start, or opening on the latest bars for a new series.
    function draw(raw) {
      const size = my.size, { service, ask } = source(), key = `${service}|${my.pick.symbol}|${size}`;
      const bars = ask < size ? combine(raw, size) : raw;
      // Outside 09:30-16:00 New York (intraday bars only), faded.
      const extended = t => { if (size >= 86400) return false; const m = Math.floor(((t + nyOffset(t)) % 86400 + 86400) % 86400 / 60); return m < 570 || m >= 960; };
      const up = css('--ok'), down = css('--crit');
      const candle = ([t, o, h, l, c]) => {
        const b = { time: t + nyOffset(t), open: o, high: h, low: l, close: c };
        if (!extended(t)) return b;
        const col = faded(c >= o ? up : down);
        return { ...b, color: col, wickColor: col, borderColor: col };
      };
      const vol = ([t, o, , , c, v]) => ({ time: t + nyOffset(t), value: v, color: faded(c >= o ? up : down) });
      const d = my.drawn, n = bars.length, first = bars[0]?.[0] ?? 0, ts = my.chart.timeScale();
      if (d.key === key && d.first === first && n >= d.len && n - d.len <= 50 && d.len) {
        for (let i = d.len - 1; i < n; i++) { my.candles.update(candle(bars[i])); if (bars[i][5] > 0) my.volume.update(vol(bars[i])); }
      } else {
        const r = d.key === key ? ts.getVisibleLogicalRange() : null;
        const shift = r && d.first ? Math.max(0, bars.findIndex(b => b[0] === d.first)) : 0;
        // Indexes have no volume: no histogram (or its "0" label) for them.
        my.candles.setData(bars.map(candle)); my.volume.setData(bars.some(b => b[5] > 0) ? bars.map(vol) : []);
        if (r) ts.setVisibleLogicalRange({ from: r.from + shift, to: r.to + shift });
        else if (n) ts.setVisibleLogicalRange({ from: Math.max(0, n - OPEN_BARS), to: n + 3 });
      }
      my.drawn = { key, first, len: n };
      const lastBar = bars[n - 1];
      my.last = !lastBar ? 'no trades yet' : size >= 86400
        ? `${lastBar[4].toFixed(2)} · ${new Date((lastBar[0] + 43200) * 1000).toLocaleDateString(undefined, { timeZone: 'UTC', month: 'short', day: 'numeric' })}`
        : `${lastBar[4].toFixed(2)} · ${new Date(lastBar[0] * 1000).toLocaleTimeString('en-GB', { timeZone: 'America/New_York', hour: '2-digit', minute: '2-digit' })} ET`;
    }
    async function load() {
      if (!my.pick || !my.alive) return;
      const { symbol } = my.pick, size = my.size, { service, ask } = source();
      const same = () => my.alive && my.pick?.symbol === symbol && my.size === size;
      try {
        if (ask === 60 && size >= 86400) throw new Error('Daily and weekly bars need a service with history for any symbol (alpaca-data).');
        const L = await library();
        if (!same()) return;
        if (!my.chart) makeChart(L); else my.chart.applyOptions(colours());
        // A series already held draws at once; then its latest bars.
        const kept = held(service, symbol, ask);
        if (kept.length && !my.drawn.key) draw(kept);
        const s = await latest(service, symbol, ask);
        if (!same() || !my.chart) return;
        draw(s.bars);
        my.err = null; my.note = s.note || (s.bars.length ? '' : `No bars for ${symbol} yet.`);
        await drawMoves();
        drawPosition();
      } catch (e) { my.err = e.message; }
      head(); legend();
    }
    // The previous chunk of history, while the view is near the left edge.
    async function back() {
      if (my.backing || !my.pick || !my.chart || !my.drawn.len) return;
      const { symbol } = my.pick, size = my.size, { service, ask } = source();
      if (exhausted(service, symbol, ask)) return;
      my.backing = true; legend();
      let added = 0;
      try {
        added = await older(service, symbol, ask);
        if (added && my.alive && my.pick?.symbol === symbol && my.size === size && my.chart) draw(held(service, symbol, ask));
      } catch (e) { my.err = e.message; head(); }
      my.backing = false; legend();
      const r = my.chart?.timeScale().getVisibleLogicalRange();
      if (added && r && r.from < EDGE) back();
    }

    // Expected-move lines: recomputed when the symbol or an indicator changes,
    // and every 5 minutes with a fresh chain.
    async function drawMoves(force = false) {
      if (!my.pick || !my.candles) return;
      const symbol = my.pick.symbol, want = JSON.stringify(my.ind);
      if (!force && my.emFor === `${symbol}|${want}` && Date.now() - my.emAt < 300e3) return;
      const v = my.ind.length ? await chainFor(symbol, reach(symbol, my.ind)) : null;
      if (!my.alive || my.pick?.symbol !== symbol || !my.candles) return;
      for (const l of my.emLines) my.candles.removePriceLine(l);
      my.emLines = []; my.em = {}; my.spot = v?.spot || 0; my.expiries = (v?.expiries || []).map(x => x.expiry);
      my.emFor = `${symbol}|${want}`; my.emAt = Date.now();
      for (const ind of my.ind) {
        const e = v ? expected(v, ind.h, symbol) : { none: chainService() ? 'no option chain' : 'no option chain service' };
        my.em[ind.id] = e;
        if (!ind.on || !e.move) continue;
        const color = colourOf(ind.colour), r = e.r = range(e, ind.band, v.spot);
        const lines = r?.lo ? [[r.hi, `${r.p}% ${e.short}`], [r.lo, '']] : [[v.spot * (1 + e.move), `${pct(e.move)} ${e.short}`], [v.spot * (1 - e.move), '']];
        for (const [price, title] of lines) my.emLines.push(my.candles.createPriceLine({
          price, color, lineWidth: 1, lineStyle: my.L.LineStyle.Dashed, axisLabelVisible: true, title,
        }));
      }
      legend();
    }
    // The legend over the chart: the symbol and last bar, then each indicator.
    function legend() {
      // Not while a setting is being typed.
      if (leg.contains(document.activeElement) && document.activeElement?.matches('input')) return;
      const sz = SIZES.find(s => s[1] === my.size)?.[0] || '';
      const { service, ask } = my.pick ? source() : { service: '', ask: 0 };
      const status = my.backing ? ' · loading older…' : my.pick && my.drawn.len && exhausted(service, my.pick.symbol, ask) ? ' · start of history' : '';
      const rows = my.ind.map(ind => {
        const e = my.em[ind.id] || {}, col = colourOf(ind.colour), h = ind.h;
        const r = e.r, inside = e.ref?.distribution?.quality !== 'poor' ? e.ref?.distribution?.p_inside_straddle : null;
        const chg = x => `${x >= my.spot ? '+' : '−'}${Math.abs((x / my.spot - 1) * 100).toFixed(1)}%`;
        const val = !e.move ? `<span class="muted">${esc(e.none || '')}</span>`
          : r?.lo ? `<b>${r.p}%</b> ${r.lo.toFixed(2)} – ${r.hi.toFixed(2)} <span class="muted">${chg(r.lo)} / ${chg(r.hi)}${e.shape ? ` · shape of ${dayLabel(e.shape)}` : ''}${r.quality === 'rough' ? ' · rough fit' : ''}</span>`
          : `<b>${pct(e.move)}</b> <span class="muted">${(my.spot * (1 - e.move)).toFixed(2)} – ${(my.spot * (1 + e.move)).toFixed(2)}${inside != null && e.scale === 1 ? ` · ${Math.round(inside * 100)}% inside` : ''}${r?.fallback ? ` · straddle: ${esc(r.fallback)}` : ''}</span>`;
        const set = my.open === ind.id ? `<div class="cd-lg-set" data-ind="${ind.id}">
            <select data-set="type" aria-label="Horizon">${[['days', 'Trading days'], ['expiry', 'To an expiry'], ['earnings', 'After earnings'], ['0dte', '0DTE']].map(([v, l]) => `<option value="${v}"${h.type === v ? ' selected' : ''}>${l}</option>`).join('')}</select>
            ${h.type === 'days' ? `<input data-set="n" type="number" min="1" max="1260" value="${h.n}" aria-label="Trading days">` : ''}
            ${h.type === 'expiry' ? `<select data-set="expiry" aria-label="Expiry">${(my.expiries || []).map(x => `<option value="${x}"${h.expiry === x ? ' selected' : ''}>${dayLabel(x)}</option>`).join('')}</select>` : ''}
            <select data-set="band" aria-label="Band">${[['straddle', 'Straddle'], ['68', '68% range'], ['95', '95% range']].map(([v, l]) => `<option value="${v}"${(ind.band || 'straddle') === v ? ' selected' : ''}>${l}</option>`).join('')}</select>
            <span class="cd-lg-cols">${COLOURS.map((_, i) => `<button type="button" class="cd-lg-col" data-set="colour" data-v="${i}" aria-label="Colour ${i + 1}" aria-pressed="${ind.colour === i}" style="background:${colourOf(i)}"></button>`).join('')}</span>
          </div>` : '';
        return `<div class="cd-lg-row${ind.on ? '' : ' off'}" data-ind="${ind.id}"><span class="cd-sw" style="background:${col}"></span><span>Expected move <span class="muted">${esc(e.name || (h.type === 'days' ? `${h.n} trading days` : h.type))}</span></span> ${val}
          <button type="button" class="cd-lg-btn" data-act="eye" aria-label="${ind.on ? 'Hide' : 'Show'}" title="${ind.on ? 'Hide' : 'Show'}">${ind.on ? '◉' : '○'}</button><button type="button" class="cd-lg-btn" data-act="gear" aria-label="Settings" aria-expanded="${my.open === ind.id}" title="Settings">⚙</button><button type="button" class="cd-lg-btn" data-act="remove" aria-label="Remove" title="Remove">×</button></div>${set}`;
      }).join('');
      leg.innerHTML = my.pick ? `<div class="cd-lg-row cd-lg-sym"><b>${esc(my.pick.symbol)}</b> <span class="muted">${sz}</span> ${esc(my.last)}<span class="muted">${status}</span></div>${rows}<div><button type="button" class="cd-lg-btn cd-lg-add" data-act="add">+ Expected move</button></div>` : '';
    }
    const changedInd = () => { saveIndicators(my.ind); drawMoves(true); };
    leg.addEventListener('click', e => {
      const b = /** @type {HTMLElement} */ (e.target).closest('button'); if (!b) return;
      const id = /** @type {HTMLElement} */ (b.closest('[data-ind]'))?.dataset.ind, ind = my.ind.find(i => i.id === id);
      if (b.dataset.act === 'add') {
        const used = new Set(my.ind.map(i => i.colour)), colour = COLOURS.findIndex((_, i) => !used.has(i));
        const n = { id: 'em' + Date.now().toString(36), kind: /** @type {'em'} */ ('em'), h: /** @type {Horizon} */ ({ type: 'days', n: 21 }), colour: colour < 0 ? my.ind.length : colour, on: true };
        my.ind.push(n); my.open = n.id; changedInd(); return;
      }
      if (!ind) return;
      if (b.dataset.act === 'eye') { ind.on = !ind.on; changedInd(); }
      else if (b.dataset.act === 'gear') { my.open = my.open === id ? '' : id; legend(); if (my.open) drawMoves(true); }
      else if (b.dataset.act === 'remove') { my.ind = my.ind.filter(i => i !== ind); if (my.open === id) my.open = ''; changedInd(); }
      else if (b.dataset.set === 'colour') { ind.colour = +(/** @type {string} */ (b.dataset.v)); changedInd(); }
    });
    leg.addEventListener('change', e => {
      const t = /** @type {HTMLInputElement} */ (e.target), ind = my.ind.find(i => i.id === /** @type {HTMLElement} */ (t.closest('[data-ind]'))?.dataset.ind);
      if (!ind) return;
      if (t.dataset.set === 'type') {
        const type = t.value;
        ind.h = type === 'days' ? { type, n: 5 } : type === 'expiry' ? { type, expiry: my.expiries?.[0] || isoIn(7) } : /** @type {Horizon} */ ({ type });
      } else if (t.dataset.set === 'n') ind.h = { type: 'days', n: Math.max(1, Math.min(1260, Math.round(+t.value) || 1)) };
      else if (t.dataset.set === 'expiry') ind.h = { type: 'expiry', expiry: t.value };
      else if (t.dataset.set === 'band') ind.band = /** @type {Band} */ (t.value);
      t.blur(); changedInd();
    });
    // Clicks on the legend aren't chart drags.
    leg.addEventListener('pointerdown', e => e.stopPropagation());

    const fresh = () => { my.chart?.remove(); my.over?.remove(); my.chart = my.candles = my.volume = my.over = null; my.emLines = []; my.posLines = []; my.sig = ''; my.emFor = ''; my.last = ''; my.drawn = { key: '', first: 0, len: 0 }; head(); legend(); load(); };
    // The builder's position, when shown and on this chart's symbol.
    const shownHere = () => !!(position.shown && position.pay && position.legs.length && position.sym === my.pick?.symbol);
    const money = (/** @type {number} */ v) => `${v < 0 ? '−' : '+'}$${Math.abs(v * 100).toFixed(0)}`;
    const sideCol = (/** @type {number} */ side) => side < 0 ? css('--crit-mark') || css('--crit') : css('--ok');
    function drawPosition() {
      for (const l of my.posLines) my.candles?.removePriceLine(l);
      my.posLines = []; my.sig = '';
      cancelAnimationFrame(my.raf);
      if (!my.candles || !my.L) return;
      if (!shownHere()) { paint(); return; }
      const pay = /** @type {(s: number) => number} */ (position.pay);
      for (const l of position.legs) my.posLines.push(my.candles.createPriceLine({
        price: l.strike, color: sideCol(l.side), lineWidth: 2, lineStyle: l.side < 0 ? my.L.LineStyle.Solid : my.L.LineStyle.Dashed, axisLabelVisible: true,
        title: `${l.side < 0 ? 'Sell' : 'Buy'}${l.qty > 1 ? ' ' + l.qty : ''} ${l.strike}${l.type === 'call' ? 'C' : 'P'}  ${money(pay(l.strike))}`,
      }));
      for (const b of position.be) my.posLines.push(my.candles.createPriceLine({
        price: b, color: css('--warn-mark') || css('--warn'), lineWidth: 1, lineStyle: my.L.LineStyle.Dotted, axisLabelVisible: true, title: 'breakeven',
      }));
      my.chart.priceScale('right').applyOptions({ autoScale: true });
      const loop = () => { paint(); my.raf = requestAnimationFrame(loop); };
      loop();
    }
    // The overlay: profit and loss shading, the sideways payoff and the P&L
    // at the pointer's price; repainted only when the scale or pointer moves.
    function paint() {
      const c = my.over; if (!c || !my.candles || !my.chart) return;
      const on = shownHere(), plotW = my.chart.timeScale().width(), plotH = box.clientHeight - my.chart.timeScale().height();
      const sig = on ? [plotW, plotH, my.candles.priceToCoordinate(100), my.candles.priceToCoordinate(200), my.hoverY, position.net, position.legs.map(l => l.side * l.qty + l.type + l.strike).join()].join('|') : 'off';
      if (sig === my.sig) return;
      my.sig = sig;
      const dpr = window.devicePixelRatio || 1, W = box.clientWidth, H = box.clientHeight;
      if (c.width !== Math.round(W * dpr) || c.height !== Math.round(H * dpr)) { c.width = Math.round(W * dpr); c.height = Math.round(H * dpr); }
      const g = /** @type {CanvasRenderingContext2D} */ (c.getContext('2d'));
      g.setTransform(dpr, 0, 0, dpr, 0, 0); g.clearRect(0, 0, W, H);
      if (!on || plotW <= 0 || plotH <= 0) return;
      const pay = /** @type {(s: number) => number} */ (position.pay), ok = css('--ok'), bad = css('--crit');
      const at = (/** @type {number} */ y) => { const p = my.candles.coordinateToPrice(y); return p == null ? null : pay(p); };
      // Shading: green where it profits at expiry, red where it loses.
      const vals = [];
      for (let y = 0; y < plotH; y += 2) { const v = at(y); vals.push([y, v]); if (v == null) continue; g.fillStyle = v >= 0 ? ok : bad; g.globalAlpha = 0.07; g.fillRect(0, y, plotW, 2); }
      g.globalAlpha = 1;
      // The payoff sideways, in the right-hand 96 px.
      const fin = vals.filter(([, v]) => v != null && isFinite(v)), lo = Math.min(0, ...fin.map(([, v]) => v)), hi = Math.max(0, ...fin.map(([, v]) => v));
      if (fin.length && hi > lo) {
        const x0 = plotW - 100, x1 = plotW - 6, X = (/** @type {number} */ v) => x0 + (v - lo) / (hi - lo) * (x1 - x0);
        g.fillStyle = css('--bg') || '#fff'; g.globalAlpha = 0.75; g.fillRect(x0 - 4, 0, x1 - x0 + 10, plotH); g.globalAlpha = 1;
        g.strokeStyle = css('--line'); g.lineWidth = 1; g.beginPath(); g.moveTo(X(0), 0); g.lineTo(X(0), plotH); g.stroke();
        g.strokeStyle = css('--fg') || '#111'; g.lineWidth = 2; g.beginPath();
        fin.forEach(([y, v], i) => i ? g.lineTo(X(v), y) : g.moveTo(X(v), y)); g.stroke();
        g.font = `10px ${css('--mono') || 'monospace'}`; g.fillStyle = css('--muted');
        g.textAlign = 'center'; g.fillText('P&L at expiry', (x0 + x1) / 2, 12);
      }
      // The P&L at the pointer's price.
      if (my.hoverY != null && my.hoverY < plotH) {
        const p = my.candles.coordinateToPrice(my.hoverY), v = p == null ? null : pay(p);
        if (v != null) {
          const text = `${p.toFixed(2)}: ${money(v)} at expiry`;
          g.font = `12px ${css('--mono') || 'monospace'}`; g.textAlign = 'left';
          const w = g.measureText(text).width + 12;
          // Bottom left: the legend has the top.
          g.fillStyle = css('--bg') || '#fff'; g.globalAlpha = 0.9; g.fillRect(6, plotH - 26, w, 20); g.globalAlpha = 1;
          g.fillStyle = v >= 0 ? ok : bad; g.fillText(text, 12, plotH - 12);
        }
      }
    }
    const changed = () => drawPosition();
    document.addEventListener('tt-position', changed);
    // The ticker box: any symbol, from the service with bars for any symbol.
    el.addEventListener('submit', e => {
      e.preventDefault();
      const input = /** @type {HTMLInputElement} */ (/** @type {HTMLElement} */ (e.target).closest('form')?.querySelector('.cd-ticker'));
      const symbol = (input?.value || '').trim().toUpperCase();
      const any = anyService();
      if (!/^[A-Z][A-Z0-9.]{0,9}$/.test(symbol) && !OCC.test(symbol)) return;
      if (OCC.test(symbol) && !optionRelay(symbol)) { my.err = `No OPRA relay carries ${OCC.exec(symbol)[1]} options, so there's nothing to chart for ${symbol}.`; head(); return; }
      if (!any && !OCC.test(symbol)) { my.err = 'Typing a ticker needs a service with bars for any symbol (alpaca-data).'; head(); return; }
      if (!my.typed.includes(symbol)) my.typed.push(symbol);
      my.pick = offered().find(c => c.symbol === symbol) || { service: optionRelay(symbol) || any, symbol };
      fresh();
    });
    const click = e => {
      const p = e.target.closest('[data-cd-pick]');
      if (p) { const [service, symbol] = p.dataset.cdPick.split('|'); my.pick = { service, symbol }; fresh(); return; }
      const z = e.target.closest('[data-cd-size]');
      if (z) { my.size = +z.dataset.cdSize; fresh(); }
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
    // A contract to chart (the Spread builder's legs): today's mid from its relay.
    const chart = (/** @type {Event} */ e) => {
      const symbol = /** @type {CustomEvent} */ (e).detail, relay = typeof symbol === 'string' ? optionRelay(symbol) : null;
      if (!relay) return;
      if (!my.typed.includes(symbol)) my.typed.push(symbol);
      my.pick = { service: relay, symbol }; fresh();
    };
    document.addEventListener('tt-chart', chart);
    document.addEventListener('tt-theme', follow);
    head(); legend(); load();
    // Minute bars change once a minute: refresh them every 5 s, longer ones every minute.
    let ticks = 0;
    my.timer = setInterval(() => { if (my.size === 60 || ++ticks % 12 === 0) load(); }, 5000);
    return {
      // The fleet's polls only matter for which symbols are offered.
      update() { if (!my.pick) { head(); load(); } },
      dispose() { my.alive = false; clearInterval(my.timer); cancelAnimationFrame(my.raf); document.removeEventListener('tt-position', changed); my.chart?.remove(); el.removeEventListener('click', click); document.removeEventListener('tt-focus', follow); document.removeEventListener('tt-chart', chart); document.removeEventListener('tt-theme', follow); },
    };
  },
});

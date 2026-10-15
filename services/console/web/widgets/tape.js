// Tape: time and sales from the IEX relay, a tick at a time, newest at the
// top: each trade's time, price and size, coloured by where it printed
// against the quote before it (green at or above the ask, a buy; red at or
// below the bid, a sell; grey between), quote changes between them if
// wanted. Each tick flashes as it arrives, and the last price flashes up or
// down, as terminals do. Big prints stand out; a size floor hides small ones.
// One line above: the last price and quote, the window's buy share of the
// volume, VWAP and trades a minute. Read from the relay's /tape twice a
// second while shown; hovering the list (or pause) holds it still. IEX is a
// few percent of all US volume, so it's a sample of the flow, not all of it.
// Any ticker can be typed: one the relay doesn't carry is watched for the
// tape on request (its open stream takes the symbol; no restart), with its
// trades showing from then on.
import { defineWidget, paint } from '../lib/widgets.js';
import { fleet } from '../lib/fleet.js';
import { esc } from '../lib/format.js';
import { focused } from '../lib/theme.js';

const EVERY = 500, HOLD = 1500, SHOW = 300, BIG = 1000, FLASH = 1200;
const SIZES = [0, 100, 500, 1000, 5000];
const relay = () => (fleet.topo?.services || []).find(s => s.tape)?.id || null;
const hms = t => new Date(t * 1000).toLocaleTimeString('en-GB', { timeZone: 'America/New_York', hour12: false });
const ms = t => String(Math.floor((t % 1) * 1000)).padStart(3, '0');
const side = x => x.p == null ? '' : x.a != null && x.p >= x.a ? 'buy' : x.b != null && x.p <= x.b ? 'sell' : 'mid';
const n = v => v >= 1e6 ? `${(v / 1e6).toFixed(2)}M` : v >= 1e4 ? `${(v / 1e3).toFixed(0)}K` : v.toLocaleString();

function head(my) {
  return `<div class="tp-bar"><form class="tp-find" role="search"><input class="tp-sym mono" name="s" value="${esc(my.sym)}" placeholder="Ticker" aria-label="Ticker, then Enter" spellcheck="false" autocomplete="off"></form><b class="mono tp-last">—</b><span class="tp-stats"></span>
    <span class="tp-ctl"><select class="tp-min" aria-label="Smallest size shown" title="Smallest size shown">${SIZES.map(z => `<option value="${z}"${z === my.min ? ' selected' : ''}>${z ? `≥ ${z.toLocaleString()}` : 'All sizes'}</option>`).join('')}</select>
    <button type="button" class="tile-btn" data-tp="quotes" aria-pressed="${my.quotes}" title="${my.quotes ? 'Hide' : 'Show'} quote changes">Q</button>
    <button type="button" class="tile-btn" data-tp="pause" aria-pressed="${my.paused}" title="${my.paused ? 'Resume' : 'Pause'}">${my.paused ? '▶' : '❚❚'}</button></span></div>`;
}

// The line above the list, after the last price.
function stats(my) {
  const trades = my.ticks.filter(x => x.k === 'T');
  let buy = 0, sell = 0, pv = 0, vol = 0;
  for (const x of trades) { const s = side(x); if (s === 'buy') buy += x.z; else if (s === 'sell') sell += x.z; pv += x.p * x.z; vol += x.z; }
  const span = trades.length > 1 ? trades[trades.length - 1].t - trades[0].t : 0;
  const top = my.ticks[my.ticks.length - 1];
  const share = buy + sell ? Math.round(buy / (buy + sell) * 100) : null;
  return (top?.b != null ? `<span class="mono muted">${top.b.toFixed(2)}<i>×</i>${top.a?.toFixed(2) ?? '—'}</span>` : '')
    + (share != null ? `<span class="tp-split" title="Of the window's volume at the bid or ask: ${n(buy)} bought at the ask, ${n(sell)} sold at the bid"><i style="width:${share}%"></i></span><span class="mono tp-share">${share}% buy</span>` : '')
    + (vol ? `<span class="mono muted" title="Volume-weighted average price of the window, and its trades a minute">VWAP ${(pv / vol).toFixed(2)}${span > 0 ? ` · ${(trades.length / span * 60).toFixed(0)}/min` : ''}</span>` : '');
}

// The list, newest first. A tick flashes for a moment after it arrives (the
// animation carried on across redraws by starting it that far in).
function list(my) {
  const now = performance.now();
  // Only the newest few: a burst of quotes shouldn't light up the whole list.
  let lit = 0;
  const fl = x => { const age = now - (x.rt || 0); return age < FLASH && lit++ < 40 ? ` tp-new" style="animation-delay:-${Math.round(age)}ms` : ''; };
  if (!relay()) return '<p class="muted">No relay serves a tape: relay-iex needs redeploying with it.</p>';
  if (my.err) return `<p class="msg-err">${esc(my.err)}</p>`;
  if (my.missing) return `<p class="muted">${esc(my.sym)} can't be shown: ${esc(my.missing)}.</p>`;
  const rows = [];
  for (let i = my.ticks.length - 1; i >= 0 && rows.length < SHOW; i--) {
    const x = my.ticks[i], fresh = fl(x);
    if (x.k === 'Q') { if (my.quotes) rows.push(`<tr class="tp-q${fresh}"><td>${hms(x.t)}<small>.${ms(x.t)}</small></td><td class="num">${x.b?.toFixed(2) ?? '—'}<i>×</i>${x.a?.toFixed(2) ?? '—'}</td><td class="num">${x.bz}<i>×</i>${x.az}</td></tr>`); continue; }
    if (x.z < my.min) continue;
    rows.push(`<tr class="tp-${side(x)}${x.z >= BIG ? ' tp-big' : ''}${fresh}"><td>${hms(x.t)}<small>.${ms(x.t)}</small></td><td class="num">${x.p.toFixed(2)}</td><td class="num">${x.z.toLocaleString()}</td></tr>`);
  }
  const quiet = !my.lastRecord || Date.now() / 1000 - my.lastRecord > 120;
  const none = my.watching ? `Watching ${esc(my.sym)}: its trades show here as they happen.` : 'No trades yet.';
  return `<div class="tp-list"><table class="tp"><tbody>${rows.join('') || `<tr><td colspan="3" class="muted">${none}</td></tr>`}</tbody></table></div>`
    + (quiet || my.paused ? `<p class="muted tp-foot">${my.paused ? 'Paused. ' : ''}${quiet ? 'The feed is quiet: outside its hours (07:55–17:05 ET), or down.' : ''}</p>` : '');
}

defineWidget('tape', {
  title: 'Tape',
  mount(el) {
    const my = { missing: /** @type {string | false} */ (false), watching: false, sym: '', symbols: /** @type {string[]} */ ([]), ticks: /** @type {any[]} */ ([]), last: 0, err: '', quotes: false, min: 0, paused: false, hover: 0, lastRecord: 0, alive: true, busy: false };
    el.innerHTML = '<div class="tp-top"></div><div class="tp-main"></div>';
    const top = /** @type {HTMLElement} */ (el.querySelector('.tp-top')), main = /** @type {HTMLElement} */ (el.querySelector('.tp-main'));
    // The last price flashes up or down when it changes.
    let lastPx = null;
    const draw = () => {
      paint(top, head(my));
      if (my.paused || Date.now() - my.hover <= HOLD) return;
      const px = [...my.ticks].reverse().find(x => x.k === 'T')?.p ?? null, b = /** @type {HTMLElement | null} */ (top.querySelector('.tp-last'));
      if (b && px == null) { b.textContent = '—'; b.classList.remove('tp-up', 'tp-down'); lastPx = null; }
      if (b && px != null && (px !== lastPx || b.textContent === '—')) {
        b.textContent = px.toFixed(2);
        b.classList.remove('tp-up', 'tp-down');
        if (lastPx != null && px !== lastPx) { void b.offsetWidth; b.classList.add(px > lastPx ? 'tp-up' : 'tp-down'); }
        lastPx = px;
      }
      const st = top.querySelector('.tp-stats'); if (st) paint(/** @type {HTMLElement} */ (st), stats(my));
      paint(main, list(my));
    };
    async function load() {
      const svc = relay();
      // Not while its panel is out of sight (another tab in front).
      if (!my.alive || my.busy || (my.ticks.length && el.offsetParent === null)) return;
      if (!svc) { draw(); return; }
      my.busy = true;
      try {
        // The symbol picked elsewhere when the relay carries it, until one is picked here.
        if (!my.sym) { const f = focused(); my.sym = f && my.symbols.includes(f) ? f : my.symbols[0] || 'SPY'; }
        const q = my.last ? `after=${my.last}` : 'limit=1000';
        const r = await fetch(`/api/stream/${encodeURIComponent(svc)}/tape?symbol=${encodeURIComponent(my.sym)}&watch=1&${q}`, { cache: 'no-store' });
        const v = await r.json().catch(() => ({}));
        if (!r.ok) throw new Error(v.error || `${r.status}`);
        if (v.symbol !== my.sym) return;
        my.symbols = v.symbols || [];
        // A ticker the relay doesn't carry is watched for the tape; an older
        // relay, or a full watch list, says why it can't be.
        my.watching = (v.watching || []).includes(my.sym);
        my.missing = v.watch || (!v.watching && my.symbols.length > 0 && !my.symbols.includes(my.sym) ? `the relay carries only ${my.symbols.join(', ')} (redeploy it to watch others)` : false);
        if (my.missing) { my.ticks = []; return; }
        // A restarted relay counts from zero again.
        if (v.last < my.last) { my.ticks = []; }
        // Arrival times, for the flash (none for the history read first).
        const rt = my.last ? performance.now() : 0;
        for (const x of v.ticks || []) { x.rt = rt; my.ticks.push(x); }
        if (my.ticks.length > 4000) my.ticks = my.ticks.slice(-4000);
        my.last = v.last; my.lastRecord = v.last_record || 0; my.err = '';
      } catch (e) { my.err = `The tape couldn't be read: ${e.message}`; }
      finally { my.busy = false; if (my.alive) draw(); }
    }
    const reset = () => { my.ticks = []; my.last = 0; lastPx = null; load(); };
    const change = e => {
      const t = /** @type {HTMLInputElement} */ (e.target);
      if (t.classList.contains('tp-min')) { my.min = +t.value; draw(); }
    };
    const click = e => {
      const a = /** @type {HTMLElement | null} */ (/** @type {HTMLElement} */ (e.target).closest('[data-tp]'))?.dataset.tp;
      if (a === 'pause') { my.paused = !my.paused; my.hover = 0; draw(); }
      if (a === 'quotes') { my.quotes = !my.quotes; draw(); }
    };
    // Held still while the pointer is over the list, to read it.
    const over = e => { if (main.contains(/** @type {Node} */ (e.target))) my.hover = Date.now(); };
    // The ticker typed, on Enter.
    const find = e => {
      const f = /** @type {HTMLFormElement} */ (e.target); if (!f.matches('.tp-find')) return;
      e.preventDefault();
      const v = /** @type {HTMLInputElement} */ (f.elements.namedItem('s')).value.trim().toUpperCase();
      if (!/^[A-Z][A-Z0-9.]{0,9}$/.test(v)) return;
      /** @type {HTMLElement} */ (document.activeElement)?.blur?.();
      if (v !== my.sym) { my.sym = v; my.missing = false; reset(); } else draw();
    };
    el.addEventListener('submit', find);
    el.addEventListener('change', change); el.addEventListener('click', click); el.addEventListener('mousemove', over);
    const timer = setInterval(load, EVERY);
    load();
    return {
      update() {},
      dispose() { my.alive = false; clearInterval(timer); el.removeEventListener('change', change); el.removeEventListener('submit', find); el.removeEventListener('click', click); el.removeEventListener('mousemove', over); },
    };
  },
});

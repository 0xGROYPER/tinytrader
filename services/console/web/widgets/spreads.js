// Spread builder: an options position from a structure and the live chain,
// priced at the mid and crossing the spread, with its maximum profit and loss,
// breakevens and chance of profit (from the expiry's implied distribution,
// fitted to the whole chain by alpaca-data, unless the fit is poor; else
// lognormal at the at-the-money volatility). "Show on chart" draws it on Candles (the
// shared position, lib/position.js), where its strikes can be dragged.
// Quotes and implied volatility come from a service serving option chains
// (alpaca-data; Alpaca's indicative feed). The underlying is typed in, or
// follows the symbol picked elsewhere. Nothing is sent: it builds and prices.
import { defineWidget } from '../lib/widgets.js';
import { fleet } from '../lib/fleet.js';
import { esc } from '../lib/format.js';
import { focused, theme, setFocus } from '../lib/theme.js';
import { position, publish } from '../lib/position.js';

const chainService = () => (fleet.topo?.services || []).find(s => s.chain)?.id || null;
const N = x => 0.5 * (1 + erf(x / Math.SQRT2));
function erf(x) {
  const s = x < 0 ? -1 : 1, t = 1 / (1 + 0.3275911 * Math.abs(x));
  return s * (1 - ((((1.061405429 * t - 1.453152027) * t + 1.421413741) * t - 0.284496736) * t + 0.254829592) * t * Math.exp(-x * x));
}
/** Black-Scholes value (zero rates). */
function bs(type, S, K, t, v) {
  if (t <= 0 || !(v > 0)) return Math.max(type === 'call' ? S - K : K - S, 0);
  const d1 = (Math.log(S / K) + v * v * t / 2) / (v * Math.sqrt(t)), d2 = d1 - v * Math.sqrt(t);
  const call = S * N(d1) - K * N(d2);
  return type === 'call' ? call : call - S + K;
}

// Structures: legs relative to the at-the-money strike index and the width
// in listed strikes (w). side +1 buys, -1 sells.
const STRUCTURES = {
  'Iron condor': w => [['call', w, -1, 1], ['put', -w, -1, 1], ['call', 2 * w, 1, 1], ['put', -2 * w, 1, 1]],
  'Iron fly': w => [['call', 0, -1, 1], ['put', 0, -1, 1], ['call', w, 1, 1], ['put', -w, 1, 1]],
  'Strangle': w => [['call', w, -1, 1], ['put', -w, -1, 1]],
  'Straddle': w => [['call', 0, 1, 1], ['put', 0, 1, 1]],
  'Call spread': w => [['call', 0, 1, 1], ['call', w, -1, 1]],
  'Put spread': w => [['put', 0, 1, 1], ['put', -w, -1, 1]],
  'Call': w => [['call', 0, 1, 1]],
  'Put': w => [['put', 0, 1, 1]],
};

defineWidget('spreads', {
  title: 'Spread builder',
  mount(el) {
    const my = { sym: focused() || theme()?.symbols?.[0] || 'SPY', chain: null, expiry: null, structure: 'Iron condor', width: 2, err: '', alive: true, at: 0 };
    el.innerHTML = `<div class="sb">
      <div class="sb-top">
        <form class="sb-search"><input class="sb-ticker mono" name="t" placeholder="Ticker" maxlength="10" autocomplete="off" spellcheck="false" aria-label="Underlying"></form>
        <strong class="sb-sym mono"></strong><span class="sb-spot mono muted"></span>
        <select class="sb-exp mono" aria-label="Expiry"></select>
        <button type="button" class="chip-btn sb-show" aria-pressed="false">Show on chart</button>
      </div>
      <div class="sb-structs chips"></div>
      <ul class="sb-legs"></ul>
      <p class="sb-sum mono"></p>
      <p class="sb-msg muted"></p></div>`;
    const $ = s => el.querySelector(s);

    async function loadChain() {
      const svc = chainService();
      if (!svc) { my.err = 'No service serves option chains (alpaca-data).'; render(); return; }
      const sym = my.sym, to = new Date(Date.now() + 120 * 86400e3).toISOString().slice(0, 10);
      try {
        const r = await fetch(`/api/stream/${encodeURIComponent(svc)}/chain?underlying=${encodeURIComponent(sym)}&to=${to}`, { cache: 'no-store' });
        const v = await r.json().catch(() => ({}));
        if (!r.ok) throw new Error(v.error || `${r.status}`);
        if (!my.alive || sym !== my.sym) return;
        my.chain = v; my.at = Date.now(); my.err = '';
        const exps = expiries().map(e => e.expiry);
        if (!exps.includes(my.expiry)) { my.expiry = exps[0] || null; build(); }
        else if (!legs().length) build();
      } catch (e) { my.err = `${sym}: ${e.message}`; my.chain = null; }
      render();
    }
    const today = () => new Date().toLocaleDateString('en-CA', { timeZone: 'America/New_York' });
    const expiries = () => (my.chain?.expiries || []).filter(e => e.expiry >= today());
    const contracts = () => (my.chain?.contracts || []).filter(c => c.expiry === my.expiry);
    const strikesOf = type => [...new Set(contracts().filter(c => !type || c.type === type).map(c => c.strike))].sort((a, b) => a - b);
    const quote = (type, strike) => contracts().find(c => c.type === type && c.strike === strike) || null;
    const atmIV = () => expiries().find(x => x.expiry === my.expiry)?.atm_iv || 0.2;
    const years = () => Math.max((Date.parse(`${my.expiry}T20:00:00Z`) - Date.now()) / (365 * 86400e3), 1 / (365 * 24 * 12));
    const legs = () => position.sym === my.sym ? position.legs : [];
    const nearest = (ks, x) => ks.reduce((b, k) => Math.abs(k - x) < Math.abs(b - x) ? k : b, ks[0]);

    function build() {
      const ks = strikesOf(), spot = my.chain?.spot, make = STRUCTURES[my.structure];
      if (!ks.length || !spot || !make) return;
      const a = ks.indexOf(nearest(ks, spot));
      position.sym = my.sym;
      position.legs = make(my.width).map(([type, o, side, qty]) => ({ type, strike: ks[Math.max(0, Math.min(ks.length - 1, a + o))], side, qty }));
    }

    function analyse() {
      const spot = my.chain?.spot, t = years(), ls = legs().map(l => ({ ...l, q: quote(l.type, l.strike) }));
      if (!spot || !ls.length) return null;
      const mid = q => q && q.bid != null && q.ask ? (q.bid + q.ask) / 2 : null;
      const iv = l => l.q?.iv || atmIV();
      // Signed per-share prices: + paid, - received.
      const net = ls.reduce((s, l) => s + l.side * l.qty * (mid(l.q) ?? bs(l.type, spot, l.strike, t, iv(l))), 0);
      const cross = ls.reduce((s, l) => s + l.side * l.qty * (l.side > 0 ? (l.q?.ask ?? mid(l.q) ?? 0) : (l.q?.bid ?? mid(l.q) ?? 0)), 0);
      const pay = S => ls.reduce((s, l) => s + l.side * l.qty * Math.max(l.type === 'call' ? S - l.strike : l.strike - S, 0), 0) - net;
      const hi = Math.max(spot * 3, ...ls.map(l => l.strike * 1.5)), n = 2400;
      const grid = Array.from({ length: n + 1 }, (_, i) => hi * i / n), pv = grid.map(pay);
      const slopeHi = pay(hi * 2) - pay(hi);
      const maxP = slopeHi > 1e-9 ? Infinity : Math.max(...pv), maxL = slopeHi < -1e-9 ? -Infinity : Math.min(...pv);
      const be = [];
      for (let i = 1; i < pv.length; i++) if ((pv[i - 1] < 0) !== (pv[i] < 0)) be.push(grid[i - 1] + (grid[i] - grid[i - 1]) * (-pv[i - 1]) / (pv[i] - pv[i - 1]));
      // Chance of profit at expiry: from the implied distribution when its fit
      // is usable (linear between its CDF points), else lognormal at the
      // at-the-money implied volatility.
      const dist = expiries().find(x => x.expiry === my.expiry)?.distribution;
      const pts = dist && dist.quality !== 'poor' && Array.isArray(dist.cdf) && dist.cdf.length > 2 ? dist.cdf : null;
      const sd = atmIV() * Math.sqrt(t);
      const cdf = pts ? x => {
        if (x <= pts[0][0]) return pts[0][1] * Math.max(0, x / pts[0][0]);
        const last = pts[pts.length - 1];
        if (x >= last[0]) return 1 - (1 - last[1]) * Math.exp(-(x - last[0]) / Math.max(last[0] * 0.05, 1e-9));
        let i = 1; while (pts[i][0] < x) i++;
        const [k0, p0] = pts[i - 1], [k1, p1] = pts[i];
        return p0 + (p1 - p0) * (x - k0) / (k1 - k0);
      } : x => x <= 0 ? 0 : N((Math.log(x / spot) + sd * sd / 2) / sd);
      let pop = 0;
      for (let i = 1; i < grid.length; i++) if (pay((grid[i - 1] + grid[i]) / 2) > 0) pop += cdf(grid[i]) - cdf(grid[i - 1]);
      return { net, cross, pay, maxP, maxL, be, pop, popFrom: pts ? `implied, ${dist.quality} fit` : 'lognormal', missing: ls.some(l => !l.q) };
    }

    const money = v => !isFinite(v) ? 'unlimited' : `${v < 0 ? '−' : ''}$${Math.abs(v * 100).toFixed(0)}`;
    const day = d => new Date(d + 'T12:00:00Z').toLocaleDateString(undefined, { timeZone: 'UTC', month: 'short', day: 'numeric' });
    function render() {
      const a = analyse();
      // Publish first: Candles draws from it.
      publish({ sym: my.sym, expiry: my.expiry, strikes: { call: strikesOf('call'), put: strikesOf('put') }, net: a?.net ?? 0, be: a?.be ?? [], pay: a?.pay ?? null });
      $('.sb-sym').textContent = my.sym;
      $('.sb-spot').textContent = my.chain?.spot ? my.chain.spot.toFixed(2) : '';
      const exps = expiries(), sel = /** @type {HTMLSelectElement} */ ($('.sb-exp')), key = `${my.sym}|${exps.map(e => e.expiry).join()}`;
      if (sel.dataset.for !== key) {
        sel.innerHTML = exps.map(e => `<option value="${e.expiry}">${day(e.expiry)}${e.implied_move ? ` · ±${(e.implied_move * 100).toFixed(1)}%` : ''}</option>`).join('');
        sel.dataset.for = key;
      }
      if (my.expiry) sel.value = my.expiry;
      $('.sb-show').setAttribute('aria-pressed', String(position.shown));
      $('.sb-structs').innerHTML = Object.keys(STRUCTURES).map(n => `<button type="button" class="chip-btn" data-st="${esc(n)}" aria-pressed="${my.structure === n}">${esc(n)}</button>`).join('')
        + (my.structure === 'Custom' ? '<span class="chip-btn" aria-pressed="true">Custom</span>' : '')
        + `<span class="sb-width muted">width <button type="button" class="chip-btn" data-w="-1" aria-label="Narrower">−</button><b class="mono">${my.width}</b><button type="button" class="chip-btn" data-w="1" aria-label="Wider">+</button></span>`;
      $('.sb-legs').innerHTML = legs().map((l, i) => { const q = quote(l.type, l.strike); return `<li data-i="${i}">
        <button type="button" class="chip-btn ${l.side > 0 ? 'sb-buy' : 'sb-sell'}" data-act="side">${l.side > 0 ? 'Buy' : 'Sell'}</button>
        <span class="mono">${l.qty}×</span>
        <button type="button" class="chip-btn" data-act="down" aria-label="Lower strike">‹</button><b class="mono sb-k">${l.strike}</b><button type="button" class="chip-btn" data-act="up" aria-label="Higher strike">›</button>
        <button type="button" class="chip-btn" data-act="type">${l.type === 'call' ? 'Call' : 'Put'}</button>
        <span class="mono muted">${q?.bid != null ? `${q.bid.toFixed(2)} / ${q.ask?.toFixed(2) ?? '—'}` : 'no quote'}</span>
        <button type="button" class="chip-btn" data-act="remove" aria-label="Remove this leg">×</button></li>`; }).join('')
        + `<li><button type="button" class="chip-btn" data-act="add">Add leg</button></li>`;
      $('.sb-sum').innerHTML = a ? [
        `${a.net >= 0 ? 'Debit' : 'Credit'} <b>$${Math.abs(a.net * 100).toFixed(0)}</b> <span class="muted">($${Math.abs(a.cross * 100).toFixed(0)} crossing)</span>`,
        `max profit <b>${money(a.maxP)}</b>`, `max loss <b>${money(a.maxL)}</b>`,
        `breakeven <b>${a.be.length ? a.be.map(b => b.toFixed(2)).join(' / ') : '—'}</b>`, `chance <b>${(a.pop * 100).toFixed(0)}%</b> <span class="muted">(${a.popFrom})</span>`,
      ].join(' · ') : '';
      $('.sb-msg').textContent = my.err || (a?.missing ? 'A leg has no quote on this expiry; it is priced from the at-the-money volatility.' : '') ||
        (my.chain ? 'Per position: 100 shares a contract. Alpaca indicative quotes.' + (position.shown ? ' Drag the strike lines on Candles to move legs.' : '') : 'Loading the chain…');
    }
    const switchTo = s => { my.sym = s; my.chain = null; my.expiry = null; position.legs = []; render(); loadChain(); };

    el.addEventListener('submit', e => {
      e.preventDefault();
      const s = (/** @type {HTMLInputElement} */ ($('.sb-ticker')).value || '').trim().toUpperCase();
      if (/^[A-Z][A-Z0-9.]{0,9}$/.test(s)) switchTo(s);
    });
    el.addEventListener('change', e => {
      const t = /** @type {HTMLSelectElement} */ (e.target);
      if (!t.classList.contains('sb-exp')) return;
      my.expiry = t.value;
      if (my.structure !== 'Custom') build();
      else for (const l of legs()) l.strike = nearest(strikesOf(l.type), l.strike) ?? l.strike;
      render();
    });
    el.addEventListener('click', e => {
      const b = /** @type {HTMLElement} */ (e.target).closest('button'); if (!b) return;
      if (b.classList.contains('sb-show')) {
        position.shown = !position.shown;
        // Point Candles at this underlying.
        if (position.shown && focused() !== my.sym) { setFocus(my.sym); document.dispatchEvent(new CustomEvent('tt-focus', { detail: my.sym })); }
        render(); return;
      }
      if (b.dataset.st) { my.structure = b.dataset.st; build(); render(); return; }
      if (b.dataset.w) { my.width = Math.max(1, Math.min(20, my.width + +b.dataset.w)); if (my.structure !== 'Custom') build(); render(); return; }
      const act = b.dataset.act; if (!act) return;
      if (act === 'add') {
        const ks = strikesOf('call');
        if (ks.length) { position.sym = my.sym; position.legs = [...legs(), { type: 'call', strike: nearest(ks, my.chain?.spot || 0), side: 1, qty: 1 }]; }
      } else {
        const i = +(/** @type {HTMLElement} */ (b.closest('li[data-i]'))?.dataset.i), l = legs()[i]; if (!l) return;
        if (act === 'side') l.side = l.side > 0 ? -1 : 1;
        if (act === 'type') { l.type = l.type === 'call' ? 'put' : 'call'; l.strike = nearest(strikesOf(l.type), l.strike) ?? l.strike; }
        if (act === 'up' || act === 'down') { const ks = strikesOf(l.type), j = ks.indexOf(l.strike) + (act === 'up' ? 1 : -1); if (ks[j] != null) l.strike = ks[j]; }
        if (act === 'remove') legs().splice(i, 1);
      }
      my.structure = 'Custom'; render();
    });
    // A strike dragged on the chart.
    const edited = (/** @type {Event} */ e) => {
      const { i, strike } = /** @type {CustomEvent} */ (e).detail, l = legs()[i];
      if (l) { l.strike = strike; my.structure = 'Custom'; render(); }
    };
    const follow = (/** @type {Event} */ e) => {
      const s = /** @type {CustomEvent} */ (e).detail;
      if (typeof s === 'string' && s !== my.sym) switchTo(s);
    };
    document.addEventListener('tt-position-edit', edited);
    document.addEventListener('tt-focus', follow);
    render(); loadChain();
    const timer = setInterval(() => { if (Date.now() - my.at > 30e3) loadChain(); }, 5000);
    return {
      update() {},
      dispose() {
        my.alive = false; clearInterval(timer);
        document.removeEventListener('tt-position-edit', edited); document.removeEventListener('tt-focus', follow);
        publish({ shown: false });
      },
    };
  },
});

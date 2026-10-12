// Spread builder: an options position from a structure and the live chain, with
// its payoff at expiry and today, the price at mid and crossing the spread,
// maximum profit and loss, breakevens, probability of profit and net greeks.
// Quotes, implied volatility and greeks come from a service serving option
// chains (alpaca-data; Alpaca's indicative feed). The underlying is typed in,
// or follows the symbol picked elsewhere. Nothing is sent: it builds and prices.
import { defineWidget } from '../lib/widgets.js';
import { fleet } from '../lib/fleet.js';
import { esc } from '../lib/format.js';
import { focused, theme } from '../lib/theme.js';

const chainService = () => (fleet.topo?.services || []).find(s => s.chain)?.id || null;
const css = n => getComputedStyle(document.documentElement).getPropertyValue(n).trim();
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

// Structures: legs relative to the at-the-money strike index (a) and the width
// in listed strikes (w). side +1 buys, -1 sells.
const STRUCTURES = {
  'Long call': w => [['call', 0, 1, 1]],
  'Long put': w => [['put', 0, 1, 1]],
  'Bull call': w => [['call', 0, 1, 1], ['call', w, -1, 1]],
  'Bear put': w => [['put', 0, 1, 1], ['put', -w, -1, 1]],
  'Bull put': w => [['put', 0, -1, 1], ['put', -w, 1, 1]],
  'Bear call': w => [['call', 0, -1, 1], ['call', w, 1, 1]],
  'Short straddle': w => [['call', 0, -1, 1], ['put', 0, -1, 1]],
  'Short strangle': w => [['call', w, -1, 1], ['put', -w, -1, 1]],
  'Iron fly': w => [['call', 0, -1, 1], ['put', 0, -1, 1], ['call', w, 1, 1], ['put', -w, 1, 1]],
  'Iron condor': w => [['call', w, -1, 1], ['put', -w, -1, 1], ['call', 2 * w, 1, 1], ['put', -2 * w, 1, 1]],
  'Risk reversal': w => [['call', w, 1, 1], ['put', -w, -1, 1]],
  'Put ratio 1×2': w => [['put', 0, 1, 1], ['put', -w, -1, 2]],
};

defineWidget('spreads', {
  title: 'Spread builder',
  mount(el) {
    const my = { sym: focused() || theme()?.symbols?.[0] || 'SPY', chain: null, expiry: null, structure: 'Iron condor', width: 2, legs: [], err: '', alive: true, at: 0 };
    el.innerHTML = `<div class="sb">
      <div class="sb-top">
        <form class="sb-search"><input class="sb-ticker mono" name="t" placeholder="Ticker" maxlength="10" autocomplete="off" spellcheck="false" aria-label="Underlying"></form>
        <strong class="sb-sym mono"></strong><span class="sb-spot mono muted"></span>
        <label class="sb-field">Expiry <select class="sb-exp"></select></label>
        <label class="sb-field">Width <input class="sb-width mono" type="number" min="1" max="20" step="1" value="2"> strikes</label>
      </div>
      <div class="sb-structs chips"></div>
      <div class="sb-main"><div class="sb-chart"></div><div class="sb-side"><div class="sb-sum"></div></div></div>
      <div class="sb-legs-wrap"><table class="sb-legs"></table><button type="button" class="chip-btn sb-add">Add leg</button></div>
      <p class="sb-msg muted"></p></div>`;
    const $ = s => el.querySelector(s);

    async function loadChain() {
      const svc = chainService();
      if (!svc) { my.err = 'No service serves option chains (alpaca-data).'; render(); return; }
      const to = new Date(Date.now() + 120 * 86400e3).toISOString().slice(0, 10);
      try {
        const r = await fetch(`/api/stream/${encodeURIComponent(svc)}/chain?underlying=${encodeURIComponent(my.sym)}&to=${to}`, { cache: 'no-store' });
        const v = await r.json().catch(() => ({}));
        if (!r.ok) throw new Error(v.error || `${r.status}`);
        if (!my.alive) return;
        my.chain = v; my.at = Date.now(); my.err = '';
        const exps = [...new Set(v.contracts.map(c => c.expiry))].sort();
        const today = new Date().toLocaleDateString('en-CA', { timeZone: 'America/New_York' });
        if (!exps.includes(my.expiry)) my.expiry = exps.find(e => e >= today) || exps[0] || null;
        if (!my.legs.length) build();
      } catch (e) { my.err = `${my.sym}: ${e.message}`; my.chain = null; }
      render();
    }
    const contracts = () => (my.chain?.contracts || []).filter(c => c.expiry === my.expiry);
    const strikes = () => [...new Set(contracts().map(c => c.strike))].sort((a, b) => a - b);
    const quote = (type, strike) => contracts().find(c => c.type === type && c.strike === strike) || null;
    const atmIV = () => (my.chain?.expiries || []).find(x => x.expiry === my.expiry)?.atm_iv || 0.2;
    const years = () => Math.max((Date.parse(`${my.expiry}T20:00:00Z`) - Date.now()) / (365 * 86400e3), 1 / (365 * 24 * 12));

    function build() {
      const ks = strikes(), spot = my.chain?.spot;
      if (!ks.length || !spot) { my.legs = []; return; }
      const a = ks.reduce((b, k, i) => Math.abs(k - spot) < Math.abs(ks[b] - spot) ? i : b, 0);
      const make = STRUCTURES[my.structure];
      my.legs = make ? make(my.width).map(([type, o, side, qty]) => ({ type, strike: ks[Math.max(0, Math.min(ks.length - 1, a + o))], side, qty })) : my.legs;
    }

    function analyse() {
      const spot = my.chain?.spot, t = years(), legs = my.legs.map(l => ({ ...l, q: quote(l.type, l.strike) }));
      if (!spot || !legs.length) return null;
      const mid = q => q && q.bid != null && q.ask ? (q.bid + q.ask) / 2 : null;
      const iv = l => l.q?.iv || atmIV();
      // Signed per-share prices: + paid, - received.
      const net = legs.reduce((s, l) => s + l.side * l.qty * (mid(l.q) ?? bs(l.type, spot, l.strike, t, iv(l))), 0);
      const cross = legs.reduce((s, l) => s + l.side * l.qty * (l.side > 0 ? (l.q?.ask ?? mid(l.q) ?? 0) : (l.q?.bid ?? mid(l.q) ?? 0)), 0);
      const atExpiry = S => legs.reduce((s, l) => s + l.side * l.qty * Math.max(l.type === 'call' ? S - l.strike : l.strike - S, 0), 0) - net;
      const today = S => legs.reduce((s, l) => s + l.side * l.qty * bs(l.type, S, l.strike, t, iv(l)), 0) - net;
      // Wide grid for the extremes and breakevens.
      const hi = Math.max(spot * 3, ...legs.map(l => l.strike * 1.5)), n = 2400;
      const grid = Array.from({ length: n + 1 }, (_, i) => hi * i / n);
      const pay = grid.map(atExpiry);
      const slopeHi = atExpiry(hi * 2) - atExpiry(hi);
      const maxP = slopeHi > 1e-9 ? Infinity : Math.max(...pay), maxL = slopeHi < -1e-9 ? -Infinity : Math.min(...pay);
      const be = [];
      for (let i = 1; i < pay.length; i++) if ((pay[i - 1] < 0) !== (pay[i] < 0)) be.push(grid[i - 1] + (grid[i] - grid[i - 1]) * (-pay[i - 1]) / (pay[i] - pay[i - 1]));
      // Probability of profit at expiry, lognormal at the at-the-money implied volatility.
      const v = atmIV(), sd = v * Math.sqrt(t);
      let pop = 0;
      for (let i = 1; i < grid.length; i++) {
        const S = (grid[i - 1] + grid[i]) / 2; if (S <= 0) continue;
        const z = x => (Math.log(x / spot) + sd * sd / 2) / sd;
        if (atExpiry(S) > 0) pop += N(z(grid[i])) - N(z(Math.max(grid[i - 1], 1e-9)));
      }
      const greek = k => legs.reduce((s, l) => s + l.side * l.qty * (l.q?.[k] ?? 0), 0) * 100;
      return { spot, t, legs, net, cross, atExpiry, today, maxP, maxL, be, pop, sd,
        delta: greek('delta'), gamma: greek('gamma'), theta: greek('theta'), vega: greek('vega'), missing: legs.some(l => !l.q) };
    }

    const money = v => !isFinite(v) ? (v > 0 ? 'Unlimited' : 'Unlimited loss') : `${v < 0 ? '−' : ''}$${Math.abs(v * 100).toFixed(0)}`;
    function chart(a) {
      const W = 560, H = 260, pl = 48, pr = 12, pt = 14, pb = 28;
      const lo = a.spot * Math.exp(-3.2 * a.sd - 0.01), hi = a.spot * Math.exp(3.2 * a.sd + 0.01);
      const xs = Array.from({ length: 161 }, (_, i) => lo + (hi - lo) * i / 160);
      const e = xs.map(a.atExpiry), d = xs.map(a.today);
      const all = [...e, ...d].filter(isFinite);
      let y0 = Math.min(...all, 0), y1 = Math.max(...all, 0); const padY = (y1 - y0) * 0.08 || 1; y0 -= padY; y1 += padY;
      const sx = x => pl + (x - lo) / (hi - lo) * (W - pl - pr), sy = y => pt + (y1 - y) / (y1 - y0) * (H - pt - pb);
      const path = ys => xs.map((x, i) => `${i ? 'L' : 'M'}${sx(x).toFixed(1)},${sy(ys[i]).toFixed(1)}`).join('');
      const ok = css('--ok') || '#2f9e6b', crit = css('--crit') || '#c0392b', acc = css('--accent') || '#5b6fd6', mut = css('--muted') || '#888', line = css('--line') || '#ccc';
      const move = a.spot * a.sd * 0.798;
      const ticks = [y0, 0, y1].map(v => `<text x="${pl - 6}" y="${sy(v) + 4}" text-anchor="end" class="sb-ax">${money(v)}</text>`).join('');
      const xt = [lo, a.spot, hi].map(v => `<text x="${sx(v)}" y="${H - 8}" text-anchor="middle" class="sb-ax">${v.toFixed(v < 100 ? 2 : 0)}</text>`).join('');
      // Profit and loss areas under the expiry curve.
      const area = (sign, col) => `<path d="${path(e.map(v => sign > 0 ? Math.max(v, 0) : Math.min(v, 0)))}L${sx(hi)},${sy(0)}L${sx(lo)},${sy(0)}Z" fill="${col}" opacity=".14"/>`;
      return `<svg viewBox="0 0 ${W} ${H}" class="sb-svg" role="img" aria-label="Payoff">
        <rect x="${sx(a.spot - move)}" y="${pt}" width="${sx(a.spot + move) - sx(a.spot - move)}" height="${H - pt - pb}" fill="${acc}" opacity=".07"/>
        ${area(1, ok)}${area(-1, crit)}
        <line x1="${pl}" x2="${W - pr}" y1="${sy(0)}" y2="${sy(0)}" stroke="${line}"/>
        <line x1="${sx(a.spot)}" x2="${sx(a.spot)}" y1="${pt}" y2="${H - pb}" stroke="${mut}" stroke-dasharray="2 3"/>
        ${a.be.filter(b => b > lo && b < hi).map(b => `<line x1="${sx(b)}" x2="${sx(b)}" y1="${pt}" y2="${H - pb}" stroke="${acc}" stroke-dasharray="3 3"/><text x="${sx(b)}" y="${pt + 10}" text-anchor="middle" class="sb-ax">${b.toFixed(2)}</text>`).join('')}
        <path d="${path(d)}" fill="none" stroke="${acc}" stroke-width="1.5" stroke-dasharray="5 4"/>
        <path d="${path(e)}" fill="none" stroke="currentColor" stroke-width="2"/>
        ${ticks}${xt}
      </svg>
      <p class="sb-legend muted"><span><b class="sb-key"></b>at expiry</span><span><b class="sb-key sb-dash"></b>today</span><span><b class="sb-key sb-band"></b>expected move ±${(move / a.spot * 100).toFixed(1)}%</span></p>`;
    }

    function render() {
      $('.sb-sym').textContent = my.sym;
      $('.sb-spot').textContent = my.chain?.spot ? my.chain.spot.toFixed(2) : '';
      const exps = [...new Set((my.chain?.contracts || []).map(c => c.expiry))].sort();
      const sel = $('.sb-exp');
      if (sel.dataset.for !== `${my.sym}|${exps.join()}`) {
        sel.innerHTML = exps.map(e => `<option value="${e}">${e}</option>`).join('');
        sel.dataset.for = `${my.sym}|${exps.join()}`;
      }
      if (my.expiry) sel.value = my.expiry;
      $('.sb-structs').innerHTML = [...Object.keys(STRUCTURES), 'Custom'].map(n => `<button type="button" class="chip-btn" data-st="${esc(n)}" aria-pressed="${my.structure === n}">${esc(n)}</button>`).join('');
      const ks = strikes(), a = analyse();
      $('.sb-legs').innerHTML = `<thead><tr><th>Side</th><th>Qty</th><th>Type</th><th>Strike</th><th>Bid</th><th>Ask</th><th>Mid</th><th>IV</th><th>Delta</th><th></th></tr></thead><tbody>${
        my.legs.map((l, i) => { const q = quote(l.type, l.strike); return `<tr data-i="${i}">
          <td><button type="button" class="chip-btn sb-side-btn ${l.side > 0 ? 'sb-buy' : 'sb-sell'}" data-act="side">${l.side > 0 ? 'Buy' : 'Sell'}</button></td>
          <td><input type="number" class="sb-qty mono" min="1" max="99" value="${l.qty}" data-act="qty"></td>
          <td><button type="button" class="chip-btn" data-act="type">${l.type === 'call' ? 'Call' : 'Put'}</button></td>
          <td><select class="mono" data-act="strike">${ks.map(k => `<option value="${k}"${k === l.strike ? ' selected' : ''}>${k}</option>`).join('')}</select></td>
          <td class="mono">${q?.bid?.toFixed(2) ?? '—'}</td><td class="mono">${q?.ask?.toFixed(2) ?? '—'}</td><td class="mono">${q?.mid?.toFixed(2) ?? '—'}</td>
          <td class="mono">${q?.iv ? (q.iv * 100).toFixed(1) + '%' : '—'}</td><td class="mono">${q?.delta != null ? q.delta.toFixed(2) : '—'}</td>
          <td><button type="button" class="chip-btn" data-act="remove" title="Remove this leg">×</button></td></tr>`; }).join('')}</tbody>`;
      $('.sb-chart').innerHTML = a ? chart(a) : '';
      $('.sb-sum').innerHTML = a ? [
        [a.net >= 0 ? 'Debit (mid)' : 'Credit (mid)', `$${Math.abs(a.net * 100).toFixed(0)}`],
        [a.cross >= 0 ? 'Debit, crossing' : 'Credit, crossing', `$${Math.abs(a.cross * 100).toFixed(0)}`],
        ['Max profit', money(a.maxP)], ['Max loss', money(a.maxL)],
        ['Breakevens', a.be.length ? a.be.map(b => b.toFixed(2)).join(' · ') : '—'],
        ['Chance of profit', `${(a.pop * 100).toFixed(0)}%`],
        ['Delta', a.delta.toFixed(1)], ['Gamma', a.gamma.toFixed(2)], ['Theta / day', `$${a.theta.toFixed(0)}`], ['Vega', `$${a.vega.toFixed(0)}`],
      ].map(([k, v]) => `<div class="sb-cell"><span class="sb-k">${k}</span><span class="sb-v mono">${v}</span></div>`).join('') : '';
      const age = my.at ? Math.round((Date.now() - my.at) / 1000) : null;
      $('.sb-msg').textContent = my.err || (a?.missing ? 'A leg has no quote on this expiry; it is priced from the at-the-money volatility.' : '') ||
        (my.chain ? `Quotes ${age}s old (Alpaca indicative feed); per 1 position, 100 shares a contract. Chance of profit assumes the at-the-money implied volatility.` : 'Loading the chain…');
    }

    el.addEventListener('submit', e => {
      e.preventDefault();
      const s = (/** @type {HTMLInputElement} */ ($('.sb-ticker')).value || '').trim().toUpperCase();
      if (!/^[A-Z][A-Z0-9.]{0,9}$/.test(s)) return;
      my.sym = s; my.chain = null; my.legs = []; my.expiry = null; render(); loadChain();
    });
    el.addEventListener('change', e => {
      const t = /** @type {HTMLElement} */ (e.target);
      if (t.classList.contains('sb-exp')) { my.expiry = /** @type {HTMLSelectElement} */ (t).value; if (my.structure !== 'Custom') build(); else my.legs = my.legs.filter(l => strikes().includes(l.strike)); render(); return; }
      if (t.classList.contains('sb-width')) { my.width = Math.max(1, Math.min(20, +(/** @type {HTMLInputElement} */ (t)).value || 1)); if (my.structure !== 'Custom') build(); render(); return; }
      const row = t.closest('tr[data-i]'); if (!row) return;
      const l = my.legs[+row.dataset.i];
      if (t.dataset.act === 'qty') l.qty = Math.max(1, Math.min(99, +(/** @type {HTMLInputElement} */ (t)).value || 1));
      if (t.dataset.act === 'strike') l.strike = +(/** @type {HTMLSelectElement} */ (t)).value;
      my.structure = 'Custom'; render();
    });
    el.addEventListener('click', e => {
      const b = /** @type {HTMLElement} */ (e.target).closest('button'); if (!b) return;
      if (b.dataset.st) { my.structure = b.dataset.st; if (my.structure !== 'Custom') build(); render(); return; }
      if (b.classList.contains('sb-add')) {
        const ks = strikes(), spot = my.chain?.spot || 0;
        const k = ks.reduce((x, y) => Math.abs(y - spot) < Math.abs(x - spot) ? y : x, ks[0]);
        if (k != null) my.legs.push({ type: 'call', strike: k, side: 1, qty: 1 });
        my.structure = 'Custom'; render(); return;
      }
      const row = b.closest('tr[data-i]'); if (!row) return;
      const i = +row.dataset.i, l = my.legs[i];
      if (b.dataset.act === 'side') l.side = -l.side;
      if (b.dataset.act === 'type') l.type = l.type === 'call' ? 'put' : 'call';
      if (b.dataset.act === 'remove') my.legs.splice(i, 1);
      my.structure = 'Custom'; render();
    });
    const follow = (/** @type {Event} */ e) => {
      const s = /** @type {CustomEvent} */ (e).detail;
      if (typeof s === 'string' && s !== my.sym) { my.sym = s; my.chain = null; my.legs = []; my.expiry = null; render(); loadChain(); }
    };
    document.addEventListener('tt-focus', follow);
    render(); loadChain();
    const timer = setInterval(() => { if (Date.now() - my.at > 30e3) loadChain(); }, 5000);
    return {
      update() {},
      dispose() { my.alive = false; clearInterval(timer); document.removeEventListener('tt-focus', follow); },
    };
  },
});

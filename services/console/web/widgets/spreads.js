// Spread builder: an options position from a structure and the live chain,
// one line a leg, summed up as what it costs, what it can lose and make, and
// its chance of profit (from the expiry's implied distribution, fitted to the
// whole chain by alpaca-data, unless the fit is poor; else lognormal at the
// at-the-money volatility). Quotes come from a service serving option chains
// (alpaca-data), which works out implied volatilities from them. The
// underlying is typed in, or follows the symbol picked elsewhere; a stock
// clicked in the screener gets a suggested position (lib/recommend.js) with
// the reason. Trade opens a review: the account, the contracts and the limit,
// then Send to that account's order gateway as the console's trade client
// (POST /api/orders/{account}). Working orders are listed with Cancel.
import { defineWidget, paint } from '../lib/widgets.js';
import { fleet, svcState } from '../lib/fleet.js';
import { esc } from '../lib/format.js';
import { focused, theme } from '../lib/theme.js';
import { position, publish, wanted } from '../lib/position.js';
import { recommend } from '../lib/recommend.js';
import { GROUPS, STRUCTURES, structureOptions, identify, whatIs } from '../lib/structures.js';
import { notSentText } from '../lib/orders.js';

const store = (k, v) => { try { if (v === undefined) return localStorage.getItem(k); localStorage.setItem(k, v); } catch (_) { /* private mode */ } return null; };
// Each account's open orders as the broker has them (alpaca-account), however placed: null if unknown.
const openOrders = () => {
  const out = new Map();
  for (const s of (fleet.topo?.services || []).filter(x => x.accounts)) {
    for (const a of svcState(s.id).accounts?.accounts || []) if (a.name && a.orders !== undefined) out.set(a.name, a.orders);
  }
  return out;
};
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

defineWidget('spreads', {
  title: 'Spread builder',
  mount(el) {
    const my = { sym: focused() || theme()?.symbols?.[0] || 'SPY', chain: null, expiry: null, structure: 'Iron condor', width: 2, err: '', alive: true, at: 0,
      // A suggestion waiting for its chain, and the one shown.
      ask: /** @type {any} */ (null), why: '',
      // The order ticket.
      acct: store('tt-sb-account') || '', payoff: store('tt-sb-payoff') === '1',
      // What's typed to confirm a live order: kept here, as the review is redrawn on every refresh.
      typed: '', limit: /** @type {number | null} */ (null), review: false, sending: false, result: '',
      orders: /** @type {any} */ (null), ordersAt: 0,
      // Positions held, loaded from Accounts: their legs carry `cost`, the entry price.
      held: /** @type {any} */ (null), heldAsk: /** @type {any} */ (null) };
    el.innerHTML = `<div class="sb">
      <div class="sb-top">
        <form class="sb-search"><input class="sb-ticker mono" name="t" placeholder="Ticker" maxlength="10" autocomplete="off" spellcheck="false" aria-label="Underlying"></form>
        <strong class="sb-sym mono"></strong><span class="sb-spot mono muted"></span>
        <select class="sb-exp mono" aria-label="Expiry"></select>
        <select class="sb-st" aria-label="Structure"></select>
        <span class="sb-views"><button type="button" class="chip-btn sb-paybtn" aria-pressed="false">Payoff</button></span>
      </div>
      <p class="sb-why muted"></p>
      <div class="sb-table"><table class="sb-legs"></table></div>
      <div class="sb-bar"><p class="sb-sum mono"></p><button type="button" class="btn sb-trade" aria-expanded="false">Trade</button></div>
      <div class="sb-ticket"></div><div class="sb-orders"></div>
      <div class="sb-pay"></div>
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
        if (my.ask?.sym === sym) { suggest(my.ask); my.ask = null; render(); return; }
        if (my.heldAsk?.sym === sym) { hold(my.heldAsk); my.heldAsk = null; render(); return; }
        const exps = expiries().map(e => e.expiry);
        if (!exps.includes(my.expiry)) { my.expiry = exps[0] || null; build(); }
        else if (!legs().length) build();
      } catch (e) { my.err = `${sym}: ${e.message}`; my.chain = null; }
      render();
    }
    const today = () => new Date().toLocaleDateString('en-CA', { timeZone: 'America/New_York' });
    // Today's expiry until a quarter past the close.
    const settled = () => +new Date().toLocaleTimeString('en-GB', { timeZone: 'America/New_York', hour: '2-digit', minute: '2-digit' }).replace(':', '') >= 1615;
    const expiries = () => (my.chain?.expiries || []).filter(e => e.expiry > today() || (e.expiry === today() && !settled()));
    const contracts = () => (my.chain?.contracts || []).filter(c => c.expiry === my.expiry);
    const strikesOf = type => [...new Set(contracts().filter(c => !type || c.type === type).map(c => c.strike))].sort((a, b) => a - b);
    const quote = (type, strike) => contracts().find(c => c.type === type && c.strike === strike) || null;
    const atmIV = () => expiries().find(x => x.expiry === my.expiry)?.atm_iv || null;
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
      // A leg without a two-sided quote: at its own implied volatility, else the at-the-money one.
      const iv = l => l.q?.iv || atmIV();
      if (ls.some(l => l.cost == null && mid(l.q) == null && !iv(l))) return null;
      // Signed per-share prices: + paid, - received; a held leg at what was paid for it.
      const now = l => mid(l.q) ?? bs(l.type, spot, l.strike, t, iv(l));
      const net = ls.reduce((s, l) => s + l.side * l.qty * (l.cost ?? now(l)), 0);
      const worth = ls.reduce((s, l) => s + l.side * l.qty * now(l), 0), held = ls.some(l => l.cost != null);
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
      const sd = (atmIV() || 0.3) * Math.sqrt(t);
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
      // Worth today at an underlying price: each leg at its own implied volatility, less what was paid.
      const today = S => ls.reduce((v, l) => v + l.side * l.qty * bs(l.type, S, l.strike, t, iv(l)), 0) - net;
      return { net, worth, held, cross, pay, today, spot, sd: (atmIV() || 0.3) * Math.sqrt(t), strikes: ls.map(l => l.strike), maxP, maxL, be, pop, popFrom: pts ? `implied, ${dist.quality} fit` : 'lognormal', missing: ls.some(l => !l.q) };
    }

    const money = v => !isFinite(v) ? 'unlimited' : `${v < 0 ? '−' : ''}$${Math.abs(v * 100).toFixed(0)}`;
    const day = d => new Date(d + 'T12:00:00Z').toLocaleDateString(undefined, { timeZone: 'UTC', month: 'short', day: 'numeric' });
    function render() {
      const a = analyse();
      // Publish first: Candles draws from it.
      publish({ sym: my.sym, expiry: my.expiry, strikes: { call: strikesOf('call'), put: strikesOf('put') }, net: a?.net ?? 0, be: a?.be ?? [], pay: a?.pay ?? null });
      $('.sb-sym').textContent = my.sym;
      $('.sb-spot').textContent = my.chain?.spot ? my.chain.spot.toFixed(2) : '';
      $('.sb-spot').title = my.chain?.spot_from === 'parity' ? 'From the options (put-call parity)' : '';
      const exps = expiries(), sel = /** @type {HTMLSelectElement} */ ($('.sb-exp')), key = `${my.sym}|${exps.map(e => e.expiry).join()}`;
      if (sel.dataset.for !== key) {
        sel.innerHTML = exps.map(e => `<option value="${e.expiry}">${day(e.expiry)}</option>`).join('');
        sel.dataset.for = key;
      }
      if (my.expiry) sel.value = my.expiry;
      $('.sb-paybtn').setAttribute('aria-pressed', String(my.payoff));
      // The structure: a preset, or what the legs came from (a suggestion, held, edited).
      const preset = !!STRUCTURES[my.structure] && !my.why;
      // Edited legs keep a name when their shape has one (a bull call spread
      // with its strikes moved is still one), so you can see what you built.
      const shape = my.structure === 'Custom' ? identify(legs()) : null;
      const label = my.structure === 'Custom' ? (shape ? `${shape} (edited)` : 'Custom') : my.structure;
      $('.sb-st').innerHTML = (preset ? '' : `<option selected>${esc(label)}</option>`)
        + structureOptions(preset ? my.structure : '', esc);
      // Held legs: the ticket is away until none are left (an order for them would open more).
      const heldNote = a?.held ? `${my.held ? `Your positions on ${esc(my.held.account)}` : 'Includes positions held'}, at their entry prices. Change a strike or type to try an adjustment.` : '';
      const shapeNote = shape ? `<b>${esc(shape)}.</b> ${esc(whatIs(shape).replace(/ w /g, ' ').replace(/ 2w /g, ' ') || 'Its legs, as you set them.')}` : '';
      $('.sb-why').innerHTML = my.why ? `<b>${esc(my.structure)}.</b> ${esc(my.why)}` : heldNote || shapeNote;
      // The legs as a positions table, like the broker's: each contract, its mid,
      // signed quantity (− sells), market value and delta in shares; the strike
      // steps with ‹ ›, C/P switches type, the quantity steps through zero to flip side.
      const usd = v => `${v < 0 ? '−' : ''}$${Math.abs(v).toFixed(2)}`;
      const midOf = q => q && q.bid != null && q.ask ? (q.bid + q.ask) / 2 : null;
      let mv = 0, dl = 0, known = true;
      const rows = legs().map((l, i) => {
        const q = quote(l.type, l.strike), n = l.side * l.qty, m = midOf(q);
        const value = m == null ? null : n * m * 100, delta = q?.delta == null ? null : n * q.delta * 100;
        if (value == null) known = false; else mv += value;
        if (delta != null) dl += delta;
        return `<tr data-i="${i}">
          <td class="sb-asset">${q?.symbol ? `<button type="button" class="sb-occ mono" data-occ="${esc(q.symbol)}" title="Chart today's mid on Candles (SPY and XSP options, from Databento)">${esc(q.symbol)}</button>` : `<span class="mono">${esc(`${my.sym} ${l.strike}${l.type[0].toUpperCase()}`)}</span>`}
            <span class="sb-steps"><button type="button" class="sb-mini" data-act="down" aria-label="Lower strike" title="Lower strike">‹</button><button type="button" class="sb-mini" data-act="type" title="Switch call / put">${l.type === 'call' ? 'C' : 'P'}</button><button type="button" class="sb-mini" data-act="up" aria-label="Higher strike" title="Higher strike">›</button></span></td>
          <td class="num sb-c-price" data-label="Price">${m == null ? '<span class="muted">no quote</span>' : usd(m)}<small class="muted">${q?.bid != null ? `${q.bid.toFixed(2)} × ${q.ask?.toFixed(2) ?? '—'}` : ''}</small></td>
          <td class="num sb-c-qty" data-label="Qty"><span class="sb-qty-cell"><button type="button" class="sb-mini" data-act="less" aria-label="One fewer" title="One fewer (shift: the whole position one unit smaller)">−</button><b class="mono">${n}</b><button type="button" class="sb-mini" data-act="more" aria-label="One more" title="One more (shift: the whole position one unit bigger)">+</button></span></td>
          <td class="num sb-c-value" data-label="Value">${value == null ? '—' : usd(value)}</td>
          <td class="num muted sb-c-delta" data-label="Δ">${delta == null ? '—' : delta.toFixed(0)}</td>
          <td class="sb-c-x"><button type="button" class="sb-x" data-act="remove" aria-label="Remove this leg" title="Remove">×</button></td></tr>`;
      }).join('');
      $('.sb-legs').innerHTML = `<thead><tr><th>Asset</th><th class="num">Price</th><th class="num">Qty</th><th class="num">Market value</th><th class="num" title="Delta in shares">Δ</th><th></th></tr></thead>
        <tbody>${rows}</tbody>
        <tfoot><tr><td><span class="sb-adds"><button type="button" class="chip-btn" data-act="add">Add leg</button><select class="sb-addsp" aria-label="Add a spread" title="Add a vertical spread to these legs (a credit spread goes one width out of the money, a debit spread at the money)"><option value="">+ Add spread…</option>${Object.keys(GROUPS[0].items).map(n => `<option>${esc(n)}</option>`).join('')}</select></span></td><td></td><td></td>
          <td class="num" data-label="Net"><b>${known && legs().length ? usd(mv) : ''}</b></td><td class="num muted" data-label="Δ">${legs().length ? dl.toFixed(0) : ''}</td><td></td></tr></tfoot>`;
      payoff(a);
      // The summary: what it costs, what it can lose and make, and the chance; the rest on hover.
      const dollars = v => `$${Math.abs(v * 100).toFixed(0)}`, credit = a && a.net < 0;
      $('.sb-sum').innerHTML = a ? [
        a.held ? `Paid <b>${dollars(a.net)}</b> · now <b>${dollars(a.worth)}</b> <span class="${a.worth - a.net >= 0 ? 'sb-good' : 'sb-bad'}">(${a.worth - a.net >= 0 ? '+' : '−'}${dollars(a.worth - a.net)})</span>`
          : `${credit ? 'Credit' : 'Debit'} <b>${dollars(a.net)}</b>`,
        `risk <b>${isFinite(a.maxL) ? dollars(a.maxL) : 'unlimited'}</b>`,
        ...(credit && isFinite(a.maxP) && Math.abs(a.maxP + a.net) < 0.005 ? [] : [`up to <b>${isFinite(a.maxP) ? dollars(a.maxP) : 'unlimited'}</b>`]),
        `<b>${(a.pop * 100).toFixed(0)}%</b>`,
      ].join(' · ') : '';
      $('.sb-sum').title = a ? [
        `${credit ? 'Credit' : 'Debit'} $${Math.abs(a.cross * 100).toFixed(0)} crossing the spread`,
        `breakeven ${a.be.length ? a.be.map(b => b.toFixed(2)).join(' / ') : '—'}`,
        `delta ${dl.toFixed(0)} shares`,
        `chance of profit at expiry (${a.popFrom})`,
        `per position, 100 shares a contract; ${({ opra: 'Databento OPRA quotes', mixed: `Databento OPRA quotes for ${my.chain?.opra_quotes} contracts, Alpaca indicative for the rest` })[my.chain?.quotes] || 'Alpaca indicative quotes'}`,
      ].join('\n') : '';
      const tb = /** @type {HTMLButtonElement} */ ($('.sb-trade'));
      tb.hidden = !a || !!a.held;
      tb.setAttribute('aria-expanded', String(my.review));
      const open = [...openOrders().values()].reduce((n, o) => n + (o?.length || 0), 0);
      tb.textContent = open ? `Trade · ${open} open` : 'Trade';
      tb.title = open ? `${open} open order${open > 1 ? 's' : ''} across the accounts (see Accounts)` : '';
      ticket(a);
      $('.sb-msg').textContent = my.err || (a?.missing ? 'A leg has no quote on this expiry; it is priced from its implied volatility.' : '') || (my.chain ? '' : 'Loading the chain…');
    }

    // The payoff chart: P&L at expiry (solid, green above zero and red below) and
    // today (dashed, each leg at its implied volatility) across underlying prices
    // spanning the strikes and about 2.5 standard deviations either side, with
    // the price, breakevens and the one-standard-deviation range marked. It is
    // drawn from the legs alone, so it works for any structure.
    const PH = 190, PAD = { l: 52, r: 12, t: 10, b: 22 };
    let drawn = null;
    function payoff(a) {
      const box = /** @type {HTMLElement} */ ($('.sb-pay'));
      drawn = a;
      if (!a || !my.payoff) { box.innerHTML = ''; return; }
      const W = Math.max(280, box.clientWidth || 600), w = W - PAD.l - PAD.r, h = PH - PAD.t - PAD.b;
      // The strikes and the price, with a margin of 2.5 standard deviations or 60% of the strikes' spread.
      const kLo = Math.min(a.spot, ...a.strikes), kHi = Math.max(a.spot, ...a.strikes);
      const m = Math.max(a.spot * a.sd * 2.5, (kHi - kLo) * 0.6, a.spot * 0.004);
      const lo = Math.max(0, kLo - m), hi = kHi + m;
      const n = 240, xs = Array.from({ length: n + 1 }, (_, i) => lo + (hi - lo) * i / n);
      const ex = xs.map(a.pay), td = xs.map(a.today);
      let yMin = Math.min(0, ...ex, ...td), yMax = Math.max(0, ...ex, ...td);
      const pad = (yMax - yMin) * 0.08 || 1; yMin -= pad; yMax += pad;
      const X = v => PAD.l + (v - lo) / (hi - lo) * w, Y = v => PAD.t + (yMax - v) / (yMax - yMin) * h;
      const line = ys => ys.map((y, i) => `${i ? 'L' : 'M'}${X(xs[i]).toFixed(1)},${Y(y).toFixed(1)}`).join('');
      // Profit and loss areas between the expiry line and zero.
      const area = sign => `M${X(lo).toFixed(1)},${Y(0).toFixed(1)}` + ex.map((y, i) => `L${X(xs[i]).toFixed(1)},${Y(sign > 0 ? Math.max(y, 0) : Math.min(y, 0)).toFixed(1)}`).join('') + `L${X(hi).toFixed(1)},${Y(0).toFixed(1)}Z`;
      const money = v => `${v < 0 ? '−' : ''}$${Math.abs(v * 100) >= 1000 ? (Math.abs(v * 100) / 1000).toFixed(1) + 'k' : Math.abs(v * 100).toFixed(0)}`;
      // Zero, the best and the worst, unless too close to zero to read.
      const ticks = [0, ...[yMax - pad, yMin + pad].filter(v => Math.abs(Y(v) - Y(0)) > 14)];
      const sd1 = [a.spot * Math.exp(-a.sd), a.spot * Math.exp(a.sd)];
      box.innerHTML = `<svg class="sb-chart" viewBox="0 0 ${W} ${PH}" width="${W}" height="${PH}" role="img" aria-label="Profit and loss at expiry and today across underlying prices">
        <rect x="${X(sd1[0]).toFixed(1)}" y="${PAD.t}" width="${(X(sd1[1]) - X(sd1[0])).toFixed(1)}" height="${h}" class="pc-band"/>
        <path d="${area(1)}" class="pc-win"/><path d="${area(-1)}" class="pc-lose"/>
        <line x1="${PAD.l}" x2="${W - PAD.r}" y1="${Y(0).toFixed(1)}" y2="${Y(0).toFixed(1)}" class="pc-zero"/>
        ${ticks.map(v => `<text x="${PAD.l - 6}" y="${(Y(v) + 4).toFixed(1)}" class="pc-tick" text-anchor="end">${money(v)}</text>`).join('')}
        ${[lo, a.spot, hi].map((v, i) => `<text x="${X(v).toFixed(1)}" y="${PH - 6}" class="pc-tick" text-anchor="${['start', 'middle', 'end'][i]}">${v.toFixed(v < 100 ? 2 : 0)}</text>`).join('')}
        ${a.be.filter(b => b > lo && b < hi).map(b => `<line x1="${X(b).toFixed(1)}" x2="${X(b).toFixed(1)}" y1="${PAD.t}" y2="${PAD.t + h}" class="pc-be"/>`).join('')}
        <line x1="${X(a.spot).toFixed(1)}" x2="${X(a.spot).toFixed(1)}" y1="${PAD.t}" y2="${PAD.t + h}" class="pc-spot"/>
        <path d="${line(td)}" class="pc-today"/><path d="${line(ex)}" class="pc-expiry"/>
        <g class="pc-hover" hidden><line class="pc-cross" y1="${PAD.t}" y2="${PAD.t + h}"/><circle r="3.5" class="pc-dot"/></g>
      </svg>
      <p class="sb-paykey muted"><span class="pc-k pc-k-exp"></span>At expiry <span class="pc-k pc-k-today"></span>Today <span class="pc-k pc-k-band"></span>±1 SD <span class="sb-payread mono"></span></p>`;
      const svg = /** @type {SVGSVGElement} */ (box.querySelector('svg')), read = box.querySelector('.sb-payread'), hov = /** @type {SVGGElement} */ (box.querySelector('.pc-hover'));
      svg.onmousemove = e => {
        const r = svg.getBoundingClientRect(), px = (e.clientX - r.left) * W / r.width;
        if (px < PAD.l || px > W - PAD.r) { hov.setAttribute('hidden', ''); read.textContent = ''; return; }
        const S = lo + (px - PAD.l) / w * (hi - lo), pe = a.pay(S), pt = a.today(S);
        hov.removeAttribute('hidden');
        hov.querySelector('line').setAttribute('x1', px.toFixed(1)); hov.querySelector('line').setAttribute('x2', px.toFixed(1));
        hov.querySelector('circle').setAttribute('cx', px.toFixed(1)); hov.querySelector('circle').setAttribute('cy', Y(pe).toFixed(1));
        read.textContent = `at ${S.toFixed(2)}: ${money(pe)} at expiry, ${money(pt)} today`;
      };
      svg.onmouseleave = () => { hov.setAttribute('hidden', ''); read.textContent = ''; };
    }
    // Redrawn at the panel's width.
    const resized = new ResizeObserver(() => { if (drawn) payoff(drawn); });

    // Positions held, loaded from Accounts.
    function hold(h) {
      my.expiry = h.expiry; my.structure = `Held on ${h.account}`; my.why = ''; my.held = { account: h.account };
      position.sym = my.sym; position.legs = h.legs.map(l => ({ ...l })); my.limit = null; my.review = false; my.result = '';
    }
    const heldAsked = () => {
      const h = wanted.held; if (!h) return;
      wanted.held = null;
      if (h.sym === my.sym && my.chain) { hold(h); render(); }
      else { my.heldAsk = h; switchTo(h.sym); }
    };

    // A suggested position for the stock asked about (from the screener).
    function suggest(ask) {
      const r = recommend({ chain: my.chain, view: ask.view, row: ask.row, earnings: ask.earnings });
      if ('none' in r) { my.why = `No suggestion: ${r.none}.`; my.structure = 'Custom'; return; }
      my.expiry = r.expiry; my.structure = r.label; my.why = r.why;
      position.sym = my.sym; position.legs = r.legs; my.limit = null; my.review = false; my.result = '';
    }

    // Trade opens the review: the account (a paper one unless picked), every
    // contract, the limit (net per spread, starting at the mid; positive pays,
    // negative receives) and what it can cost, then Send. The quantities are the legs'.
    const symbolOf = l => quote(l.type, l.strike)?.symbol || null;
    const cents = x => Math.round(x * 100) / 100;
    function order(a) {
      const ls = legs();
      if (!a || !ls.length || ls.some(l => !symbolOf(l))) return null;
      // Ratios in lowest terms; the quantity carries the rest.
      const g = ls.reduce((x, l) => { let p = x, q = l.qty; while (q) [p, q] = [q, p % q]; return p; }, 0) || 1;
      const limit = my.limit ?? cents(a.net / g);
      if (!limit) return null;
      return { legs: ls.map(l => ({ symbol: symbolOf(l), side: l.side > 0 ? 'buy' : 'sell', ratio: l.qty / g })), qty: g, limit_price: limit, effect: 'open' };
    }
    function ticket(a) {
      const box = /** @type {HTMLElement} */ ($('.sb-ticket'));
      const result = my.result ? `<p class="sb-result">${my.result}</p>` : '';
      if (!my.review || a?.held) { box.innerHTML = result; return; }
      // Not while typing in it: a refresh would take the input away.
      const f = document.activeElement;
      if (f && f.tagName === 'INPUT' && box.contains(f)) return;
      const note = t => { box.innerHTML = `<div class="sb-review"><p class="muted sb-note">${t}</p><p><button type="button" class="btn" data-act="back">Close</button></p></div>`; };
      if (!my.orders) { note('Loading the accounts…'); return; }
      if (my.orders.unavailable) { note(`Orders: ${esc(my.orders.unavailable)}`); return; }
      if (!my.orders.trading) { note('Sending orders needs the console\'s trade token (operator: scripts/gateway-token GATEWAY_CONSOLE_TOKEN, then restart the gateways and the console).'); return; }
      const o = order(a);
      if (!o) { note('A leg has no listed contract or price here, so there is nothing to send.'); return; }
      // A live account is never picked for you: the first paper one is the default.
      const accts = my.orders.accounts || [], usable = accts.filter(x => !x.error), isLive = x => x?.environment === 'live';
      if (!usable.some(x => x.account === my.acct)) my.acct = usable.find(x => !isLive(x))?.account || '';
      const live = isLive(accts.find(x => x.account === my.acct));
      const credit = o.limit_price < 0, total = Math.abs(o.limit_price) * 100 * o.qty;
      // At the limit rather than the mid.
      const worst = a.maxL + a.net - o.limit_price * o.qty;
      const risk = isFinite(a.maxL) ? (worst < 0 ? `max loss $${Math.abs(worst * 100).toFixed(0)}` : 'no loss at expiry') : 'loss unlimited';
      const k = l => `${l.side > 0 ? 'Buy' : 'Sell'} ${l.qty} ${my.sym} ${my.expiry} ${l.strike} ${l.type}`;
      // What's already working on this account, however it was placed.
      const mine = openOrders().get(my.acct), desc = x => x.legs?.length ? x.legs.map(l => `${l.side === 'sell' ? '−' : '+'}${(l.ratio_qty || 1) * (x.qty || 1)} ${l.symbol}`).join(' ') : `${x.side === 'sell' ? '−' : '+'}${x.qty} ${x.symbol}`;
      const already = mine === null ? `<p class="sb-bad">${esc(my.acct)}'s open orders couldn't be read: check the broker first.</p>`
        : mine?.length ? `<p class="sb-bad"><b>${esc(my.acct)} already has ${mine.length} open order${mine.length > 1 ? 's' : ''}:</b></p><ul class="mono">${mine.map(x => `<li>${esc(desc(x))} @ ${x.limit_price ?? x.type} <span class="muted">${esc(x.status || '')}${(x.client_order_id || '').startsWith('tt-') ? '' : ', placed outside the console'}</span></li>`).join('')}</ul>` : '';
      box.innerHTML = `<div class="sb-review${live ? ' sb-live' : ''}">${live ? '<p class="sb-bad"><b>LIVE ACCOUNT: real money.</b></p>' : ''}
        <p class="sb-tk"><label>Account <select class="sb-acct">${accts.map(x => `<option value="${esc(x.account)}"${x.error ? ' disabled' : ''}${x.account === my.acct ? ' selected' : ''}>${esc(x.account)}${isLive(x) ? ' (LIVE: real money)' : ''}${x.error ? ' (unreachable)' : x.state && x.state !== 'ready' ? ` (${esc(x.state)})` : ''}</option>`).join('')}</select></label>
          <label>Limit <input class="sb-limit mono" type="number" step="0.01" value="${o.limit_price.toFixed(2)}" title="Net per ${o.legs.length === 1 ? 'contract' : 'spread'}: positive pays (debit), negative receives (credit)"></label>
          <span class="muted">${credit ? 'credit' : 'debit'}${o.qty > 1 ? ` × ${o.qty}` : ''}</span></p>
        ${already}<ul class="mono">${legs().map(l => `<li>${esc(k(l))} <span class="muted">${esc(symbolOf(l))}</span></li>`).join('')}</ul>
        <p>${credit ? `Receive at least $${total.toFixed(0)}` : `Pay at most $${total.toFixed(0)}`}; ${risk}. ${o.legs.length === 1 ? 'One order' : 'One combo order'}, day, opening.</p>
        ${live ? `<p><label>Type <b class="mono">${esc(my.acct)}</b> to send: <input class="sb-confirm mono" autocomplete="off" spellcheck="false" value="${esc(my.typed)}"></label></p>` : ''}
        <p><button type="button" class="btn primary" data-act="send"${!my.acct || my.sending || (live && my.typed.trim() !== my.acct) ? ' disabled' : ''}>${my.sending ? 'Sending…' : live ? 'Send live order' : 'Send order'}</button> <button type="button" class="btn" data-act="back">Cancel</button></p></div>${result}`;
    }
    function ordersList() {
      const box = $('.sb-orders'), all = (my.orders?.accounts || []).flatMap(x => (x.orders || []).map(o => ({ ...o, account: x.account })));
      paint(box, all.length ? `<p class="muted sb-note">Working orders</p><ul class="sb-ol mono">${all.map(o => `<li>${esc(o.account)} · ${esc(o.client_order_id)} · ${esc(o.status || '')} · ${o.filled_qty || 0}/${esc(String(o.qty ?? '?'))} at ${esc(String(o.limit_price ?? '?'))}${o.legs?.length ? ` · ${o.legs.map(l => esc(l.symbol)).join(' ')}` : ''} <button type="button" class="chip-btn" data-cancel="${esc(o.account)}|${esc(o.client_order_id)}">Cancel</button></li>`).join('')}</ul>` : '');
    }
    async function loadOrders() {
      try {
        const r = await fetch('/api/orders', { cache: 'no-store' });
        if (my.alive) {
          my.orders = r.ok ? await r.json() : { unavailable: r.status === 404 ? 'this console has no order routing yet (it needs redeploying).' : `the console answered ${r.status}.` };
          my.ordersAt = Date.now();
        }
      } catch (e) { my.orders = { unavailable: `the console can't be reached (${e.message}).` }; my.ordersAt = Date.now(); }
      if (my.alive) { if (my.review) ordersList(); else render(); }
    }
    async function send() {
      if (my.sending) return;
      // Checked before anything is redrawn: a live order needs the account's name typed.
      const live = (my.orders?.accounts || []).find(x => x.account === my.acct)?.environment === 'live';
      if (live && my.typed.trim() !== my.acct) { my.result = `<span class="sb-bad">Type ${esc(my.acct)} to send a live order.</span>`; ticket(analyse()); return; }
      my.sending = true; ticket(analyse());
      const o = order(analyse());
      if (!o) { my.sending = false; my.result = '<span class="sb-bad">Not sent: a leg has no listed contract.</span>'; my.review = false; render(); return; }
      try {
        const r = await fetch(`/api/orders/${encodeURIComponent(my.acct)}`, { method: 'POST', headers: { 'content-type': 'application/json' },
          body: JSON.stringify({ confirm: my.acct, live, order: { legs: o.legs, qty: o.qty, limit_price: o.limit_price, effect: 'open' } }) });
        const v = await r.json().catch(() => ({}));
        my.result = outcome(v, r.ok);
      } catch (e) { my.result = `Not known whether it was sent: ${esc(e.message)}. Check working orders before sending again.`; }
      my.sending = false; my.review = false; my.typed = ''; render(); loadOrders();
    }
    function outcome(v, ok) {
      if (!ok || v.error) return `<span class="sb-bad">Not sent: ${esc(v.error || 'error')}.</span>`;
      const id = esc(v.client_order_id || v.current?.client_order_id || '');
      if (v.outcome === 'accepted') return `<span class="sb-good">Accepted by the broker (${esc(v.status || 'new')}), ${id}. Not a fill yet: it's listed below until it fills or is cancelled.</span>`;
      if (v.outcome === 'rejected') return `<span class="sb-bad">Rejected by the broker: ${esc(v.message || v.code || '')}.</span>`;
      if (v.outcome === 'not_sent') return `<span class="sb-bad">Not sent: ${esc(notSentText(v.reason || '', v.detail))}.</span>`;
      if (v.outcome === 'unknown') return `<span class="sb-bad">Unknown (${esc(v.reason || '')}): it may have reached the broker. Don't send it again; the gateway is checking and it will show below if it did.</span>`;
      return esc(JSON.stringify(v).slice(0, 200));
    }
    const switchTo = s => { my.sym = s; my.chain = null; my.expiry = null; my.why = ''; my.held = null; my.limit = null; my.review = false; my.result = ''; if (!STRUCTURES[my.structure]) my.structure = 'Iron condor'; position.legs = []; render(); loadChain(); };

    el.addEventListener('submit', e => {
      e.preventDefault();
      const s = (/** @type {HTMLInputElement} */ ($('.sb-ticker')).value || '').trim().toUpperCase();
      if (/^[A-Z][A-Z0-9.]{0,9}$/.test(s)) switchTo(s);
    });
    el.addEventListener('input', e => {
      const t = /** @type {HTMLInputElement} */ (e.target);
      if (!t.classList.contains('sb-confirm')) return;
      my.typed = t.value;
      const send = /** @type {HTMLButtonElement | null} */ (el.querySelector('[data-act="send"]'));
      if (send) send.disabled = my.sending || my.typed.trim() !== my.acct;
    });
    // A vertical added to the legs there are: a credit spread's short strike
    // one width out of the money (calls above, puts below), a debit spread's
    // long strike at the money. Legs on the same contract net out.
    function addSpread(name) {
      const ks = strikesOf(), spot = my.chain?.spot, make = STRUCTURES[name];
      if (!ks.length || !spot || !make) return;
      const shape = make(my.width), credit = /credit/i.test(name), calls = shape[0][0] === 'call';
      const a = ks.indexOf(nearest(ks, spot)) + (credit ? (calls ? my.width : -my.width) : 0);
      const out = legs().map(l => ({ ...l }));
      for (const [type, o, side, qty] of shape) {
        const strike = ks[Math.max(0, Math.min(ks.length - 1, a + o))];
        const had = out.find(l => l.type === type && l.strike === strike);
        if (!had) { out.push({ type, strike, side, qty }); continue; }
        const n = had.side * had.qty + side * qty;
        had.side = n >= 0 ? 1 : -1; had.qty = Math.abs(n); delete had.cost;
      }
      position.sym = my.sym; position.legs = out.filter(l => l.qty > 0);
      my.structure = 'Custom'; my.why = ''; my.limit = null; my.review = false;
    }
    el.addEventListener('change', e => {
      const t = /** @type {HTMLSelectElement} */ (e.target);
      if (t.classList.contains('sb-addsp')) { if (t.value) { addSpread(t.value); render(); } return; }
      if (t.classList.contains('sb-st')) { my.review = false; my.limit = null; my.result = ''; my.structure = t.value; my.why = ''; my.held = null; build(); render(); return; }
      if (t.classList.contains('sb-acct')) { my.acct = t.value; my.typed = ''; store('tt-sb-account', t.value); render(); return; }
      if (t.classList.contains('sb-limit')) { const v = Math.round(+t.value * 100) / 100; if (v) my.limit = v; t.blur(); render(); return; }
      if (!t.classList.contains('sb-exp')) return;
      my.limit = null;
      my.expiry = t.value;
      // A preset structure is rebuilt; a suggested or edited one keeps its strikes, moved to listed ones.
      if (STRUCTURES[my.structure] && !my.why) build();
      else for (const l of legs()) { l.strike = nearest(strikesOf(l.type), l.strike) ?? l.strike; delete l.cost; }
      render();
    });
    el.addEventListener('click', e => {
      const b = /** @type {HTMLElement} */ (e.target).closest('button'); if (!b) return;
      // The payoff chart, shown on request (Candles has the overlay); remembered.
      // A contract's symbol: its chart on Candles, opened if need be.
      if (b.dataset.occ) { document.dispatchEvent(new CustomEvent('tt-show-view', { detail: 'candles' })); setTimeout(() => document.dispatchEvent(new CustomEvent('tt-chart', { detail: b.dataset.occ }))); return; }
      // Trade: the review, at the mid as it is now (the price shown is the price sent).
      if (b.classList.contains('sb-trade')) {
        if (my.review) { my.review = false; render(); return; }
        my.limit = null; const o = order(analyse()); my.limit = o?.limit_price ?? null;
        my.review = true; my.typed = ''; my.result = ''; render(); return;
      }
      if (b.classList.contains('sb-paybtn')) { my.payoff = !my.payoff; store('tt-sb-payoff', my.payoff ? '1' : '0'); render(); return; }
      if (b.dataset.cancel) {
        const [acct, id] = b.dataset.cancel.split('|'); b.disabled = true;
        fetch(`/api/orders/${encodeURIComponent(acct)}/${encodeURIComponent(id)}/cancel`, { method: 'POST', headers: { 'content-type': 'application/json' }, body: '{}' })
          .then(r => r.json()).then(v => { my.result = v.outcome === 'accepted' ? `Cancel sent for ${esc(id)}.` : `Cancel ${esc(v.outcome || v.error || '')}: ${esc(v.detail || v.message || '')}`; })
          .catch(e => { my.result = `Cancel: ${esc(e.message)}`; }).finally(() => { render(); loadOrders(); });
        return;
      }
      const act = b.dataset.act;
      if (act === 'back') { my.review = false; render(); return; }
      if (act === 'send') { send(); return; }
      my.review = false; my.limit = null; my.result = '';
      if (!act) return;
      if (act === 'add') {
        const ks = strikesOf('call');
        if (ks.length) { position.sym = my.sym; position.legs = [...legs(), { type: 'call', strike: nearest(ks, my.chain?.spot || 0), side: 1, qty: 1 }]; }
      } else if (e.shiftKey && (act === 'more' || act === 'less')) {
        // Shift-click sizes the whole position by one unit, keeping its ratios
        // (1/1/1/1 to 2/2/2/2, +1/−2 to +2/−4); never below one unit or past 20 a leg.
        const ls = legs(), gcd = (x, y) => y ? gcd(y, x % y) : x;
        const unit = ls.reduce((g, l) => gcd(g, l.qty), 0) || 1, m = unit + (act === 'more' ? 1 : -1);
        if (m < 1 || ls.some(l => l.qty / unit * m > 20)) return;
        for (const l of ls) l.qty = l.qty / unit * m;
        my.review = false; render(); return;
      } else {
        const i = +(/** @type {HTMLElement} */ (b.closest('[data-i]'))?.dataset.i), l = legs()[i]; if (!l) return;
        if (act === 'side') l.side = l.side > 0 ? -1 : 1;
        // Signed quantity, skipping zero: past it the leg flips between buy and sell.
        if (act === 'more' || act === 'less') {
          let n = l.side * l.qty + (act === 'more' ? 1 : -1);
          if (n === 0) n = act === 'more' ? 1 : -1;
          n = Math.max(-20, Math.min(20, n)); l.side = n > 0 ? 1 : -1; l.qty = Math.abs(n);
        }
        // A held leg moved to another contract is a what-if: priced at the market, not its entry.
        if (act === 'type') { l.type = l.type === 'call' ? 'put' : 'call'; l.strike = nearest(strikesOf(l.type), l.strike) ?? l.strike; delete l.cost; }
        if (act === 'up' || act === 'down') { const ks = strikesOf(l.type), j = ks.indexOf(l.strike) + (act === 'up' ? 1 : -1); if (ks[j] != null) { l.strike = ks[j]; delete l.cost; } }
        if (act === 'remove') legs().splice(i, 1);
      }
      my.structure = 'Custom'; my.why = ''; render();
    });
    // A suggestion asked for from the screener: for this underlying once its chain is in.
    const asked = () => {
      const ask = wanted.ask; if (!ask) return;
      wanted.ask = null;
      if (ask.sym === my.sym && my.chain) { suggest(ask); render(); }
      else { my.ask = ask; switchTo(ask.sym); }
    };
    const follow = (/** @type {Event} */ e) => {
      const s = /** @type {CustomEvent} */ (e).detail;
      // A suggestion for it is on its way (the screener sends both).
      if (typeof s === 'string' && s !== my.sym && wanted.ask?.sym !== s && my.ask?.sym !== s) switchTo(s);
    };
    document.addEventListener('tt-recommend', asked);
    document.addEventListener('tt-held', heldAsked);
    document.addEventListener('tt-focus', follow);
    // Opened from an account's card: that account for the Trade review.
    const acctAsked = e => { const n = /** @type {CustomEvent} */ (e).detail; if (typeof n === 'string' && n) { my.acct = n; my.typed = ''; render(); } };
    document.addEventListener('tt-sb-account', acctAsked);
    if (wanted.held) heldAsked(); else if (wanted.ask) asked(); else { render(); loadChain(); }
    loadOrders();
    resized.observe(el);
    const timer = setInterval(() => {
      if (Date.now() - my.at > 30e3) loadChain();
      if (Date.now() - my.ordersAt > 10e3) loadOrders();
    }, 5000);
    return {
      update() {},
      dispose() {
        my.alive = false; clearInterval(timer); resized.disconnect();
        document.removeEventListener('tt-recommend', asked); document.removeEventListener('tt-held', heldAsked); document.removeEventListener('tt-focus', follow); document.removeEventListener('tt-sb-account', acctAsked);
        publish({ shown: false });
      },
    };
  },
});

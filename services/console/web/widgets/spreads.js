// Spread builder: an options position from a structure and the live chain,
// priced at the mid and crossing the spread, with its maximum profit and loss,
// breakevens and chance of profit (from the expiry's implied distribution,
// fitted to the whole chain by alpaca-data, unless the fit is poor; else
// lognormal at the at-the-money volatility). "Open on chart" draws it on
// Candles (the shared position, lib/position.js). Quotes come from a service
// serving option chains (alpaca-data; Alpaca's indicative feed), which
// works out implied volatilities from them. The underlying is typed in, or
// follows the symbol picked elsewhere; a stock clicked in the screener gets
// a suggested position (lib/recommend.js) with the reason. An order for the
// position goes to a paper account's order gateway as the console's trade
// client (POST /api/orders/{account}) after a review step; the console's
// working orders are listed with Cancel.
import { defineWidget, paint } from '../lib/widgets.js';
import { fleet } from '../lib/fleet.js';
import { esc } from '../lib/format.js';
import { focused, theme, setFocus } from '../lib/theme.js';
import { position, publish, wanted } from '../lib/position.js';
import { recommend } from '../lib/recommend.js';

const store = (k, v) => { try { if (v === undefined) return localStorage.getItem(k); localStorage.setItem(k, v); } catch (_) { /* private mode */ } return null; };
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
    const my = { sym: focused() || theme()?.symbols?.[0] || 'SPY', chain: null, expiry: null, structure: 'Iron condor', width: 2, err: '', alive: true, at: 0,
      // A suggestion waiting for its chain, and the one shown.
      ask: /** @type {any} */ (null), why: '',
      // The order ticket.
      acct: store('tt-sb-account') || '', qty: 1, limit: /** @type {number | null} */ (null), review: false, sending: false, result: '',
      orders: /** @type {any} */ (null), ordersAt: 0 };
    el.innerHTML = `<div class="sb">
      <div class="sb-top">
        <form class="sb-search"><input class="sb-ticker mono" name="t" placeholder="Ticker" maxlength="10" autocomplete="off" spellcheck="false" aria-label="Underlying"></form>
        <strong class="sb-sym mono"></strong><span class="sb-spot mono muted"></span>
        <select class="sb-exp mono" aria-label="Expiry"></select>
        <button type="button" class="chip-btn sb-show" aria-pressed="false">Open on chart</button>
      </div>
      <div class="sb-structs chips"></div>
      <p class="sb-why"></p>
      <ul class="sb-legs"></ul>
      <p class="sb-sum mono"></p>
      <div class="sb-ticket"></div>
      <div class="sb-orders"></div>
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
      if (ls.some(l => mid(l.q) == null && !iv(l))) return null;
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
      return { net, cross, pay, maxP, maxL, be, pop, popFrom: pts ? `implied, ${dist.quality} fit` : 'lognormal', missing: ls.some(l => !l.q) };
    }

    const money = v => !isFinite(v) ? 'unlimited' : `${v < 0 ? '−' : ''}$${Math.abs(v * 100).toFixed(0)}`;
    const day = d => new Date(d + 'T12:00:00Z').toLocaleDateString(undefined, { timeZone: 'UTC', month: 'short', day: 'numeric' });
    function render() {
      const a = analyse();
      // Publish first: Candles draws from it.
      publish({ sym: my.sym, expiry: my.expiry, strikes: { call: strikesOf('call'), put: strikesOf('put') }, net: a?.net ?? 0, be: a?.be ?? [], pay: a?.pay ?? null });
      $('.sb-sym').textContent = my.sym;
      $('.sb-spot').textContent = my.chain?.spot ? my.chain.spot.toFixed(2) + (my.chain.spot_from === 'parity' ? ' (from options)' : '') : '';
      const exps = expiries(), sel = /** @type {HTMLSelectElement} */ ($('.sb-exp')), key = `${my.sym}|${exps.map(e => e.expiry).join()}`;
      if (sel.dataset.for !== key) {
        sel.innerHTML = exps.map(e => `<option value="${e.expiry}">${day(e.expiry)}${e.implied_move ? ` · ±${(e.implied_move * 100).toFixed(1)}%` : ''}</option>`).join('');
        sel.dataset.for = key;
      }
      if (my.expiry) sel.value = my.expiry;
      $('.sb-show').setAttribute('aria-pressed', String(position.shown));
      $('.sb-why').innerHTML = my.why ? `<b>${esc(my.structure)}.</b> ${esc(my.why)}` : '';
      $('.sb-structs').innerHTML = Object.keys(STRUCTURES).map(n => `<button type="button" class="chip-btn" data-st="${esc(n)}" aria-pressed="${my.structure === n}">${esc(n)}</button>`).join('')
        + (STRUCTURES[my.structure] ? '' : `<span class="chip-btn" aria-pressed="true">${esc(my.structure)}</span>`)
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
      ticket(a);
      $('.sb-msg').textContent = my.err || (a?.missing ? 'A leg has no quote on this expiry; it is priced from its implied volatility.' : '') ||
        (my.chain ? 'Per position: 100 shares a contract. Alpaca indicative quotes; implied volatilities worked out from them.' : 'Loading the chain…');
    }

    // A suggested position for the stock asked about (from the screener).
    function suggest(ask) {
      const r = recommend({ chain: my.chain, view: ask.view, row: ask.row, earnings: ask.earnings });
      if ('none' in r) { my.why = `No suggestion: ${r.none}.`; my.structure = 'Custom'; return; }
      my.expiry = r.expiry; my.structure = r.label; my.why = r.why;
      position.sym = my.sym; position.legs = r.legs; my.limit = null; my.review = false; my.result = '';
    }

    // The order ticket: account, quantity and limit, then a review before sending.
    const symbolOf = l => quote(l.type, l.strike)?.symbol || null;
    const cents = x => Math.round(x * 100) / 100;
    function order(a) {
      const ls = legs();
      if (!a || !ls.length || ls.some(l => !symbolOf(l))) return null;
      // Ratios in lowest terms; the quantity carries the rest.
      const g = ls.reduce((x, l) => { let p = x, q = l.qty; while (q) [p, q] = [q, p % q]; return p; }, 0) || 1;
      const limit = my.limit ?? cents(a.net / g);
      return { legs: ls.map(l => ({ symbol: symbolOf(l), side: l.side > 0 ? 'buy' : 'sell', ratio: l.qty / g })), qty: my.qty * g, limit_price: limit, effect: 'open' };
    }
    function ticket(a) {
      const box = $('.sb-ticket'), o = order(a), accts = my.orders?.accounts || [];
      if (!my.orders) { box.innerHTML = ''; return; }
      if (!my.orders.trading) { box.innerHTML = '<p class="muted sb-note">Sending orders needs the console\'s trade token for the paper gateways (operator: scripts/gateway-token GATEWAY_CONSOLE_TOKEN, then restart the gateways and the console).</p>'; return; }
      if (!o) { box.innerHTML = ''; return; }
      const usable = accts.filter(x => !x.error);
      if (!usable.some(x => x.account === my.acct)) my.acct = usable[0]?.account || '';
      const credit = o.limit_price < 0, single = o.legs.length === 1;
      const unit = `${credit ? 'credit' : 'debit'} $${Math.abs(o.limit_price).toFixed(2)}`;
      const total = Math.abs(o.limit_price) * 100 * o.qty;
      const risk = isFinite(a.maxL) ? `max loss $${Math.abs(a.maxL * 100 * my.qty).toFixed(0)}` : 'loss unlimited';
      if (my.review) {
        const k = l => `${l.side > 0 ? 'Buy' : 'Sell'} ${l.qty * my.qty} ${my.sym} ${my.expiry} ${l.strike} ${l.type}`;
        box.innerHTML = `<div class="sb-review"><p><b>Send to ${esc(my.acct)}</b> (paper) as ${single ? 'one order' : 'one combo order'}, limit ${unit}${single ? '' : ' per spread'} × ${o.qty}: ${credit ? 'receive' : 'pay'} $${total.toFixed(0)} at most${credit ? ' less' : ''}; ${risk}. Day order, opening.</p>
          <ul class="mono">${legs().map(l => `<li>${esc(k(l))} <span class="muted">${esc(symbolOf(l))}</span></li>`).join('')}</ul>
          <p><button type="button" class="btn primary" data-act="send"${my.sending ? ' disabled' : ''}>${my.sending ? 'Sending…' : 'Send order'}</button> <button type="button" class="btn" data-act="back">Back</button></p></div>`;
        return;
      }
      box.innerHTML = `<div class="sb-tk">
        <label>Account <select class="sb-acct">${accts.map(x => `<option value="${esc(x.account)}"${x.error ? ' disabled' : ''}${x.account === my.acct ? ' selected' : ''}>${esc(x.account)}${x.error ? ' (unreachable)' : x.state && x.state !== 'ready' ? ` (${esc(x.state)})` : ''}</option>`).join('')}</select></label>
        <label>Qty <input class="sb-qty mono" type="number" min="1" max="100" value="${my.qty}"></label>
        <label>Limit <input class="sb-limit mono" type="number" step="0.01" value="${o.limit_price.toFixed(2)}" title="Net per ${single ? 'contract' : 'spread'}: positive pays (debit), negative receives (credit)"></label>
        <span class="muted mono">${unit} · ${risk}</span>
        <button type="button" class="btn primary" data-act="review"${my.acct ? '' : ' disabled'}>Review order</button></div>
        ${my.result ? `<p class="sb-result">${my.result}</p>` : ''}`;
    }
    function ordersList() {
      const box = $('.sb-orders'), all = (my.orders?.accounts || []).flatMap(x => (x.orders || []).map(o => ({ ...o, account: x.account })));
      paint(box, all.length ? `<p class="muted sb-note">Working orders from the console</p><ul class="sb-ol mono">${all.map(o => `<li>${esc(o.account)} · ${esc(o.client_order_id)} · ${esc(o.status || '')} · ${o.filled_qty || 0}/${esc(String(o.qty ?? '?'))} at ${esc(String(o.limit_price ?? '?'))}${o.legs?.length ? ` · ${o.legs.map(l => esc(l.symbol)).join(' ')}` : ''} <button type="button" class="chip-btn" data-cancel="${esc(o.account)}|${esc(o.client_order_id)}">Cancel</button></li>`).join('')}</ul>` : '');
    }
    async function loadOrders() {
      try {
        const r = await fetch('/api/orders', { cache: 'no-store' });
        if (r.ok && my.alive) { my.orders = await r.json(); my.ordersAt = Date.now(); }
      } catch (_) { /* shown as nothing */ }
      if (my.alive) { if (!my.review) ticket(analyse()); ordersList(); }
    }
    async function send() {
      const o = order(analyse()); if (!o || my.sending) return;
      my.sending = true; ticket(analyse());
      try {
        const r = await fetch(`/api/orders/${encodeURIComponent(my.acct)}`, { method: 'POST', headers: { 'content-type': 'application/json' },
          body: JSON.stringify({ confirm: my.acct, order: { legs: o.legs, qty: o.qty, limit_price: o.limit_price, effect: 'open' } }) });
        const v = await r.json().catch(() => ({}));
        my.result = outcome(v, r.ok);
      } catch (e) { my.result = `Not known whether it was sent: ${esc(e.message)}. Check working orders before sending again.`; }
      my.sending = false; my.review = false; render(); loadOrders();
    }
    function outcome(v, ok) {
      if (!ok || v.error) return `<span class="sb-bad">Not sent: ${esc(v.error || 'error')}.</span>`;
      const id = esc(v.client_order_id || v.current?.client_order_id || '');
      if (v.outcome === 'accepted') return `<span class="sb-good">Accepted by the broker (${esc(v.status || 'new')}), ${id}. Not a fill yet: it's listed below until it fills or is cancelled.</span>`;
      if (v.outcome === 'rejected') return `<span class="sb-bad">Rejected by the broker: ${esc(v.message || v.code || '')}.</span>`;
      if (v.outcome === 'not_sent') return `<span class="sb-bad">Not sent (${esc(v.reason || '')}): ${esc(v.detail || '')}.</span>`;
      if (v.outcome === 'unknown') return `<span class="sb-bad">Unknown (${esc(v.reason || '')}): it may have reached the broker. Don't send it again; the gateway is checking and it will show below if it did.</span>`;
      return esc(JSON.stringify(v).slice(0, 200));
    }
    const switchTo = s => { my.sym = s; my.chain = null; my.expiry = null; my.why = ''; my.limit = null; my.review = false; my.result = ''; if (!STRUCTURES[my.structure]) my.structure = 'Iron condor'; position.legs = []; render(); loadChain(); };

    el.addEventListener('submit', e => {
      e.preventDefault();
      const s = (/** @type {HTMLInputElement} */ ($('.sb-ticker')).value || '').trim().toUpperCase();
      if (/^[A-Z][A-Z0-9.]{0,9}$/.test(s)) switchTo(s);
    });
    el.addEventListener('change', e => {
      const t = /** @type {HTMLSelectElement} */ (e.target);
      if (t.classList.contains('sb-acct')) { my.acct = t.value; store('tt-sb-account', t.value); return; }
      if (t.classList.contains('sb-qty')) { my.qty = Math.max(1, Math.min(100, Math.round(+t.value) || 1)); render(); return; }
      if (t.classList.contains('sb-limit')) { const v = Math.round(+t.value * 100) / 100; my.limit = v ? v : null; render(); return; }
      if (!t.classList.contains('sb-exp')) return;
      my.limit = null;
      my.expiry = t.value;
      // A preset structure is rebuilt; a suggested or edited one keeps its strikes, moved to listed ones.
      if (STRUCTURES[my.structure] && !my.why) build();
      else for (const l of legs()) l.strike = nearest(strikesOf(l.type), l.strike) ?? l.strike;
      render();
    });
    el.addEventListener('click', e => {
      const b = /** @type {HTMLElement} */ (e.target).closest('button'); if (!b) return;
      if (b.classList.contains('sb-show')) {
        position.shown = !position.shown;
        // Point Candles at this underlying, opening it if it isn't in the workspace.
        if (position.shown) {
          if (focused() !== my.sym) { setFocus(my.sym); document.dispatchEvent(new CustomEvent('tt-focus', { detail: my.sym })); }
          document.dispatchEvent(new CustomEvent('tt-show-view', { detail: 'candles' }));
        }
        render(); return;
      }
      if (b.dataset.cancel) {
        const [acct, id] = b.dataset.cancel.split('|'); b.disabled = true;
        fetch(`/api/orders/${encodeURIComponent(acct)}/${encodeURIComponent(id)}/cancel`, { method: 'POST', headers: { 'content-type': 'application/json' }, body: '{}' })
          .then(r => r.json()).then(v => { my.result = v.outcome === 'accepted' ? `Cancel sent for ${esc(id)}.` : `Cancel ${esc(v.outcome || v.error || '')}: ${esc(v.detail || v.message || '')}`; })
          .catch(e => { my.result = `Cancel: ${esc(e.message)}`; }).finally(() => { render(); loadOrders(); });
        return;
      }
      const act = b.dataset.act;
      if (act === 'review') { my.review = true; my.result = ''; render(); return; }
      if (act === 'back') { my.review = false; render(); return; }
      if (act === 'send') { send(); return; }
      my.review = false; my.limit = null; my.result = '';
      if (b.dataset.st) { my.structure = b.dataset.st; my.why = ''; build(); render(); return; }
      if (b.dataset.w) { my.width = Math.max(1, Math.min(20, my.width + +b.dataset.w)); if (STRUCTURES[my.structure]) build(); render(); return; }
      if (!act) return;
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
    document.addEventListener('tt-focus', follow);
    if (wanted.ask) asked(); else { render(); loadChain(); }
    loadOrders();
    const timer = setInterval(() => {
      if (Date.now() - my.at > 30e3) loadChain();
      if (Date.now() - my.ordersAt > 10e3) loadOrders();
    }, 5000);
    return {
      update() {},
      dispose() {
        my.alive = false; clearInterval(timer);
        document.removeEventListener('tt-recommend', asked); document.removeEventListener('tt-focus', follow);
        publish({ shown: false });
      },
    };
  },
});

// Earnings: upcoming reports with what options expect against what has
// happened before, to judge whether the expected move is rich or cheap.
//
// For each report:
// - Jump: the expected earnings move from the term structure. The first
//   expiry after the report holds the jump on top of ordinary volatility:
//   σ₁²T₁ = b²T₁ + m². The ordinary level b comes from the expiry before the
//   report or, without one, from the next expiry after (σ₂²T₂ = b²T₂ + m²).
// - Straddle: the at-the-money straddle of that first expiry, as a fraction
//   of the price (includes ordinary movement too, so a little larger).
// - Past: the actual move on each of the last 8 reports (close before to
//   close after, by when it reported), their average size, and how often EPS
//   beat the estimate. Ratio: Jump ÷ average past move; above 1, options
//   price more than usual (the short-volatility side), below 1 less.
//
// Chains and bars come from alpaca-data, reports and past earnings from
// econ-calendar (whose Alpha Vantage histories are limited to 20 a day, so
// they load for theme companies and for a row when it's opened).
import { defineWidget, paint } from '../lib/widgets.js';
import { fleet } from '../lib/fleet.js';
import { esc } from '../lib/format.js';
import { theme, setFocus } from '../lib/theme.js';
import { get } from '../lib/cache.js';

const svcWith = k => (fleet.topo?.services || []).find(s => s[k])?.id || null;
const DAY = 86_400_000;
const pct = (x, d = 1) => x == null || !isFinite(x) ? '—' : `${(x * 100).toFixed(d)}%`;
const cap = n => n == null ? '' : n >= 1e12 ? `$${(n / 1e12).toFixed(1)}T` : `$${Math.round(n / 1e9)}B`;
const WHEN = { 'before-open': 'AM', 'after-close': 'PM', during: 'day', unknown: '—' };

/** Upcoming reports from the econ service, earliest first. */
function upcoming() {
  const e = Object.values(fleet.snap?.services || {}).map(s => s.earnings).find(x => Array.isArray(x?.reports));
  return e?.reports || [];
}

/** The first trading day on or after a date (weekends skipped; holidays aren't, a day's error at most). @param {string} d */
function nextWeekday(d) {
  const t = new Date(d + 'T12:00:00Z');
  while ([0, 6].includes(t.getUTCDay())) t.setUTCDate(t.getUTCDate() + 1);
  return t.toISOString().slice(0, 10);
}
/** The day the market reacts: the report day if before the open, else the next trading day. */
function reaction(date, when) {
  if (when === 'before-open') return nextWeekday(date);
  const t = new Date(date + 'T12:00:00Z'); t.setUTCDate(t.getUTCDate() + 1);
  return nextWeekday(t.toISOString().slice(0, 10));
}
/** Years from now to an expiry's close (16:00 New York, about 20:00 UTC). @param {string} e */
const years = e => Math.max(0, (Date.parse(e + 'T20:00:00Z') - Date.now()) / DAY / 365);

/** The expected earnings jump from a chain's expiries. @param {{ expiry: string, atm_iv: number | null, implied_move: number | null }[]} xs @param {string} react */
export function impliedJump(xs, react) {
  const ok = xs.filter(x => x.atm_iv != null && years(x.expiry) > 1 / 365);
  const i = ok.findIndex(x => x.expiry >= react);
  if (i < 0) return { jump: null, straddle: null, first: null };
  const e1 = ok[i], t1 = years(e1.expiry), v1 = e1.atm_iv ** 2 * t1;
  let base2 = null;
  if (i > 0) base2 = ok[i - 1].atm_iv ** 2;
  else if (ok[i + 1]) { const t2 = years(ok[i + 1].expiry); base2 = (ok[i + 1].atm_iv ** 2 * t2 - v1) / (t2 - t1); }
  const m2 = base2 == null ? null : v1 - Math.max(0, base2) * t1;
  return { jump: m2 == null ? null : Math.sqrt(Math.max(0, m2)), straddle: e1.implied_move, first: e1.expiry };
}

/** Each past report's move, from daily closes by date. @param {any[]} reports @param {Map<string, number>} close @param {string[]} dates sorted */
export function pastMoves(reports, close, dates) {
  const before = d => { let k = -1; for (let i = 0; i < dates.length && dates[i] < d; i++) k = i; return k; };
  return reports.map(r => {
    const k = before(r.date); // the last close before the report day
    if (k < 0) return { ...r, move: null };
    // Before the open (or during the session): the close before to the
    // report day's. After the close: the report day's to the next day's. Not
    // given: across both days.
    const from = r.when === 'after-close' ? dates[k + 1] : dates[k];
    const to = r.when === 'before-open' || r.when === 'during' ? dates[k + 1] : dates[k + 2];
    const move = from && to && close.get(from) && close.get(to) ? close.get(to) / close.get(from) - 1 : null;
    return { ...r, move };
  });
}

function stats(moves) {
  const m = moves.filter(x => x.move != null).slice(0, 8), s = moves.filter(x => x.surprise_pct != null).slice(0, 8);
  return {
    n: m.length,
    avg: m.length ? m.reduce((a, x) => a + Math.abs(x.move), 0) / m.length : null,
    beat: s.length ? s.filter(x => x.surprise_pct > 0).length / s.length : null,
    surprise: s.length ? s.reduce((a, x) => a + x.surprise_pct, 0) / s.length / 100 : null,
  };
}

function rows(my) {
  const t = theme(), now = new Date().toISOString().slice(0, 10), until = new Date(Date.now() + my.days * DAY).toISOString().slice(0, 10);
  return upcoming().filter(r => r.date >= now && r.date <= until)
    .filter(r => t?.symbols.length ? t.symbols.includes(r.symbol) : (r.market_cap || 0) >= my.min)
    .slice(0, 40);
}

function draw(my) {
  const list = rows(my), t = theme();
  const chips = `<div class="chips">${[[14, '2 weeks'], [42, '6 weeks']].map(([d, l]) => `<button type="button" class="chip-btn" data-er-days="${d}" aria-pressed="${my.days === d}">${l}</button>`).join('')}
    ${t?.symbols.length ? '<span class="muted" style="font-size:12px">the theme’s companies</span>' : [[200e9, '≥ $200B'], [50e9, '≥ $50B'], [10e9, '≥ $10B']].map(([m, l]) => `<button type="button" class="chip-btn" data-er-min="${m}" aria-pressed="${my.min === m}">${l}</button>`).join('')}</div>`;
  if (!upcoming().length) return chips + '<p class="muted">No earnings calendar is being served yet.</p>';
  if (!list.length) return chips + '<p class="muted">No reports in this range.</p>';
  const head = '<tr><th>Date</th><th></th><th>Symbol</th><th class="num">Cap</th><th class="num">EPS est.</th><th class="num" title="Expected earnings move from the term structure">Jump</th><th class="num" title="First expiry after the report: straddle ÷ price">Straddle</th><th class="num" title="Average size of the move on the last 8 reports">Past avg</th><th class="num" title="Jump ÷ past average: above 1, options price more than usual">Ratio</th><th class="num" title="Share of the last 8 reports that beat the EPS estimate">Beat</th></tr>';
  const body = list.map(r => {
    const o = my.opts.get(r.symbol), h = my.hist.get(r.symbol), s = h?.stats, j = o?.jump;
    const ratio = j != null && s?.avg ? j / s.avg : null;
    const cls = ratio == null ? '' : ratio >= 1.15 ? ' rich' : ratio <= 0.85 ? ' cheap' : '';
    const open = my.open === r.symbol;
    const main = `<tr class="er-row${open ? ' open' : ''}" data-er="${esc(r.symbol)}"><td>${esc(r.date.slice(5))}</td><td class="muted">${WHEN[r.when] || '—'}</td><td><b>${esc(r.symbol)}</b> <span class="muted">${esc((r.name || '').slice(0, 28))}</span></td>
      <td class="num">${cap(r.market_cap)}</td><td class="num">${r.eps_estimate ?? '—'}</td>
      <td class="num">${o ? (o.err ? '<span class="muted" title="' + esc(o.err) + '">n/a</span>' : j ? pct(j) : '<span class="muted" title="The quotes show no extra volatility in the first expiry after the report">—</span>') : '…'}</td><td class="num">${o && !o.err ? pct(o.straddle) : ''}</td>
      <td class="num">${h ? (h.err ? '<span class="muted" title="' + esc(h.err) + '">n/a</span>' : pct(s.avg)) : '<span class="muted" title="Click the row to load its past reports">load ▸</span>'}</td>
      <td class="num${cls}">${ratio == null ? '—' : ratio.toFixed(2)}</td><td class="num">${s?.beat == null ? '—' : `${Math.round(s.beat * 8)}/8`}</td></tr>`;
    return main + (open ? `<tr class="er-detail"><td colspan="10">${detail(r, o, h)}</td></tr>` : '');
  }).join('');
  return chips + `<div class="er-wrap"><table class="er">${head}${body}</table></div>
    <p class="muted er-foot">Jump: expected earnings move from the options' term structure (indicative quotes). Ratio above 1 (red): options price a bigger move than the last 8 reports' average; below 1 (blue): smaller.</p>`;
}

function detail(r, o, h) {
  if (!h) return '<p class="muted">Loading past reports…</p>';
  if (h.err) return `<p class="muted">${esc(h.err)}</p>`;
  const max = Math.max(0.01, ...h.moves.map(x => Math.abs(x.move || 0)), o?.jump || 0);
  const bars = h.moves.slice(0, 8).reverse().map(x => {
    const w = x.move == null ? 0 : Math.abs(x.move) / max * 100;
    return `<div class="er-bar"><span class="er-d">${esc(x.date)} ${WHEN[x.when] || ''}</span>
      <span class="er-track"><i class="${(x.move || 0) >= 0 ? 'up' : 'down'}" style="width:${w.toFixed(1)}%"></i>${o?.jump ? `<b style="left:${(o.jump / max * 100).toFixed(1)}%" title="Expected jump now"></b>` : ''}</span>
      <span class="er-m ${(x.move || 0) >= 0 ? 'up' : 'down'}">${x.move == null ? '—' : `${x.move >= 0 ? '+' : ''}${(x.move * 100).toFixed(1)}%`}</span>
      <span class="muted">EPS ${x.eps ?? '—'} vs ${x.estimate ?? '—'}${x.surprise_pct == null ? '' : ` (${x.surprise_pct >= 0 ? '+' : ''}${x.surprise_pct.toFixed(1)}%)`}</span></div>`;
  }).join('');
  const s = h.stats;
  return `<div class="er-detail-in"><p>${esc(r.symbol)} reports ${esc(r.date)} ${r.when === 'before-open' ? 'before the open' : r.when === 'after-close' ? 'after the close' : '(time not given)'}.
    ${o?.first ? `Options to ${esc(o.first)} expect a move of about ±${pct(o.jump)}; ` : ''}the last ${s.n} reports moved ${pct(s.avg)} on average, and EPS beat the estimate ${s.beat == null ? '—' : `${Math.round(s.beat * 100)}%`} of the time (average surprise ${pct(s.surprise)}).</p>
    <div class="er-bars">${bars}</div><p class="muted" style="font-size:12px;margin:6px 0 0">Bars: each past report's move, newest at the bottom; the line marks today's expected jump.</p></div>`;
}

defineWidget('earnings', {
  title: 'Earnings',
  mount(el) {
    // A theme's companies report over the coming weeks; otherwise the next two.
    const my = { days: theme()?.symbols.length ? 42 : 14, min: 200e9, open: null, opts: new Map(), hist: new Map(), alive: true, busy: false };
    const update = () => { paint(el, draw(my)); };
    async function json(url) {
      return get(url, { fresh: 60e3 });
    }
    async function options(r) {
      const svc = svcWith('chain'); if (!svc) return;
      const to = new Date(Date.parse(r.date) + 60 * DAY).toISOString().slice(0, 10);
      try {
        const c = await json(`/api/stream/${encodeURIComponent(svc)}/chain?underlying=${encodeURIComponent(r.symbol)}&to=${to}`);
        my.opts.set(r.symbol, impliedJump(c.expiries || [], reaction(r.date, r.when)));
      } catch (e) { my.opts.set(r.symbol, { err: e.message }); }
    }
    async function history(sym) {
      const hs = svcWith('earnings_history'), bs = svcWith('snapshots');
      if (!hs || !bs || my.hist.has(sym)) return;
      my.hist.set(sym, null); update();
      try {
        const [h, b] = await Promise.all([
          json(`/api/stream/${encodeURIComponent(hs)}/earnings_history?symbol=${encodeURIComponent(sym)}`),
          json(`/api/bars/${encodeURIComponent(bs)}?symbol=${encodeURIComponent(sym)}&days=800&interval=86400`),
        ]);
        const date = t => new Date((t + 43200) * 1000).toISOString().slice(0, 10);
        const close = new Map((b.bars || []).map(x => [date(x[0]), x[4]]));
        const moves = pastMoves((h.reports || []).filter(x => x.date < new Date().toISOString().slice(0, 10)), close, [...close.keys()].sort());
        my.hist.set(sym, { moves, stats: stats(moves) });
      } catch (e) { my.hist.set(sym, { err: e.message }); }
      if (my.alive) update();
    }
    // Options for every row, three at a time; past earnings for the theme's companies.
    async function fill() {
      if (my.busy) return; my.busy = true;
      const list = rows(my).filter(r => !my.opts.has(r.symbol));
      for (let i = 0; i < list.length && my.alive; i += 3) { await Promise.all(list.slice(i, i + 3).map(options)); update(); }
      const t = theme();
      if (t?.symbols.length) for (const r of rows(my)) await history(r.symbol);
      my.busy = false;
    }
    const click = e => {
      const d = e.target.closest('[data-er-days]'); if (d) { my.days = +d.dataset.erDays; update(); fill(); return; }
      const m = e.target.closest('[data-er-min]'); if (m) { my.min = +m.dataset.erMin; update(); fill(); return; }
      const row = e.target.closest('[data-er]'); if (!row) return;
      const sym = row.dataset.er; my.open = my.open === sym ? null : sym; update();
      // Its underlying goes to the IV heatmap and the chart.
      if (my.open) { history(sym); setFocus(sym); document.dispatchEvent(new CustomEvent('tt-focus', { detail: sym })); }
    };
    el.addEventListener('click', click);
    const themed = () => { my.open = null; update(); fill(); };
    document.addEventListener('tt-theme', themed);
    // Chains are refreshed every 5 minutes; the list follows the fleet's polls.
    const timer = setInterval(() => { my.opts.clear(); fill(); }, 300_000);
    update(); fill();
    return {
      update() { if (!my.busy && rows(my).some(r => !my.opts.has(r.symbol))) fill(); },
      dispose() { my.alive = false; clearInterval(timer); el.removeEventListener('click', click); document.removeEventListener('tt-theme', themed); },
    };
  },
});

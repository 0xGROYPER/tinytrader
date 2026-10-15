// Observations: what changed character, in plain sentences, from daily
// closes (lib/trend.js) for the Cross-asset board's symbols and every
// watchlist's (lib/watchlists.js). The rules: a close across the 20-, 50- or
// 200-day average (and when it was last on that side), a 52-week or 20-day
// high or low, a run of four or more up or down sessions, a day's move of
// two standard deviations or more against the last 20, the VIX term
// structure turning inverted or back, and the yield curve (10-year less
// 13-week) changing sign. Each is scored by how unusual it is and the list
// is ranked by it. Read when shown, and again once ten minutes old; a
// symbol points the workspace's charts at it.
import { defineWidget, paint } from '../lib/widgets.js';
import { fleet } from '../lib/fleet.js';
import { esc } from '../lib/format.js';
import { setFocus } from '../lib/theme.js';
import { watchlists } from '../lib/watchlists.js';
import { board, closes, analyse, label, fmtChange, fmtLevel, isYield } from '../lib/trend.js';

const STALE = 10 * 60e3, SHOW = 30;
const KINDS = { trend: 'Trend', range: 'Range', streak: 'Streak', move: 'Move', vol: 'Volatility', rates: 'Rates' };
const nice = d => d ? new Date(d + 'T12:00:00Z').toLocaleDateString(undefined, { timeZone: 'UTC', month: 'short', day: 'numeric', year: d.slice(0, 4) === new Date().toISOString().slice(0, 4) ? undefined : 'numeric' }) : '';

/** Observations for one symbol's analysis. */
function rules(a, where) {
  const out = [], name = label(a.s), lvl = fmtLevel(a);
  const add = (kind, score, text) => out.push({ kind, score, text, s: a.s, where });
  for (const [k, w] of [[200, 4], [50, 3], [20, 1.5]]) {
    const x = a.avg[k];
    if (x?.crossed) add('trend', w, `${name} closed ${x.above ? 'above' : 'below'} its ${k}-day average (${fmtLevel({ ...a, last: x.v, value: a.yield ? x.v / 10 : x.v })}) ${x.since ? `for the first time since ${nice(x.since)}` : 'for the first time in the data (over a year)'}.`);
  }
  if (a.newHi52) add('range', 4, `${name} at a 52-week high (${lvl}).`);
  else if (a.newHi20) add('range', 2, `${name} at a 20-day high (${lvl}).`);
  if (a.newLo52) add('range', 4, `${name} at a 52-week low (${lvl}).`);
  else if (a.newLo20) add('range', 2, `${name} at a 20-day low (${lvl}).`);
  if (Math.abs(a.streak) >= 4) add('streak', Math.abs(a.streak) - 2, `${name} ${a.streak > 0 ? 'up' : 'down'} ${Math.abs(a.streak)} sessions running, ${fmtChange(a.streakChange, a.yield)} over them.`);
  if (a.z != null && Math.abs(a.z) >= 2) add('move', Math.abs(a.z), `${name} ${fmtChange(a.d1, a.yield)} on the day, a ${Math.abs(a.z).toFixed(1)}σ move against its last 20.`);
  return out;
}

/** Across symbols: the VIX term structure and the yield curve, today against the session before. */
function shapes(series) {
  const out = [], at = (s, back) => { const c = series.get(s)?.closes; return c && c.length > back ? c[c.length - 1 - back][1] : null; };
  for (const [near, far, nn, fn] of [['VIX9D', 'VIX', '9-day', '30-day'], ['VIX', 'VIX3M', '30-day', '3-month']]) {
    const [a0, b0, a1, b1] = [at(near, 0), at(far, 0), at(near, 1), at(far, 1)];
    if ([a0, b0, a1, b1].some(v => v == null)) continue;
    if ((a0 > b0) !== (a1 > b1)) out.push({ kind: 'vol', score: 4, s: near, where: 'board', text: a0 > b0 ? `VIX term structure inverted: the ${nn} (${a0.toFixed(2)}) above the ${fn} (${b0.toFixed(2)}).` : `VIX term structure back to upward sloping: the ${nn} (${a0.toFixed(2)}) below the ${fn} (${b0.toFixed(2)}).` });
    else if (a0 > b0) out.push({ kind: 'vol', score: 2.5, s: near, where: 'board', text: `VIX term structure still inverted: the ${nn} ${a0.toFixed(2)} against the ${fn} ${b0.toFixed(2)}.` });
  }
  const [t0, i0, t1, i1] = [at('TNX', 0), at('IRX', 0), at('TNX', 5), at('IRX', 5)];
  if (![t0, i0, t1, i1].some(v => v == null) && Math.sign(t0 - i0) !== Math.sign(t1 - i1)) {
    out.push({ kind: 'rates', score: 3, s: 'TNX', where: 'board', text: `Yield curve ${t0 > i0 ? 'back to positive' : 'inverted'} this week: the 10-year ${(t0 / 10).toFixed(2)}% against the 13-week ${(i0 / 10).toFixed(2)}% (${((t0 - i0) * 10).toFixed(0)}bp).` });
  }
  return out;
}

function draw(my) {
  if (my.err && !my.items) return `<p class="msg-err">${esc(my.err)}</p>`;
  if (!my.items) return '<p class="muted">Reading daily closes…</p>';
  const shown = my.items.filter(o => my.kind === 'all' || o.kind === my.kind).slice(0, SHOW);
  const counts = new Map(); for (const o of my.items) counts.set(o.kind, (counts.get(o.kind) || 0) + 1);
  const chip = (k, t) => `<button type="button" class="chip-btn" data-ob-kind="${k}" aria-pressed="${my.kind === k}">${esc(t)}${k === 'all' ? '' : ` ${counts.get(k) || 0}`}</button>`;
  return `<div class="ob-head"><span class="chips">${chip('all', 'All')}${Object.entries(KINDS).filter(([k]) => counts.has(k)).map(([k, t]) => chip(k, t)).join('')}</span>
      <span class="muted">${my.items.length} from ${my.n} symbols${my.asOf ? `, closes as of ${esc(my.asOf)}` : ''}</span></div>
    ${shown.length ? `<ol class="ob">${shown.map(o => `<li><span class="ob-k ob-${o.kind}">${esc(KINDS[o.kind])}</span><span class="ob-t">${esc(o.text)}</span><button type="button" class="ob-s mono" data-sym="${esc(o.s)}" title="${o.where === 'board' ? 'On the Cross-asset board' : `In ${esc(o.where)}`}">${esc(o.s)}</button></li>`).join('')}</ol>` : '<p class="muted">Nothing out of the ordinary.</p>'}
    ${my.err ? `<p class="muted">${esc(my.err)}</p>` : ''}`;
}

defineWidget('observations', {
  title: 'Observations',
  mount(el) {
    const my = { items: /** @type {any[] | null} */ (null), err: '', at: 0, n: 0, asOf: '', kind: 'all', alive: true };
    const update = () => { paint(el, draw(my)); };
    async function load() {
      // Where each symbol comes from: the board first, else its first watchlist.
      const where = new Map();
      for (const g of board()) for (const r of g.rows) where.set(r.s, 'board');
      for (const l of watchlists()) for (const s of l.symbols) if (!where.has(s)) where.set(s, l.name);
      const syms = [...where.keys()].slice(0, 400);
      const r = await closes(fleet, syms);
      if (!my.alive) return;
      const items = [], dates = new Set();
      for (const s of syms) {
        const a = analyse(s, r.series.get(s)?.closes);
        if (!a) continue;
        if (!s.includes('/')) dates.add(a.date);
        items.push(...rules(a, where.get(s)));
      }
      items.push(...shapes(r.series));
      // Yields move a lot in σ on quiet days: weigh their moves a little less.
      for (const o of items) if (o.kind === 'move' && isYield(o.s)) o.score *= 0.8;
      items.sort((a, b) => b.score - a.score);
      my.items = items; my.n = r.series.size; my.err = r.error; my.wait = r.wait; my.at = Date.now(); my.asOf = [...dates].sort().pop() || '';
      update();
    }
    const click = e => {
      const t = /** @type {HTMLElement} */ (e.target);
      const k = /** @type {HTMLElement | null} */ (t.closest('[data-ob-kind]'))?.dataset.obKind;
      if (k) { my.kind = k; update(); return; }
      const s = /** @type {HTMLElement | null} */ (t.closest('[data-sym]'))?.dataset.sym;
      if (s && !s.includes('/') && !/^(VIX|IRX|FVX|TNX|TYX)/.test(s)) { setFocus(s); document.dispatchEvent(new CustomEvent('tt-focus', { detail: s })); }
    };
    const changed = () => { my.at = 0; load(); };
    el.addEventListener('click', click);
    document.addEventListener('tt-board', changed); document.addEventListener('tt-watchlist', changed);
    update(); load();
    return {
      update() { if (my.at && Date.now() - my.at > (my.wait || STALE)) load(); },
      dispose() { my.alive = false; el.removeEventListener('click', click); document.removeEventListener('tt-board', changed); document.removeEventListener('tt-watchlist', changed); },
    };
  },
});

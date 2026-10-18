// Day: one market day hour by hour, New York time, within 04:00 to 20:00
// and only as far as something happens (the first and last hours with a
// release, the open or close, earnings or now), with three or more quiet
// hours in a row folded to one line: pre-market, the session and
// after-hours shaded apart, each scheduled
// release at its time, and the day's earnings where they land: before the
// open in the pre-market, after the close in after-hours, and those whose
// time isn't given apart. Opened from the Calendar (a hard click on a day in
// Safari, a double-click, or a long press) as a tab beside it, on that day;
// ‹ › step a day. A symbol points the workspace's charts at it. Releases and
// earnings come from the economic calendar service, the session from the
// market calendar.
import { defineWidget, paint } from '../lib/widgets.js';
import { fleet, svcState } from '../lib/fleet.js';
import { esc } from '../lib/format.js';
import { theme, setFocus, LAYERS } from '../lib/theme.js';

const FIRST = 4, LAST = 20; // hours shown, New York time
const nyToday = () => new Date().toLocaleDateString('en-CA', { timeZone: 'America/New_York' });
const nyNow = () => { const [h, m] = new Date().toLocaleTimeString('en-GB', { timeZone: 'America/New_York', hour: '2-digit', minute: '2-digit' }).split(':').map(Number); return h * 60 + m; };
const step = (day, n) => { const d = new Date(day + 'T12:00:00Z'); d.setUTCDate(d.getUTCDate() + n); return d.toISOString().slice(0, 10); };
const mins = t => { const [h, m] = (t || '').split(':').map(Number); return isFinite(h) ? h * 60 + (m || 0) : null; };
const cap = v => !v ? '' : v >= 1e12 ? `$${(v / 1e12).toFixed(1)}T` : v >= 1e9 ? `$${Math.round(v / 1e9)}B` : `$${Math.round(v / 1e6)}M`;

function session(day) {
  const cal = (fleet.topo?.services || []).map(s => svcState(s.id).calendar).find(c => c?.days?.length);
  const wd = new Date(day + 'T12:00:00Z').getUTCDay(), d = cal?.days.find(x => x.date === day);
  if (wd === 0 || wd === 6) return { open: null, close: null, text: 'Weekend: markets closed' };
  if (!cal) return { open: 570, close: 960, text: 'No market calendar: a normal session assumed' };
  if (day < cal.days[0].date || day > cal.days[cal.days.length - 1].date) return { open: 570, close: 960, text: 'Beyond the market calendar: a normal session assumed' };
  if (!d) return { open: null, close: null, text: 'Holiday: markets closed' };
  return { open: mins(d.open), close: mins(d.close), text: d.close < '16:00' ? `Early close: ${d.open}–${d.close} ET` : `Open ${d.open}–${d.close} ET` };
}

function data(day) {
  const svcs = Object.values(fleet.snap?.services || {});
  const econ = svcs.map(s => s?.econ).find(e => Array.isArray(e?.events));
  const earn = svcs.map(s => s?.earnings).find(e => Array.isArray(e?.reports));
  const evs = (econ?.events || []).filter(e => e.date === day).sort((a, b) => (a.time || '').localeCompare(b.time || ''));
  const reps = (earn?.reports || []).filter(r => r.date === day).sort((a, b) => (b.market_cap || 0) - (a.market_cap || 0));
  return { evs, reps, loaded: !!(econ || earn) };
}

function draw(my) {
  const day = my.day, s = session(day), { evs, reps, loaded } = data(day), watched = new Set(theme()?.symbols || []);
  const label = new Date(day + 'T12:00:00Z').toLocaleDateString(undefined, { timeZone: 'UTC', weekday: 'long', month: 'long', day: 'numeric', year: 'numeric' });
  const layer = id => LAYERS.find(([x]) => x === id)?.[1] || id;
  const rel = e => `<li class="dy-ev ${esc(e.importance || '')}" title="${esc(`${e.time} ${e.title} · ${e.source || ''} · ${e.importance || ''}`)}"><span class="dy-t mono">${esc(e.time || '')}</span><span class="dy-w">${esc(e.title)}</span><span class="dy-s muted">${esc([layer(e.category || 'econ'), e.source].filter(Boolean).join(' · '))}</span></li>`;
  const eps = r => r.eps_actual != null ? `EPS ${r.eps_actual} vs ${r.eps_estimate ?? '?'}` : r.eps_estimate != null ? `est. ${r.eps_estimate}` : '';
  const co = r => `<li class="dy-co ${esc(r.importance || '')}${watched.has(r.symbol) ? ' dy-watch' : ''}"><button type="button" class="dy-sym mono" data-sym="${esc(r.symbol)}" title="Point the workspace's charts at ${esc(r.symbol)}">${esc(r.symbol)}</button><span class="dy-w">${esc(r.name || '')}${r.disputed ? ' <span class="pill p-warn" title="The calendars disagree on the date">dates differ</span>' : ''}</span><span class="dy-s muted mono">${esc([cap(r.market_cap), eps(r)].filter(Boolean).join(' · '))}</span></li>`;
  const block = (title, list) => list.length ? `<div class="dy-earn"><h4>${title} <span class="muted">${list.length}</span></h4><ul>${list.map(co).join('')}</ul></div>` : '';
  const pre = reps.filter(r => r.when === 'before-open'), post = reps.filter(r => r.when === 'after-close'), unknown = reps.filter(r => r.when !== 'before-open' && r.when !== 'after-close');
  // Hours, each with what falls in it; earnings at the pre-market's end and after-hours' start.
  const now = day === nyToday() ? nyNow() : null, open = s.open, close = s.close;
  const phase = h => open == null ? 'closed' : h * 60 < open ? 'pre' : h * 60 >= close ? 'post' : 'rth';
  const off = evs.filter(e => { const m = mins(e.time); return m == null || m < FIRST * 60 || m >= LAST * 60; });
  // Hours with something in them; the rest trimmed from the ends and folded in between.
  const busy = new Set();
  for (const e of evs) { const m = mins(e.time); if (m != null && m >= FIRST * 60 && m < LAST * 60) busy.add(Math.floor(m / 60)); }
  if (open != null) { busy.add(Math.floor(open / 60)); busy.add(Math.floor(close / 60)); if (pre.length) busy.add(Math.floor((open - 1) / 60)); }
  if (now != null && now >= FIRST * 60 && now < LAST * 60) busy.add(Math.floor(now / 60));
  const from = busy.size ? Math.min(...busy) : FIRST, to = busy.size ? Math.max(...busy) + 1 : FIRST;
  const hh = h => `${String(h).padStart(2, '0')}:00`;
  const rows = [];
  for (let h = from; h < to; h++) {
    let q = h; while (q < to && !busy.has(q)) q++;
    if (q - h >= 3) { rows.push(`<div class="dy-hour dy-fold dy-${phase(h)}"><span class="dy-h mono">${hh(h)}</span><div class="dy-slot muted">nothing until ${hh(q)}</div></div>`); h = q - 1; continue; }
    const here = evs.filter(e => { const m = mins(e.time); return m != null && m >= h * 60 && m < (h + 1) * 60; });
    const marks = [];
    if (open != null && open >= h * 60 && open < (h + 1) * 60) marks.push(`<li class="dy-mark">${String(Math.floor(open / 60)).padStart(2, '0')}:${String(open % 60).padStart(2, '0')} open</li>`);
    if (close != null && close >= h * 60 && close < (h + 1) * 60) marks.push(`<li class="dy-mark">${String(Math.floor(close / 60)).padStart(2, '0')}:${String(close % 60).padStart(2, '0')} close</li>`);
    const earnHere = (open != null && Math.floor((open - 1) / 60) === h ? block('Earnings before the open', pre) : '') + (close != null && Math.floor(close / 60) === h ? block('Earnings after the close', post) : '');
    const nowLine = now != null && now >= h * 60 && now < (h + 1) * 60 ? `<span class="dy-now" style="top:${((now - h * 60) / 60 * 100).toFixed(1)}%" title="Now"></span>` : '';
    rows.push(`<div class="dy-hour dy-${phase(h)}${now != null && (h + 1) * 60 <= now ? ' dy-past' : ''}"><span class="dy-h mono">${String(h).padStart(2, '0')}:00</span><div class="dy-slot">${nowLine}${here.length || marks.length ? `<ul>${marks.join('')}${here.map(rel).join('')}</ul>` : ''}${earnHere}</div></div>`);
  }
  // A closed day still lists its earnings, under the hours.
  const closedEarn = open == null ? block('Earnings before the open', pre) + block('Earnings after the close', post) : '';
  const total = reps.reduce((t, r) => t + (r.market_cap || 0), 0);
  return `<div class="dy">
    <div class="dy-head"><button type="button" class="tile-btn" data-dy-step="-1" aria-label="Previous day" title="Previous day">‹</button>
      <div class="dy-title"><h2>${esc(label)}</h2><p class="dy-sess muted mono">${esc(s.text)} · ${evs.length} release${evs.length === 1 ? '' : 's'} · ${reps.length} earnings${total ? ` (${esc(cap(total))})` : ''}</p></div>
      <button type="button" class="btn small" data-dy-today${day === nyToday() ? ' disabled' : ''}>Today</button><button type="button" class="tile-btn" data-dy-step="1" aria-label="Next day" title="Next day">›</button></div>
    ${loaded ? '' : '<p class="muted">The economic calendar service isn\'t serving releases or earnings.</p>'}
    ${off.length ? `<div class="dy-off"><h4>Outside 04:00–20:00</h4><ul>${off.map(rel).join('')}</ul></div>` : ''}
    ${rows.length ? `<div class="dy-grid">${rows.join('')}</div>` : '<p class="muted">Nothing scheduled.</p>'}
    ${closedEarn}${block('Earnings, time not given', unknown)}
  </div>`;
}

defineWidget('day', {
  title: 'Day',
  mount(el) {
    // The day it was opened on (the panel's arguments), else today.
    let args = {}; try { args = JSON.parse(el.dataset.args || '{}'); } catch (_) {}
    const my = { day: /^\d{4}-\d{2}-\d{2}$/.test(args.day || '') ? args.day : nyToday() };
    const update = () => { paint(el, draw(my)); };
    const go = day => { my.day = day; update(); el.dispatchEvent(new CustomEvent('tt-tab-args', { bubbles: true, detail: { args: { day }, title: dayTitle(day) } })); };
    const click = e => {
      const t = /** @type {HTMLElement} */ (e.target);
      const st = t.closest('[data-dy-step]'); if (st) { go(step(my.day, +(/** @type {HTMLElement} */ (st)).dataset.dyStep)); return; }
      if (t.closest('[data-dy-today]')) { go(nyToday()); return; }
      const sym = /** @type {HTMLElement | null} */ (t.closest('[data-sym]'))?.dataset.sym;
      if (sym) setFocus(sym);
    };
    el.addEventListener('click', click);
    update();
    // Each minute, so the now line moves.
    const timer = setInterval(update, 60e3);
    return { update, dispose: () => { el.removeEventListener('click', click); clearInterval(timer); } };
  },
});

/** A Day tab's title: "Day · Oct 13". */
export const dayTitle = day => `Day · ${new Date(day + 'T12:00:00Z').toLocaleDateString(undefined, { timeZone: 'UTC', month: 'short', day: 'numeric' })}`;

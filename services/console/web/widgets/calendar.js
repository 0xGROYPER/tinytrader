import { defineWidget } from '../lib/widgets.js';
import { fleet, svcState } from '../lib/fleet.js';
import { esc, duration } from '../lib/format.js';

// The US market calendar, from the first service that serves one.
// The market calendar as a month: one widget that reads well as a page or
// in a narrow tile (it sizes itself to its container, not the window).
// Short names for scheduled releases, for the month's small cells.
const SHORT = [['Consumer Price Index', 'CPI'], ['Producer Price Index', 'PPI'], ['Employment Situation', 'Jobs'], ['Job Openings and Labor Turnover', 'JOLTS'], ['FOMC decision', 'FOMC'], ['FOMC minutes', 'Minutes'], ['GDP', 'GDP'], ['Gross Domestic Product', 'GDP'], ['Personal Income and Outlays', 'PCE'], ['Employment Cost Index', 'ECI'], ['Initial jobless claims', 'Claims'], ['U.S. International Trade', 'Trade'], ['U.S. Import and Export Price', 'Import px'], ['Productivity and Costs', 'Productivity']];
const short = title => SHORT.find(([p]) => title.startsWith(p))?.[1] || title.split(/[,(]/)[0].trim();

// Scheduled releases from a service serving an `econ` stream, and earnings
// reports from one serving `earnings` (largest companies first), by New York date.
function releases() {
  const svcs = Object.values(fleet.snap?.services || {});
  const econ = svcs.map(s => s.econ).find(e => Array.isArray(e?.events));
  const earn = svcs.map(s => s.earnings).find(e => Array.isArray(e?.reports));
  const by = new Map(), reports = new Map();
  for (const e of econ?.events || []) (by.get(e.date) || by.set(e.date, []).get(e.date)).push(e);
  for (const r of earn?.reports || []) (reports.get(r.date) || reports.set(r.date, []).get(r.date)).push(r);
  for (const list of reports.values()) list.sort((a, b) => (b.market_cap || 0) - (a.market_cap || 0));
  return { by, reports, earnings: !!earn };
}

const cap = n => n == null ? '' : n >= 1e12 ? `$${(n / 1e12).toFixed(1)}T` : `$${Math.round(n / 1e9)}B`;
const when = { 'before-open': 'Before open', 'after-close': 'After close', unknown: 'Time not given' };

function draw(my) {
  const cal = (fleet.topo?.services || []).map(s => svcState(s.id).calendar).find(c => c?.days?.length);
  const head = '<div><h1>Calendar</h1><p class="muted" style="margin:0">US equity market sessions, in New York time.</p></div>';
  if (!cal) return head + `<p class="muted">${(fleet.topo?.services || []).some(s => s.calendar) ? 'Loading the market calendar…' : 'No service is serving a market calendar.'}</p>`;
  const ny = iso => new Date(iso).toLocaleString(undefined, { timeZone: 'America/New_York', weekday: 'short', month: 'short', day: 'numeric', hour: 'numeric', minute: '2-digit' });
  const until = iso => duration((Date.parse(iso) - Date.now()) / 1000);
  // The clock is a reading from when the calendar was fetched; carry it
  // forward past the close or open it named.
  const c = { ...(cal.clock || {}) };
  if (c.is_open && Date.parse(c.next_close) <= Date.now()) c.is_open = false;
  else if (!c.is_open && c.next_open && Date.parse(c.next_open) <= Date.now()) {
    const d = cal.days.find(x => x.date === c.next_open.slice(0, 10));
    if (d) { c.is_open = true; c.next_close = `${d.date}T${d.close}:00${c.next_open.slice(19)}`; }
  }
  const now = c.is_open
    ? `<span class="pill p-ok" title="Closes ${esc(ny(c.next_close))}">Open</span><span class="muted">closes in ${until(c.next_close)}</span>`
    : `<span class="pill p-idle" title="Opens ${esc(ny(c.next_open))}">Closed</span><span class="muted">opens in ${until(c.next_open)}</span>`;
  const sessions = new Map(cal.days.map(d => [d.date, d]));
  const firstDay = cal.days[0].date, lastDay = cal.days[cal.days.length - 1].date;
  const today = new Date().toLocaleDateString('en-CA', { timeZone: 'America/New_York' });
  // Weekdays inside the calendar's range with no session are holidays; a close before 16:00 is an early close.
  const kind = (date, wd) => wd === 0 || wd === 6 ? 'weekend' : date < firstDay || date > lastDay ? 'unknown'
    : !sessions.has(date) ? 'holiday' : sessions.get(date).close < '16:00' ? 'early' : 'open';
  const months = [firstDay.slice(0, 7), lastDay.slice(0, 7)];
  let month = my.month || today.slice(0, 7);
  if (month < months[0]) month = months[0];
  if (month > months[1]) month = months[1];
  my.month = month;
  const [y, m] = month.split('-').map(Number);
  const lead = new Date(Date.UTC(y, m - 1, 1)).getUTCDay(), count = new Date(Date.UTC(y, m, 0)).getUTCDate();
  const cells = ['Sun', 'Mon', 'Tue', 'Wed', 'Thu', 'Fri', 'Sat'].map(d => `<div class="mcal-dow">${d}</div>`);
  for (let i = 0; i < lead; i++) cells.push('<div class="cal-day blank"></div>');
  const rel = releases();
  my.cells = new Map();
  for (let d = 1; d <= count; d++) {
    const date = `${month}-${String(d).padStart(2, '0')}`, wd = (lead + d - 1) % 7, k = kind(date, wd), sess = sessions.get(date);
    // Only what differs from a normal session is written: closed, or an early close.
    const hrs = k === 'holiday' ? 'Closed' : k === 'early' ? `Closes ${sess.close}` : '';
    const evs = rel.by.get(date) || [], reps = rel.reports.get(date) || [];
    const title = [date, hrs || (k === 'weekend' ? 'weekend' : k === 'open' ? `${sess.open}–${sess.close} ET` : ''), ...evs.map(e => `${e.time} ${e.title}`),
      reps.length ? `Earnings: ${reps.map(r => r.symbol).join(', ')}` : ''].filter(Boolean).join('\n');
    // Releases first; earnings as one tag naming the largest company.
    const tags = evs.slice(0, 2).map(e => `<i class="evt ${esc(e.importance)}">${esc(short(e.title))}</i>`).join('') + (evs.length > 2 ? `<i class="evt more">+${evs.length - 2}</i>` : '')
      + (reps.length ? `<i class="evt earn">${esc(reps[0].symbol)}${reps.length > 1 ? ` +${reps.length - 1}` : ''}</i>` : '');
    my.cells.set(date, { hours: hrs || (k === 'open' ? `${sess.open}–${sess.close} ET` : k === 'weekend' ? 'Weekend' : ''), evs, reps });
    cells.push(`<div class="cal-day k-${k}${date === today ? ' is-today' : ''}${date < today ? ' past' : ''}${date === my.pin ? ' picked' : ''}" data-day="${date}" aria-label="${esc(title)}"><span class="n">${d}</span>${hrs ? `<span class="hrs">${esc(hrs)}</span>` : ''}${tags ? `<span class="evts">${tags}</span>` : ''}</div>`);
  }
  while (cells.length % 7) cells.push('<div class="cal-day blank"></div>');
  const label = new Date(Date.UTC(y, m - 1, 15)).toLocaleDateString(undefined, { timeZone: 'UTC', month: 'long', year: 'numeric' });
  const nav = (dir, text, off) => `<button type="button" class="tile-btn" data-cal-nav="${dir}"${off ? ' disabled' : ''} aria-label="${text}" title="${text}">${dir < 0 ? '‹' : '›'}</button>`;
  const weeks = cells.length / 7 - 1;
  return head + `<div class="mcal">
      <div class="mcal-head"><h2>${esc(label)}</h2><span class="mcal-now">${now}</span><span class="mcal-nav">${nav(-1, 'Previous month', month <= months[0])}<button type="button" class="btn small" data-cal-nav="0"${month === today.slice(0, 7) ? ' disabled' : ''}>Today</button>${nav(1, 'Next month', month >= months[1])}</span></div>
      <div class="mcal-grid" style="--weeks:${weeks}">${cells.join('')}</div>
    </div>
`;
}

// A day in full, shown over the month while the day is hovered (or pinned
// by a click): its session, scheduled releases and earnings reports.
function dayCard(day, cell) {
  const label = new Date(day + 'T12:00:00Z').toLocaleDateString(undefined, { timeZone: 'UTC', weekday: 'long', month: 'long', day: 'numeric' });
  const { hours = '', evs = [], reps = [] } = cell || {};
  const eps = r => r.eps_actual != null ? `EPS ${r.eps_actual} (est. ${r.eps_estimate ?? '?'})` : r.eps_estimate != null ? `est. ${r.eps_estimate}` : '';
  const rel = evs.length
    ? `<ul>${evs.map(e => `<li class="${esc(e.importance)}"><span class="t">${esc(e.time)}</span><span class="w">${esc(e.title)}</span><span class="s">${esc(e.source)}</span></li>`).join('')}</ul>`
    : '';
  // Earnings by when they report: before the open, after the close, or not said.
  const row = r => {
    const differ = r.disputed ? Object.entries(r.dates).map(([s, d]) => `${s}: ${d}`).join(', ') : '';
    return `<li class="${esc(r.importance)}"><span class="w"><b>${esc(r.symbol)}</b> ${esc(r.name || '')}${differ ? ` <span class="pill p-warn" title="${esc(differ)}">dates differ</span>` : ''}</span><span class="s">${esc([cap(r.market_cap), eps(r)].filter(Boolean).join(' · '))}</span></li>`;
  };
  const earn = [['before-open', 'Earnings before the open (AM)'], ['after-close', 'Earnings after the close (PM)'], ['unknown', 'Earnings, time not disclosed']]
    .map(([w, title]) => { const list = reps.filter(r => (r.when in when ? r.when : 'unknown') === w); return list.length ? `<h4>${title}</h4><ul class="cp-earn">${list.map(row).join('')}</ul>` : ''; })
    .join('');
  return `<h3>${esc(label)}</h3>${hours ? `<p class="cp-hrs">${esc(hours)}</p>` : ''}${rel}${earn}${!rel && !earn ? '<p class="muted" style="margin:0">Nothing scheduled.</p>' : ''}`;
}

defineWidget('calendar', {
  title: 'Calendar',
  mount(el) {
    const my = { month: null, pin: null, hover: null, cells: new Map() };
    el.style.position = 'relative';
    // The card for the hovered (or pinned) day, beside its cell, inside the widget.
    const card = () => {
      el.querySelector('.cal-pop')?.remove();
      const day = my.hover || my.pin, cellEl = day && el.querySelector(`[data-day="${day}"]`);
      if (!cellEl) return;
      // A card shown only while hovering lets the mouse through, so it never
      // gets in the way of the days beneath; a pinned one can be read and scrolled.
      el.insertAdjacentHTML('beforeend', `<div class="cal-pop${my.hover && my.hover !== my.pin ? ' hovering' : ''}" role="dialog" aria-label="Day details">${dayCard(day, my.cells.get(day))}</div>`);
      const pop = /** @type {HTMLElement} */ (el.querySelector('.cal-pop')), c = cellEl.getBoundingClientRect(), h = el.getBoundingClientRect();
      const w = pop.offsetWidth, room = el.clientWidth;
      let left = c.right - h.left + el.scrollLeft + 6;
      if (left + w > room) left = c.left - h.left + el.scrollLeft - w - 6;
      pop.style.left = `${Math.max(4, Math.min(left, room - w - 4))}px`;
      // Within the visible part of the widget: beside the cell, moved up if it would run off the bottom.
      pop.style.maxHeight = `${Math.min(440, el.clientHeight - 8)}px`;
      const lo = el.scrollTop + 4, hi = el.scrollTop + el.clientHeight - pop.offsetHeight - 4;
      pop.style.top = `${Math.max(lo, Math.min(c.top - h.top + el.scrollTop, hi))}px`;
    };
    const update = () => { const top = el.scrollTop; el.innerHTML = draw(my); el.scrollTop = top; card(); };
    const over = e => {
      if (e.target.closest('.cal-pop')) return;
      const day = e.target.closest('[data-day]')?.dataset.day || null;
      if (day !== my.hover) { my.hover = day; card(); }
    };
    const leave = () => { if (my.hover) { my.hover = null; card(); } };
    const click = e => {
      if (e.target.closest('.cal-pop')) return;
      const day = e.target.closest('[data-day]');
      // A click pins the day's card (for touch, or to read it at length); again unpins.
      if (day) { my.pin = my.pin === day.dataset.day ? null : day.dataset.day; update(); return; }
      if (my.pin && !e.target.closest('[data-cal-nav]')) { my.pin = null; update(); return; }
      const b = e.target.closest('[data-cal-nav]'); if (!b) return;
      const dir = +b.dataset.calNav;
      if (!dir) my.month = null;
      else { const [y, m] = my.month.split('-').map(Number), d = new Date(Date.UTC(y, m - 1 + dir, 1)); my.month = d.toISOString().slice(0, 7); }
      update();
    };
    el.addEventListener('click', click);
    el.addEventListener('mouseover', over);
    el.addEventListener('mouseleave', leave);
    update();
    return { update, dispose: () => { el.removeEventListener('click', click); el.removeEventListener('mouseover', over); el.removeEventListener('mouseleave', leave); } };
  },
});

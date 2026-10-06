import { defineWidget } from '../lib/widgets.js';
import { fleet, svcState } from '../lib/fleet.js';
import { esc, duration } from '../lib/format.js';

// The US market calendar, from the first service that serves one.
// The market calendar as a month: one widget that reads well as a page or
// in a narrow tile (it sizes itself to its container, not the window).
function draw(my) {
  const cal = (fleet.topo?.services || []).map(s => svcState(s.id).calendar).find(c => c?.days?.length);
  const head = '<div><h1>Calendar</h1><p class="muted" style="margin:0">US equity market sessions, in New York time.</p></div>';
  if (!cal) return head + `<p class="muted">${(fleet.topo?.services || []).some(s => s.id === 'accounts') ? 'Loading the market calendar…' : 'No service is serving a market calendar.'}</p>`;
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
  for (let d = 1; d <= count; d++) {
    const date = `${month}-${String(d).padStart(2, '0')}`, wd = (lead + d - 1) % 7, k = kind(date, wd), sess = sessions.get(date);
    const hrs = k === 'holiday' ? 'Closed' : k === 'early' ? `Closes ${sess.close}` : k === 'open' ? `${sess.open}–${sess.close}` : k === 'unknown' ? '—' : '';
    const title = `${date}${hrs ? ' · ' + hrs : k === 'weekend' ? ' · weekend' : ''}`;
    cells.push(`<div class="cal-day k-${k}${date === today ? ' is-today' : ''}${date < today ? ' past' : ''}" title="${esc(title)}"><span class="n">${d}</span><span class="hrs">${esc(hrs)}</span></div>`);
  }
  while (cells.length % 7) cells.push('<div class="cal-day blank"></div>');
  const label = new Date(Date.UTC(y, m - 1, 15)).toLocaleDateString(undefined, { timeZone: 'UTC', month: 'long', year: 'numeric' });
  const nav = (dir, text, off) => `<button type="button" class="tile-btn" data-cal-nav="${dir}"${off ? ' disabled' : ''} aria-label="${text}" title="${text}">${dir < 0 ? '‹' : '›'}</button>`;
  // The next days that aren't a normal session.
  const coming = [];
  for (let t = Date.parse(today + 'T12:00:00Z'); coming.length < 4; t += 86400000) {
    const date = new Date(t).toISOString().slice(0, 10); if (date > lastDay) break;
    const k = kind(date, new Date(t).getUTCDay());
    if (k === 'holiday' || k === 'early') coming.push(`<li><span>${esc(new Date(t).toLocaleDateString(undefined, { timeZone: 'UTC', weekday: 'short', month: 'short', day: 'numeric' }))}</span><span class="${k === 'holiday' ? 'warn' : ''}">${k === 'holiday' ? 'Closed' : 'Closes ' + esc(sessions.get(date).close)}</span></li>`);
  }
  const weeks = cells.length / 7 - 1;
  return head + `<div class="mcal">
      <div class="mcal-head"><h2>${esc(label)}</h2><span class="mcal-now">${now}</span><span class="mcal-nav">${nav(-1, 'Previous month', month <= months[0])}<button type="button" class="btn small" data-cal-nav="0"${month === today.slice(0, 7) ? ' disabled' : ''}>Today</button>${nav(1, 'Next month', month >= months[1])}</span></div>
      <div class="mcal-grid" style="--weeks:${weeks}">${cells.join('')}</div>
    </div>
    ${`${month}-01` <= lastDay && lastDay < `${month}-31` ? `<p class="muted" style="margin:0;font-size:13px">The calendar is known through ${esc(lastDay)}.</p>` : ''}
    <div class="cal-coming"><h3>Coming up</h3>${coming.length ? `<ul>${coming.join('')}</ul>` : `<p class="muted" style="margin:0;font-size:13px">No holidays or early closes through ${esc(lastDay)}.</p>`}</div>`;
}

defineWidget('calendar', {
  title: 'Calendar',
  mount(el) {
    const my = { month: null };
    const update = () => { const top = el.scrollTop; el.innerHTML = draw(my); el.scrollTop = top; };
    const click = e => {
      const b = e.target.closest('[data-cal-nav]'); if (!b) return;
      const dir = +b.dataset.calNav;
      if (!dir) my.month = null;
      else { const [y, m] = my.month.split('-').map(Number), d = new Date(Date.UTC(y, m - 1 + dir, 1)); my.month = d.toISOString().slice(0, 7); }
      update();
    };
    el.addEventListener('click', click);
    update();
    return { update, dispose: () => el.removeEventListener('click', click) };
  },
});

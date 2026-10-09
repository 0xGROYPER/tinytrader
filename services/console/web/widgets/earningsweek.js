// Earnings week: the week's reports as a grid, after Earnings Whispers'
// weekly chart. Monday to Friday across, each day split into before the open
// and after the close (and a small row for reports whose time isn't given);
// in each, tickers by market capitalisation, largest first, the largest drawn
// bigger and the workspace theme's symbols marked. Hover for the company, its
// size and the EPS estimate; click to point the workspace's charts at it.
// Reports come from the economic calendar service (its merged calendars).
import { defineWidget } from '../lib/widgets.js';
import { fleet } from '../lib/fleet.js';
import { esc } from '../lib/format.js';
import { theme, setFocus } from '../lib/theme.js';

const SHOW = 14; // tickers per half-day before "more"
const DAYS = ['Mon', 'Tue', 'Wed', 'Thu', 'Fri'];
const iso = d => d.toISOString().slice(0, 10);

/** Monday of the week holding `d` (a weekend shows the coming week). */
function monday(d) {
  const x = new Date(Date.UTC(d.getFullYear(), d.getMonth(), d.getDate()));
  const wd = x.getUTCDay();
  x.setUTCDate(x.getUTCDate() + (wd === 0 ? 1 : wd === 6 ? 2 : 1 - wd));
  return x;
}

const cap = v => !v ? '' : v >= 1e12 ? `$${(v / 1e12).toFixed(1)}T` : v >= 1e9 ? `$${Math.round(v / 1e9)}B` : `$${Math.round(v / 1e6)}M`;

function reports() {
  const e = Object.values(fleet.snap?.services || {}).map(s => s?.earnings).find(x => Array.isArray(x?.reports));
  return e ? e.reports : null;
}

defineWidget('earningsweek', {
  title: 'Earnings week',
  mount(el) {
    const my = { week: monday(new Date()), open: new Set() };
    const chip = (r, watched) => {
      const size = r.market_cap >= 500e9 ? 'xl' : r.market_cap >= 100e9 ? 'l' : r.market_cap >= 20e9 ? 'm' : 's';
      const tip = [r.name || r.symbol, cap(r.market_cap), r.eps_estimate != null ? `EPS estimate ${r.eps_estimate}` : '', r.disputed ? 'dates disagree across calendars' : '']
        .filter(Boolean).join(' · ');
      return `<button class="ew-chip ew-${size}${watched.has(r.symbol) ? ' ew-watch' : ''}${r.disputed ? ' ew-disputed' : ''}" data-sym="${esc(r.symbol)}" title="${esc(tip)}">${esc(r.symbol)}</button>`;
    };
    const half = (list, key, label, watched) => {
      if (!list.length) return `<div class="ew-half"><p class="ew-label">${label}</p><p class="muted ew-none">—</p></div>`;
      const all = my.open.has(key);
      const shown = all ? list : list.slice(0, SHOW);
      const more = list.length - shown.length;
      return `<div class="ew-half"><p class="ew-label">${label} <span class="muted">${list.length}</span></p><div class="ew-chips">${shown.map(r => chip(r, watched)).join('')}${more > 0 ? `<button class="ew-more" data-key="${key}">+${more} more</button>` : ''}</div></div>`;
    };
    const render = () => {
      const all = reports();
      if (!all) { el.innerHTML = '<p class="muted">No earnings calendar is being served.</p>'; return; }
      const watched = new Set(theme()?.symbols || []);
      const days = DAYS.map((_, i) => { const d = new Date(my.week); d.setUTCDate(d.getUTCDate() + i); return iso(d); });
      const today = iso(new Date());
      const by = (day, when) => all.filter(r => r.date === day && (when === 'other' ? !['before-open', 'after-close'].includes(r.when) : r.when === when))
        .sort((a, b) => (b.market_cap || 0) - (a.market_cap || 0));
      const fri = days[4];
      el.innerHTML = `<div class="ew">
        <div class="ew-bar"><button class="ew-nav" data-step="-7" title="Previous week">‹</button>
          <strong>Week of ${new Date(days[0] + 'T12:00:00Z').toLocaleDateString(undefined, { month: 'short', day: 'numeric' })}</strong>
          <button class="ew-nav" data-step="7" title="Next week">›</button>
          <button class="ew-nav ew-today" data-step="0">This week</button>
          <span class="muted ew-count">${all.filter(r => r.date >= days[0] && r.date <= fri).length} reports</span></div>
        <div class="ew-grid">${days.map((day, i) => {
          const other = by(day, 'other');
          return `<section class="ew-day${day === today ? ' ew-now' : ''}">
            <h4>${DAYS[i]} <span class="muted">${Number(day.slice(8))}</span></h4>
            ${half(by(day, 'before-open'), `${day}b`, 'Before the open', watched)}
            ${half(by(day, 'after-close'), `${day}a`, 'After the close', watched)}
            ${other.length ? half(other, `${day}o`, 'Time not given', watched) : ''}
          </section>`;
        }).join('')}</div></div>`;
    };
    el.addEventListener('click', ev => {
      const t = ev.target.closest('button');
      if (!t) return;
      if (t.dataset.sym) {
        setFocus(t.dataset.sym);
        document.dispatchEvent(new CustomEvent('tt-focus', { detail: t.dataset.sym }));
        return;
      }
      if (t.dataset.key) { my.open.add(t.dataset.key); render(); return; }
      if (t.dataset.step !== undefined) {
        const step = Number(t.dataset.step);
        if (step === 0) my.week = monday(new Date());
        else my.week.setUTCDate(my.week.getUTCDate() + step);
        my.open.clear();
        render();
      }
    });
    render();
    return { update: render };
  },
});

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

const TILES = 4; // the largest in each half-day, as tiles
const SHOW = 24; // tickers per half-day before "more"
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
    const tip = r => [r.name || r.symbol, cap(r.market_cap), r.eps_estimate != null ? `EPS estimate ${r.eps_estimate}` : '', r.disputed ? 'dates disagree across calendars' : '']
      .filter(Boolean).join(' · ');
    const cls = (r, watched) => `${watched.has(r.symbol) ? ' ew-watch' : ''}${r.disputed ? ' ew-disputed' : ''}`;
    const tile = (r, watched) => `<button class="ew-tile${cls(r, watched)}" data-sym="${esc(r.symbol)}" title="${esc(tip(r))}">
        <span class="ew-sym">${esc(r.symbol)}</span><span class="ew-name">${esc((r.name || '').replace(/,?\s+(?:&\s*)?(Inc|Corp|Corporation|Co|Company|Ltd|plc|Holdings|Group)\.?$/i, ''))}</span><span class="ew-cap">${cap(r.market_cap)}</span></button>`;
    const chip = (r, watched) => `<button class="ew-chip${cls(r, watched)}" data-sym="${esc(r.symbol)}" title="${esc(tip(r))}">${esc(r.symbol)}</button>`;
    const half = (list, key, label, kind, watched) => {
      const all = my.open.has(key);
      const rest = list.slice(TILES);
      const shown = all ? rest : rest.slice(0, SHOW);
      const more = rest.length - shown.length;
      return `<div class="ew-half ew-${kind}">
        <p class="ew-label"><span>${label}</span><span class="ew-n">${list.length || ''}</span></p>
        ${list.length ? `<div class="ew-tiles">${list.slice(0, TILES).map(r => tile(r, watched)).join('')}</div>
        <div class="ew-chips">${shown.map(r => chip(r, watched)).join('')}${more > 0 ? `<button class="ew-more" data-key="${key}">+${more}</button>` : ''}</div>`
        : '<p class="ew-none">No reports</p>'}</div>`;
    };
    const day = d => new Date(d + 'T12:00:00Z');
    const render = () => {
      const all = reports();
      if (!all) { el.innerHTML = '<p class="muted">No earnings calendar is being served.</p>'; return; }
      const watched = new Set(theme()?.symbols || []);
      const days = DAYS.map((_, i) => { const d = new Date(my.week); d.setUTCDate(d.getUTCDate() + i); return iso(d); });
      const today = iso(new Date());
      const by = (d, when) => all.filter(r => r.date === d && (when === 'other' ? !['before-open', 'after-close'].includes(r.when) : r.when === when))
        .sort((a, b) => (b.market_cap || 0) - (a.market_cap || 0));
      const inWeek = all.filter(r => r.date >= days[0] && r.date <= days[4]);
      const big = inWeek.filter(r => (r.market_cap || 0) >= 100e9).length;
      const fmt = (d, o) => day(d).toLocaleDateString(undefined, o);
      const range = `${fmt(days[0], { month: 'long', day: 'numeric' })} – ${fmt(days[4], days[0].slice(5, 7) === days[4].slice(5, 7) ? { day: 'numeric' } : { month: 'long', day: 'numeric' })}, ${days[4].slice(0, 4)}`;
      el.innerHTML = `<div class="ew">
        <header class="ew-head">
          <div><p class="ew-kicker">Earnings this week</p><h3 class="ew-title">${range}</h3></div>
          <div class="ew-stats"><span><b>${inWeek.length}</b> reports</span><span><b>${big}</b> over $100B</span></div>
          <nav class="ew-navs"><button class="ew-nav" data-step="-7" title="Previous week">‹</button><button class="ew-nav" data-step="0">This week</button><button class="ew-nav" data-step="7" title="Next week">›</button></nav>
        </header>
        <div class="ew-grid">${days.map((d, i) => {
          const other = by(d, 'other');
          return `<section class="ew-day${d === today ? ' ew-now' : ''}">
            <h4><span class="ew-dow">${fmt(d, { weekday: 'long' })}</span><span class="ew-date">${fmt(d, { month: 'short', day: 'numeric' })}</span></h4>
            ${half(by(d, 'before-open'), `${d}b`, 'Before the open', 'am', watched)}
            ${half(by(d, 'after-close'), `${d}a`, 'After the close', 'pm', watched)}
            ${other.length ? half(other, `${d}o`, 'Time not given', 'tbd', watched) : ''}
          </section>`;
        }).join('')}</div>
        <footer class="ew-foot"><span>Largest first by market capitalisation · outlined: this workspace's symbols · dashed: calendars disagree</span><span>tinytrader · Nasdaq, Finnhub, Alpha Vantage</span></footer>
      </div>`;
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

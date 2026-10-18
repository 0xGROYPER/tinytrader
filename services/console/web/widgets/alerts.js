// Alerts: every price alert (lib/alerts.js), fired ones first, with Re-arm
// and Remove, and a form for a new one on any symbol. Alerts are also set
// from a Watchlist row's dot, a ticker view's Alert button in the header,
// and Search's results; fired ones are listed under the bell.
import { defineWidget, paint } from '../lib/widgets.js';
import { esc, etStamp } from '../lib/format.js';
import { setFocus } from '../lib/theme.js';
import { alerts, describe, alertForm } from '../lib/alerts.js';

const when = t => etStamp(t);

function draw(my) {
  const all = [...alerts()].sort((a, b) => (b.fired || 0) - (a.fired || 0) || a.symbol.localeCompare(b.symbol));
  const rows = all.map(a => `<tr class="${a.fired ? 'fired' : ''}"><td><button type="button" class="linkish mono" data-al-sym="${esc(a.symbol)}">${esc(a.symbol)}</button></td><td>${esc(describe(a).slice(a.symbol.length + 1))}</td>
    <td>${a.fired ? `fired ${esc(when(a.fired))} at ${a.price?.toFixed(2) ?? '?'}` : '<span class="muted">armed</span>'}</td>
    <td class="al-acts">${a.fired ? `<button type="button" class="chip-btn" data-al-rearm="${esc(a.id)}">Re-arm</button>` : ''}<button type="button" class="sb-x" data-al-rm="${esc(a.id)}" aria-label="Remove" title="Remove">×</button></td></tr>`).join('');
  return `<form class="al-new" data-al-pick><input class="wl-in mono" name="s" placeholder="Symbol for a new alert" value="${esc(my.sym)}" autocomplete="off" spellcheck="false" aria-label="Symbol"><button class="btn small">Next</button></form>
    ${my.sym ? alertForm(my.sym, null, 'data-al-cancel') : ''}
    ${all.length ? `<div class="scroll"><table class="pos-tbl al-tbl"><thead><tr><th>Symbol</th><th>When it</th><th>State</th><th></th></tr></thead><tbody>${rows}</tbody></table></div>` : '<p class="muted">No alerts yet.</p>'}
    <p class="muted" style="font-size:11px;margin:0">Checked every 15 s while a console page is open, against IEX prices. Nothing watches with every console page closed.</p>`;
}

defineWidget('alerts', {
  title: 'Alerts',
  mount(el) {
    const my = { sym: '' };
    const update = () => { if (!el.contains(document.activeElement) || document.activeElement?.tagName === 'BUTTON') paint(el, draw(my)); };
    const submit = e => {
      const f = /** @type {HTMLFormElement} */ (e.target); if (!f.matches('[data-al-pick]')) return;
      e.preventDefault();
      const s = /** @type {HTMLInputElement} */ (f.elements.namedItem('s')).value.trim().toUpperCase();
      if (!/^[A-Z][A-Z0-9.]{0,9}$/.test(s)) return;
      my.sym = s; /** @type {HTMLElement} */ (document.activeElement)?.blur?.(); update();
      /** @type {HTMLInputElement | null} */ (el.querySelector('.al-form input'))?.focus();
    };
    const click = e => {
      const t = /** @type {HTMLElement} */ (e.target);
      if (t.closest('[data-al-cancel]')) { my.sym = ''; update(); return; }
      const s = /** @type {HTMLElement | null} */ (t.closest('[data-al-sym]'))?.dataset.alSym;
      if (s) setFocus(s);
    };
    el.addEventListener('submit', submit); el.addEventListener('click', click);
    document.addEventListener('tt-alerts', update);
    update();
    return { update() {}, dispose() { el.removeEventListener('submit', submit); el.removeEventListener('click', click); document.removeEventListener('tt-alerts', update); } };
  },
});

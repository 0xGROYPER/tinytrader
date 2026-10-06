// Prices: one card per series of every panel the topology declares (the
// relays' trades), with the latest value, its change and a chart.
import { defineWidget, redrawing } from '../lib/widgets.js';
import { fleet, svcState, metric, nodesById } from '../lib/fleet.js';
import { esc, fmt, duration } from '../lib/format.js';

// ---------- panels: time series a topology declares ----------
// A panel reads a service's series endpoint (an object of key -> [[unix_ms, value], ...])
// and draws one card per key. Titles, formats and footer metrics come from the topology.
function cards() {
  const sections = [];
  for (const panel of fleet.topo?.panels || []) {
    const series = svcState(panel.service)[panel.source || 'recent'];
    if (!series) continue;
    const svc = nodesById()[panel.service];
    const out = Object.entries(series).sort().map(([key, pts]) => {
      const last = pts.length ? pts[pts.length - 1][1] : null, first = pts.length ? pts[0][1] : null;
      const chg = first != null && last != null ? last - first : null;
      const cls = chg > 0 ? 'pos' : chg < 0 ? 'neg' : '';
      const span = pts.length > 1 ? duration((pts[pts.length - 1][0] - pts[0][0]) / 1000) : '';
      const foot = (panel.footer || []).map(f => {
        const v = metric(panel.service, f.metric, f.match ? { [f.match]: key } : undefined);
        return v == null ? '' : fmt(v, f);
      }).filter(Boolean).join(' · ');
      const pct = first ? ((chg / first) * 100).toFixed(3) : '0';
      return `<article class="card">
        <h3>${esc((panel.title || '{key}').replace('{key}', key))} <span class="muted" style="font-family:var(--ui);font-size:12px">via ${esc(svc?.label || panel.service)}</span></h3>
        <div class="val"><span class="big">${fmt(last, panel)}</span>${chg != null ? `<span class="chg ${cls}">${chg >= 0 ? '+' : '−'}${fmt(Math.abs(chg), { ...panel, prefix: '', suffix: '' })} (${pct}%) ${span}</span>` : ''}</div>
        <div class="chart" data-chart="${esc(panel.service)}|${esc(panel.source || 'recent')}|${esc(key)}" data-decimals="${panel.decimals ?? 2}"></div>
        <div class="foot">${esc(foot)}</div>
      </article>`;
    });
    if (out.length) sections.push(`<section><div class="sec-head"><h2>${esc(panel.heading || 'Live data')}</h2>${panel.note ? `<span class="muted" style="font-size:13px">${esc(panel.note)}</span>` : ''}</div><div class="cards">${out.join('')}</div></section>`);
  }
  return sections.join('');
}

const draw = () => cards() || '<p class="muted">No prices yet: they appear while the market data relays are running (from 07:55 ET for stocks, 09:25 ET for options).</p>';

defineWidget('prices', { title: 'Prices', mount: redrawing(draw) });

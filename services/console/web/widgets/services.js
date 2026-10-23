// Services: every service with its container state, health, uptime,
// restarts and resources, and the tools the deployment runs beside them (the
// topology's `links`, by name: a notebook, a scheduler's UI, ...).
import { defineWidget, redrawing } from '../lib/widgets.js';
import { fleet, svcState, statusOf, statusText, subOf, uptimeOf } from '../lib/fleet.js';
import { esc, bytes, duration } from '../lib/format.js';

// The topology's links, each opened in a new tab; only http(s) URLs.
function toolLinks() {
  const links = Object.entries(fleet.topo?.links || {}).filter(([, u]) => typeof u === 'string' && /^https?:\/\//i.test(u));
  if (!links.length) return '';
  return `<p class="muted" style="margin:4px 0 0">Tools: ${links.map(([name, u]) => `<a href="${esc(u)}" target="_blank" rel="noopener noreferrer">${esc(name)}</a>`).join(' · ')}</p>`;
}

function servicesTable() {
  const rows = (fleet.topo?.services || []).map(svc => {
    const n = { ...svc, kind: 'svc' }, st = statusOf(n), c = svcState(svc.id).container;
    return `<tr><td class="mono" style="color:var(--fg)">${esc(svc.label || svc.id)}</td><td class="mono">${esc(svc.host || 'local')}</td>
      <td><span class="pill ${statusText[st][1]}">${statusText[st][0]}</span></td><td>${esc(subOf(n))}</td>
      <td class="mono">${duration(uptimeOf(svc.id))}</td><td class="mono">${c?.restart_count ?? '—'}</td>
      <td class="mono">${c?.cpu_percent != null ? c.cpu_percent.toFixed(1) + '%' : '—'}</td><td class="mono">${bytes(c?.memory?.usage_bytes)}</td></tr>`;
  }).join('');
  return `<div><h1>Services</h1><p class="muted" style="margin:0">Container state from Docker; health and metrics from each service's own endpoints.</p>${toolLinks()}</div>
  <div class="box scroll"><table><thead><tr><th>Service</th><th>Host</th><th>Status</th><th>Detail</th><th>Up</th><th>Restarts</th><th>CPU</th><th>Memory</th></tr></thead><tbody>${rows}</tbody></table></div>`;
}

defineWidget('services', { title: 'Services', mount: redrawing(servicesTable) });

// Notebook: the deployment's notebook server (JupyterLab), embedded, from the
// topology's `links.notebook`. It runs beside tinytrader, not in it; the
// server must allow the console's origin to frame it.
import { defineWidget } from '../lib/widgets.js';
import { fleet } from '../lib/fleet.js';
import { esc } from '../lib/format.js';

defineWidget('notebook', {
  title: 'Notebook',
  mount(el) {
    let shown = null;
    const update = () => {
      const url = fleet.topo?.links?.notebook || null;
      if (url === shown && el.firstChild) return; // keep the notebook's own state
      shown = url;
      el.innerHTML = url
        ? `<div class="nb"><p class="nb-bar muted"><span>${esc(url)}</span><a href="${esc(url)}" target="_blank" rel="noopener">Open in a tab</a></p><iframe class="nb-frame" src="${esc(url)}" title="Notebook"></iframe></div>`
        : '<p class="muted">No notebook server is configured (the topology’s links.notebook).</p>';
    };
    update();
    return { update };
  },
});

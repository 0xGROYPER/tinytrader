// Terminal: a terminal program in the browser, embedded from the topology's
// `links.terminal`: a GoTTY server (https://github.com/sorenisanerd/gotty)
// running, for example, Emacs. It runs beside tinytrader, on loopback, not in
// it. Click into it to type; the browser keeps some shortcuts (Ctrl-W, -T, -N).
import { defineWidget } from '../lib/widgets.js';
import { fleet } from '../lib/fleet.js';
import { esc } from '../lib/format.js';

defineWidget('terminal', {
  title: 'Terminal',
  mount(el) {
    let shown = null;
    const update = () => {
      const url = fleet.topo?.links?.terminal || null;
      if (url === shown && el.firstChild) return; // keep the session
      shown = url;
      el.innerHTML = url
        ? `<div class="nb"><p class="nb-bar muted"><span>${esc(url)}</span><a href="${esc(url)}" target="_blank" rel="noopener">Open in a tab</a></p><iframe class="nb-frame" src="${esc(url)}" title="Terminal"></iframe></div>`
        : '<p class="muted">No terminal server is configured (the topology’s links.terminal).</p>';
    };
    update();
    return { update };
  },
});

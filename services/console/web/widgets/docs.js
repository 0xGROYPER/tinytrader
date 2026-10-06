// Docs: every document of every collection, by folder, linking to the docs
// pages.
import { defineWidget } from '../lib/widgets.js';
import { collections, groupLabel } from '../lib/docs.js';
import { esc } from '../lib/format.js';

function draw(list) {
  if (!list.length) return '<p class="muted">No documents are configured.</p>';
  return list.map(c => [...new Set(c.docs.map(d => d.group))].map(g => `<section><div class="group-h">${esc(groupLabel(g))}</div><div class="doc-links">${c.docs.filter(d => d.group === g).map(d =>
    `<a href="/docs/#${esc(c.id)}-${d.index}">${esc(d.title)}</a>`).join('')}</div></section>`).join('')).join('');
}

defineWidget('docs', {
  title: 'Docs',
  mount(el) {
    el.innerHTML = '<p class="muted">Loading documents…</p>';
    collections().then(list => { el.innerHTML = draw(list); });
    return {};
  },
});

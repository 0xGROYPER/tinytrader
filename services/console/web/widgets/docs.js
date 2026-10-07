// Docs: the documentation's home. The tinytrader mark, a search over every
// document's title and path, and where to start (each folder's README and
// the tutorials). It is the docs pages' home and every new workspace's
// first panel. A document picked here opens in the docs pages when the
// widget is their home, and in place, with a way back, inside a workspace.
import { defineWidget } from '../lib/widgets.js';
import { collections } from '../lib/docs.js';
import { esc } from '../lib/format.js';

/** @typedef {{ index: number, title: string, path: string, group: string }} Doc */
/** @typedef {{ id: string, name: string, docs: Doc[] }} Collection */

/** Every document, with its collection. @param {Collection[]} list */
const all = list => list.flatMap(c => c.docs.map(d => ({ c, d })));

/** Where to start: READMEs, then tutorials, in path order. @param {Collection[]} list */
function starts(list) {
  const rank = ({ d }) => d.path.endsWith('README.md') ? 0 : /(^|\/)tutorials\//.test(d.path) ? 1 : 2;
  return all(list).filter(x => rank(x) < 2).sort((a, b) => rank(a) - rank(b) || a.d.path.localeCompare(b.d.path));
}

/** Documents whose title or path holds every word of `q`. @param {Collection[]} list @param {string} q */
function search(list, q) {
  const words = q.toLowerCase().split(/\s+/).filter(Boolean);
  if (!words.length) return [];
  return all(list).filter(({ d }) => words.every(w => `${d.title} ${d.path}`.toLowerCase().includes(w))).slice(0, 12);
}

const link = ({ c, d }) => `<a href="/docs/#${esc(c.id)}-${d.index}" data-open-doc="${esc(c.id)}:${d.index}"><b>${esc(d.title)}</b><span>${esc(d.path)}</span></a>`;

/** The search results, or nothing when there's no query. @param {Collection[]} list @param {string} q */
const results = (list, q) => q.trim()
  ? `<div class="dh-results">${search(list, q).map(link).join('') || '<p class="muted">No documents match.</p>'}</div>`
  : '<div class="dh-results" hidden></div>';

/** The header's pixel wordmark, cropped to "tinytrader" (without ".io") and drawn large. */
function mark() {
  const svg = /** @type {SVGSVGElement | null} */ (document.querySelector('svg.wordmark')?.cloneNode(true));
  if (!svg) return '<div class="dh-mark">tinytrader</div>';
  svg.setAttribute('viewBox', '0 0 37 5');
  svg.removeAttribute('width'); svg.removeAttribute('height');
  svg.setAttribute('class', 'dh-mark');
  svg.setAttribute('aria-label', 'tinytrader');
  return svg.outerHTML;
}

/** @param {Collection[]} list @param {string} q */
function home(list, q) {
  const s = starts(list);
  return `<div class="docs-home">
      ${mark()}
      <input class="dh-search" type="search" placeholder="Search the docs" aria-label="Search the docs" value="${esc(q)}" autocomplete="off">
      ${results(list, q)}
      ${s.length ? `<div class="dh-start"><h3>Start here</h3>${s.map(link).join('')}</div>` : ''}
    </div>`;
}

defineWidget('docs', {
  title: 'Docs',
  mount(el) {
    /** @type {Collection[]} */
    let list = [];
    let q = '';
    /** @type {{ c: Collection, d: Doc } | null} */
    let reading = null;
    const draw = () => {
      if (!reading) { el.innerHTML = list.length ? home(list, q) : '<p class="muted">No documents are configured.</p>'; return; }
      el.innerHTML = '<div class="docs-read"><button type="button" class="btn small" data-docs-back>← Docs</button><article class="prose"><p class="muted">Loading…</p></article></div>';
      const { c, d } = reading;
      fetch(`/api/collections/${encodeURIComponent(c.id)}/${d.index}`, { cache: 'no-store' })
        .then(r => r.json())
        .then(x => { const a = el.querySelector('.prose'); if (a && reading?.d === d) a.innerHTML = `<div class="path">${esc(x.path)}</div>${x.html}`; })
        .catch(() => { const a = el.querySelector('.prose'); if (a) a.innerHTML = '<p class="muted">Could not load this document.</p>'; });
    };
    // Typing redraws only the results, so the search box keeps its focus.
    const input = (/** @type {Event} */ e) => {
      const t = /** @type {HTMLInputElement} */ (e.target);
      if (!t.classList?.contains('dh-search')) return;
      q = t.value;
      const box = el.querySelector('.dh-results');
      if (box) box.outerHTML = results(list, q);
    };
    const open = (/** @type {string} */ ref) => {
      const [id, i] = ref.split(':');
      const c = list.find(x => x.id === id), d = c?.docs.find(x => x.index === +i);
      if (!c || !d) return;
      // The docs pages take it when this is their home; otherwise read it here.
      const ev = new CustomEvent('tt-open-doc', { bubbles: true, cancelable: true, detail: { id, index: d.index } });
      if (el.dispatchEvent(ev)) { reading = { c, d }; draw(); }
    };
    const click = (/** @type {MouseEvent} */ e) => {
      const t = /** @type {HTMLElement} */ (e.target);
      if (t.closest('[data-docs-back]')) { reading = null; draw(); return; }
      const a = /** @type {HTMLElement | null} */ (t.closest('[data-open-doc]'));
      if (a && !e.metaKey && !e.ctrlKey) { e.preventDefault(); open(a.dataset.openDoc || ''); }
    };
    const key = (/** @type {KeyboardEvent} */ e) => {
      const t = /** @type {HTMLElement} */ (e.target);
      if (e.key !== 'Enter' || !t.classList?.contains('dh-search')) return;
      const first = /** @type {HTMLElement | null} */ (el.querySelector('.dh-results [data-open-doc]'));
      if (first) open(first.dataset.openDoc || '');
    };
    el.addEventListener('input', input);
    el.addEventListener('click', click);
    el.addEventListener('keydown', key);
    el.innerHTML = '<p class="muted">Loading documents…</p>';
    collections().then(l => { list = l; draw(); });
    return {
      dispose() { el.removeEventListener('input', input); el.removeEventListener('click', click); el.removeEventListener('keydown', key); },
    };
  },
});

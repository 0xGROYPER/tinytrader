import { prof, painted } from './profile.js';
import { collect } from './channels.js';

// The widget registry (decision 0010). A widget registers a title and a
// mount(el) that builds into the element it's given and returns an
// instance: update() after each poll, dispose() when its panel goes.
const registry = new Map();

// What a mount subscribes to on the channels (lib/channels.js) ends with the
// panel: its dispose() ends those subscriptions too.
/** @param {string} name @param {WidgetDef} def */
export function defineWidget(name, def) {
  if (registry.has(name)) throw new Error(`widget ${name} is defined twice`);
  registry.set(name, { ...def, mount(el) {
    const { value: inst, end } = collect(() => def.mount(el));
    const dispose = inst.dispose;
    inst.dispose = () => { dispose?.call(inst); end(); };
    return inst;
  } });
}
/** @type {(name: string) => WidgetDef | undefined} */
export const widget = name => registry.get(name);
export const widgetNames = () => [...registry.keys()];

// Replaces an element's HTML, keeping the scroll position of the element and
// of whatever scrolls around it (a widget's panel scrolls, not the part of it
// that repaints), unless it is unchanged: most polls change nothing a widget
// shows, and rebuilding its DOM every two seconds costs layout, style and
// garbage collection for nothing.
/** @param {HTMLElement} el @param {string} html @returns {boolean} whether it changed */
export function paint(el, html) {
  if (/** @type {any} */ (el).ttHtml === html) { if (prof.on) painted(false); return false; }
  if (prof.on) painted(true);
  const kept = scrolled(el);
  el.innerHTML = html;
  restore(kept);
  /** @type {any} */ (el).ttHtml = html;
  return true;
}

/** Where a scroller goes back to after a repaint: where it is now, unless an
 * earlier repaint left it short of where it was (the content was briefly
 * shorter, so the browser clamped it) and nothing has moved it since.
 * @param {number} now its scrollTop now
 * @param {{ want: number, got: number } | undefined} last what the last repaint wanted and got
 * @returns {number} */
export function scrollTarget(now, last) {
  return last && last.got === now && last.want > now ? last.want : now;
}

/** The element and each element around it that is scrolled (or still owed a
 * scroll from an earlier repaint), with where each should be kept.
 * @param {HTMLElement} el @returns {Array<[HTMLElement, number]>} */
export function scrolled(el) {
  /** @type {Array<[HTMLElement, number]>} */
  const out = [];
  for (let n = el; n; n = n.parentElement) {
    const want = scrollTarget(n.scrollTop, /** @type {any} */ (n).ttScroll);
    if (want > 0) out.push([n, want]);
  }
  return out;
}

/** Puts each scroller back, remembering when the new content was too short to.
 * @param {Array<[HTMLElement, number]>} kept */
export function restore(kept) {
  for (const [n, want] of kept) {
    if (n.scrollTop !== want) n.scrollTop = want;
    const got = n.scrollTop;
    /** @type {any} */ (n).ttScroll = got < want ? { want, got } : undefined;
  }
}

/** Holds a widget's repaints while a pointer is pressed on its content, so a
 * repaint can't replace the box or button under a click before the click
 * lands (the click would then hit nothing). Released just after the click (and
 * at most `cap` ms after the press, should the release never arrive), with
 * one repaint then if any was held.
 * @param {HTMLElement} el
 * @param {() => void} flush
 * @param {{ doc?: Pick<Document, 'addEventListener' | 'removeEventListener'>, later?: (f: () => void, ms: number) => unknown, cap?: number }} [opts]
 */
export function pressHold(el, flush, { doc = document, later = (f, ms) => setTimeout(f, ms), cap = 2000 } = {}) {
  let down = false, missed = false, seq = 0;
  const end = s => { if (s !== seq || !down) return; down = false; if (missed) { missed = false; flush(); } };
  // A press on the panel itself (its scrollbar) holds nothing.
  const press = e => { if (e.target === el) return; down = true; const s = ++seq; later(() => end(s), cap); };
  const release = () => { if (!down) return; const s = seq; later(() => end(s), 0); };
  el.addEventListener('pointerdown', press);
  doc.addEventListener('pointerup', release, true); doc.addEventListener('pointercancel', release, true);
  return {
    /** Whether a repaint must wait: if so, it is made on release. */
    held() { if (down) missed = true; return down; },
    dispose() { el.removeEventListener('pointerdown', press); doc.removeEventListener('pointerup', release, true); doc.removeEventListener('pointercancel', release, true); },
  };
}

// Mounts for a widget that redraws its HTML on every update, keeping its
// scroll position. `draw` returns the HTML; `wire(el)`, if given, attaches
// listeners once, to the element only.
/**
 * @param {() => string} draw
 * @param {(el: HTMLElement, update: () => void) => (() => void) | undefined} [wire]
 * @returns {(el: HTMLElement) => WidgetInstance}
 */
export function redrawing(draw, wire) {
  return el => {
    const update = () => { paint(el, draw()); };
    const off = wire?.(el, update);
    update();
    return { update, dispose: () => off?.() };
  };
}

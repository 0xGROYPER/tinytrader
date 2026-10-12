// The widget registry (decision 0010). A widget registers a title and a
// mount(el) that builds into the element it's given and returns an
// instance: update() after each poll, dispose() when its panel goes.
const registry = new Map();

/** @param {string} name @param {WidgetDef} def */
export function defineWidget(name, def) {
  if (registry.has(name)) throw new Error(`widget ${name} is defined twice`);
  registry.set(name, def);
}
/** @type {(name: string) => WidgetDef | undefined} */
export const widget = name => registry.get(name);
export const widgetNames = () => [...registry.keys()];

// Replaces an element's HTML, keeping its scroll position, unless it is
// unchanged: most polls change nothing a widget shows, and rebuilding its DOM
// every two seconds costs layout, style and garbage collection for nothing.
/** @param {HTMLElement} el @param {string} html @returns {boolean} whether it changed */
export function paint(el, html) {
  if (/** @type {any} */ (el).ttHtml === html) return false;
  const top = el.scrollTop; el.innerHTML = html; el.scrollTop = top;
  /** @type {any} */ (el).ttHtml = html;
  return true;
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

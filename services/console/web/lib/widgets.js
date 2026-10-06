// The widget registry (decision 0010). A widget registers a title and a
// mount(el) that builds into the element it's given and returns an
// instance: update() after each poll, dispose() when its panel goes.
const registry = new Map();

export function defineWidget(name, def) {
  if (registry.has(name)) throw new Error(`widget ${name} is defined twice`);
  registry.set(name, def);
}
export const widget = name => registry.get(name);
export const widgetNames = () => [...registry.keys()];

// Mounts for a widget that redraws its HTML on every update, keeping its
// scroll position. `draw` returns the HTML; `wire(el)`, if given, attaches
// listeners once, to the element only.
export function redrawing(draw, wire) {
  return el => {
    const update = () => { const top = el.scrollTop; el.innerHTML = draw(); el.scrollTop = top; };
    const off = wire?.(el, update);
    update();
    return { update, dispose: () => off?.() };
  };
}

// paint() and pressHold() (web/lib/widgets.js), in Node with stand-in elements:
// `node --test services/console/tests/` (also checks.console-js in Nix).
import { test } from 'node:test';
import assert from 'node:assert/strict';

Object.assign(globalThis, { window: globalThis, requestAnimationFrame: () => 0, addEventListener() {}, document: { addEventListener() {}, removeEventListener() {}, querySelector: () => null } });
const { paint, scrollTarget, pressHold } = await import('../web/lib/widgets.js');

/** An element as tall as its HTML (one px a character) or its children, in a
 * box `view` px tall: a scroller clamps its scrollTop as a browser does. */
function box(view = Infinity, parent = null) {
  const kids = [];
  const el = {
    parentElement: parent, html: '', top: 0,
    get height() { return this.html.length + kids.reduce((x, k) => x + k.height, 0); },
    get innerHTML() { return this.html; }, set innerHTML(h) { this.html = h; this.top = Math.min(this.top, this.max); },
    get max() { return Math.max(0, this.height - view); },
    get scrollTop() { return Math.min(this.top, this.max); }, set scrollTop(v) { this.top = Math.max(0, Math.min(v, this.max)); },
  };
  if (parent) parent.kids.push(el);
  el.kids = kids;
  return el;
}
// Repainting a part also re-clamps what scrolls around it, as a layout does.
const settle = (...els) => { for (const e of els) e.top = Math.min(e.top, e.max); };

test('the panel that scrolls keeps its place when a part of it repaints', () => {
  // The Accounts widget: the panel body scrolls, .acct-main (which repaints) does not.
  const panel = box(100), main = box(Infinity, panel);
  paint(main, 'x'.repeat(500));
  panel.scrollTop = 380; // scrolled down to Liquidate
  paint(main, 'y'.repeat(500)); settle(panel);
  assert.equal(panel.scrollTop, 380);
});

test('a repaint that is briefly shorter does not leave the panel at the top', () => {
  const panel = box(100), main = box(Infinity, panel);
  paint(main, 'x'.repeat(500));
  panel.scrollTop = 380;
  // One poll draws much less (a card waiting for its reading): the browser clamps the panel.
  paint(main, 'short'); settle(panel);
  assert.equal(panel.scrollTop, 0, 'clamped while the content is short');
  paint(main, 'z'.repeat(500)); settle(panel);
  assert.equal(panel.scrollTop, 380, 'back where the operator left it once the content is back');
});

test('the operator scrolling after a short repaint wins over the remembered place', () => {
  const panel = box(100), main = box(Infinity, panel);
  paint(main, 'x'.repeat(500)); panel.scrollTop = 380;
  paint(main, 'x'.repeat(150)); settle(panel);
  assert.equal(panel.scrollTop, 50);
  panel.scrollTop = 10; // scrolled up by hand
  paint(main, 'z'.repeat(500)); settle(panel);
  assert.equal(panel.scrollTop, 10);
});

test('scrollTarget: where a scroller goes back to', () => {
  assert.equal(scrollTarget(120, undefined), 120);
  assert.equal(scrollTarget(50, { want: 380, got: 50 }), 380, 'left short by a repaint, not moved since');
  assert.equal(scrollTarget(10, { want: 380, got: 50 }), 10, 'moved since: where it is');
});

test('an unchanged repaint touches nothing', () => {
  const panel = box(100), main = box(Infinity, panel);
  assert.equal(paint(main, 'x'.repeat(500)), true);
  panel.scrollTop = 200;
  assert.equal(paint(main, 'x'.repeat(500)), false);
  assert.equal(panel.scrollTop, 200);
});

/** A stand-in for addEventListener on an element or the document. */
function target() {
  const on = new Map();
  return { on, addEventListener(t, f) { on.set(t, f); }, removeEventListener(t) { on.delete(t); }, fire(t, e = {}) { on.get(t)?.(e); } };
}

test('pressHold: repaints wait while a box or button is pressed, then one repaint follows the click', () => {
  const el = target(), doc = target(), timers = [];
  let flushed = 0;
  const h = pressHold(/** @type {any} */ (el), () => flushed++, { doc, later: (f, ms) => timers.push({ f, ms }) });
  assert.equal(h.held(), false, 'nothing pressed: repaint');
  el.fire('pointerdown', { target: {} }); // a Liquidate checkbox
  assert.equal(h.held(), true, 'a poll while pressed waits');
  doc.fire('pointerup');
  assert.equal(h.held(), true, 'still held until the click has landed');
  timers.filter(t => t.ms === 0).forEach(t => t.f());
  assert.equal(h.held(), false);
  assert.equal(flushed, 1, 'the held repaint is made');
  timers.forEach(t => t.f()); // the cap firing late does nothing more
  assert.equal(flushed, 1);
});

test('pressHold: a lost release is capped; a press on the scrollbar holds nothing', () => {
  const el = target(), doc = target(), timers = [];
  let flushed = 0;
  const h = pressHold(/** @type {any} */ (el), () => flushed++, { doc, later: (f, ms) => timers.push({ f, ms }), cap: 2000 });
  el.fire('pointerdown', { target: el });
  assert.equal(h.held(), false, 'the panel itself (its scrollbar)');
  el.fire('pointerdown', { target: {} });
  h.held();
  timers.find(t => t.ms === 2000).f();
  assert.equal(h.held(), false, 'released at the cap with no pointerup');
  assert.equal(flushed, 1);
  h.dispose();
  assert.equal(el.on.size + doc.on.size, 0, 'listeners removed');
});

// The Lists widget's drawing (web/widgets/lists.js), in Node:
// `node --test services/console/tests/` (also checks.console-js in Nix).
import { test } from 'node:test';
import assert from 'node:assert/strict';

// Just enough of a browser for the page's modules to load.
Object.assign(globalThis, { window: globalThis, requestAnimationFrame: () => 0, addEventListener() {}, document: { addEventListener() {}, querySelector: () => null } });
const { draw } = await import('../web/widgets/lists.js');

const at = Date.UTC(2026, 9, 16, 18, 30, 5); // 14:30:05 in New York
const list = extra => ({ id: 'a', label: 'A', columns: [{ key: 'name', label: 'Name' }, { key: 'n', label: 'N' }], rows: [], updated_at: at, age_ms: 0, error: null, ...extra });
const my = (lists, extra) => ({ lists, pick: '', err: '', at, ...extra });

test('a list is a table of its columns, every value escaped, with when it was read', () => {
  const rows = [{ name: '<img src=x onerror=alert(1)>', n: 1 }, { name: 'b', n: { deep: '"q"' } }, {}];
  const html = draw(my([list({ rows, columns: [{ key: 'name', label: '<Name>' }, { key: 'n', label: 'N' }] })]));
  assert.ok(!html.includes('<img'), html);
  assert.match(html, /<th>&lt;Name&gt;<\/th><th>N<\/th>/);
  assert.match(html, /<td class="mono">&lt;img src=x onerror=alert\(1\)&gt;<\/td><td class="mono">1<\/td>/);
  assert.match(html, /<td class="mono">\{&quot;deep&quot;:&quot;\\&quot;q\\&quot;&quot;\}<\/td>/);
  assert.match(html, /<tr><td class="mono"><\/td><td class="mono"><\/td><\/tr>/, 'a missing value is an empty cell');
  assert.match(html, /Updated 14:30:05 ET/);
  assert.ok(!html.includes('msg-err') && !html.includes('<select'), html);
  assert.match(draw(my([list({ age_ms: 65_000 })])), /Updated 14:29:00 ET/);
  assert.match(draw(my([list()])), /Nothing listed\./);
});

test("a source's failure is said, with or without rows from before", () => {
  const stale = draw(my([list({ rows: [{ name: 'kept' }], age_ms: 30_000, error: 'no answer within 3000 ms' })]));
  assert.match(stale, /<p class="msg-err">no answer within 3000 ms\. Showing what it last listed\.<\/p>/);
  assert.match(stale, /kept/);
  const never = draw(my([list({ updated_at: null, age_ms: null, error: 'the source answered <500>' })]));
  assert.match(never, /<p class="msg-err">the source answered &lt;500&gt;<\/p>/);
  assert.match(never, /Not read yet\./);
  assert.ok(!never.includes('Nothing listed'), never);
});

test('the console itself failing, loading, and no lists configured', () => {
  assert.match(draw(my(null)), /Loading/);
  assert.equal(draw(my(null, { err: 'The lists could not be read: 502' })), '<p class="msg-err">The lists could not be read: 502</p>');
  assert.match(draw(my([], {})), /No lists are configured/);
  const kept = draw(my([list({ rows: [{ name: 'x' }] })], { err: 'The lists could not be read: offline' }));
  assert.match(kept, /<td class="mono">x<\/td>/);
  assert.match(kept, /offline/);
});

test('with several lists, a picker shows the one picked, or else the first', () => {
  const two = [list({ rows: [{ name: 'from a' }] }), list({ id: 'b"', label: 'B & co', rows: [{ name: 'from b' }] })];
  const picked = draw(my(two, { pick: 'b"' }));
  assert.match(picked, /<option value="a">A<\/option><option value="b&quot;" selected>B &amp; co<\/option>/);
  assert.match(picked, /from b/);
  assert.ok(!picked.includes('from a'));
  assert.match(draw(my(two, { pick: 'gone' })), /from a/);
  assert.ok(!draw(my([list()])).includes('<select'));
});

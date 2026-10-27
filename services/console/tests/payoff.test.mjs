// The Spread builder's payoff (web/lib/payoff.js), in Node:
// `node --test services/console/tests/` (also checks.console-js in Nix).
// A leg without a quote must never blank the expiry curve.
import { test } from 'node:test';
import assert from 'node:assert/strict';

const { analyse, priceLeg, midOf, extent, linePath } = await import('../web/lib/payoff.js');

// A live XSP call butterfly, held: +1 772C at 1.04, −2 773C at 0.25, +1 774C at 0.03.
// A 0.57 debit: at most $57 lost, $43 made at 773.
const fly = (q = {}, extra = {}) => [
  { type: 'call', strike: 772, side: 1, qty: 1, cost: 1.04, q: q[772], ...extra[772] },
  { type: 'call', strike: 773, side: -1, qty: 2, cost: 0.25, q: q[773], ...extra[773] },
  { type: 'call', strike: 774, side: 1, qty: 1, cost: 0.03, q: q[774], ...extra[774] },
];
const T = 2 / 365;
const close = (a, b, msg) => assert.ok(Math.abs(a - b) < 1e-9, `${msg}: ${a} ≠ ${b}`);
const allFinite = (xs, msg) => assert.ok(xs.every(Number.isFinite), `${msg}: ${xs.find(x => !Number.isFinite(x))}`);
const grid = (lo, hi, n = 240) => Array.from({ length: n + 1 }, (_, i) => lo + (hi - lo) * i / n);

test('a butterfly with every quote missing (and no underlying price) still has its expiry curve', () => {
  const a = analyse({ legs: fly(), spot: null, t: T });
  assert.ok(a, 'analysed');
  close(a.net, 0.57, 'the debit, from the entry prices');
  close(a.maxL, -0.57, 'max loss: the debit');
  close(a.maxP, 0.43, 'max profit: the width less the debit');
  close(a.pay(773), 0.43, 'the most at the body');
  close(a.pay(760), -0.57, 'below the wings');
  close(a.pay(790), -0.57, 'above the wings');
  assert.deepEqual(a.be.map(b => +b.toFixed(2)), [772.57, 773.43], 'breakevens');
  allFinite(grid(760, 786).map(a.pay), 'the expiry curve');
  // What needs a quote is said unknown, not NaN.
  assert.equal(a.worth, null, 'worth now unknown');
  assert.equal(a.today, null, 'no today curve');
  assert.equal(a.unpriced.length, 3);
  assert.equal(a.pop, null, 'no chance of profit without the underlying');
});

test('missing quotes as NaN, null, zero, one-sided or crossed bid/ask price nothing and break nothing', () => {
  const bad = { 772: { bid: NaN, ask: NaN }, 773: { bid: null, ask: 0.3 }, 774: { bid: 0, ask: 0 } };
  for (const q of [bad, { 772: { bid: 1.2, ask: 1.0 }, 773: { bid: 0.25, ask: null }, 774: { bid: undefined, ask: 'N/A' } }]) {
    for (const v of Object.values(q)) assert.equal(midOf(v), null);
    const a = analyse({ legs: fly(q), spot: 773.2, t: T, ivAt: () => null });
    close(a.net, 0.57, 'the debit');
    close(a.maxP, 0.43, 'max profit');
    close(a.maxL, -0.57, 'max loss');
    allFinite(grid(760, 786).map(a.pay), 'the expiry curve');
    assert.equal(a.today, null, 'no today curve without a price for every leg');
    assert.equal(a.worth, null);
    allFinite([a.cross], 'crossing the spread');
  }
});

test('quoted and unquoted legs: mid, then last, then the broker\'s mark, then none', () => {
  const q = { 772: { bid: 1.02, ask: 1.06, iv: 0.14 }, 773: { bid: null, ask: null, last: 0.27 } };
  const a = analyse({ legs: fly(q, { 774: { mark: 0.02 } }), spot: 773.1, t: T, ivAt: () => null });
  assert.deepEqual(a.legs.map(l => l.from), ['mid', 'last', 'mark']);
  close(a.legs[0].px, 1.04, 'the mid');
  close(a.net, 0.57, 'the cost is what was paid, whatever the quotes');
  close(a.worth, 1.04 - 2 * 0.27 + 0.02, 'worth now from mid, last and mark');
  assert.equal(a.stale.length, 2);
  // Every leg has a price and a volatility (its quote's, or implied from its last or mark): a today curve.
  assert.ok(a.today, 'today curve');
  allFinite(grid(765, 781).map(a.today), 'the today curve');
  assert.ok(a.pop > 0 && a.pop < 1, 'a chance of profit');

  // One leg with nothing at all: today and worth go, the expiry curve stays.
  const b = analyse({ legs: fly({ 772: q[772] }), spot: 773.1, t: T, ivAt: () => null });
  assert.deepEqual(b.unpriced.map(l => l.strike), [773, 774]);
  assert.equal(b.today, null);
  assert.equal(b.worth, null);
  close(b.maxP, 0.43, 'max profit');
});

test('a leg not held with no price at all: the cost is unknown, said so, and nothing is NaN', () => {
  const legs = fly({ 772: { bid: 1.02, ask: 1.06 }, 773: { bid: 0.24, ask: 0.26 } }).map(({ cost, ...l }) => l);
  const a = analyse({ legs, spot: 773, t: T, ivAt: () => null });
  assert.equal(a.net, null);
  assert.deepEqual(a.unpaid.map(l => l.strike), [774]);
  assert.equal(a.today, null);
  allFinite(grid(760, 786).map(a.pay), 'the shape at expiry, before the cost');
  // Priced from the strikes' volatility once there is one.
  assert.equal(priceLeg(legs[2], { spot: 773, t: T, ivAt: () => 0.15 }).from, 'model');
});

test('a planned butterfly at its quotes prices as before', () => {
  const legs = fly({ 772: { bid: 1.02, ask: 1.06, iv: 0.15 }, 773: { bid: 0.24, ask: 0.26, iv: 0.14 }, 774: { bid: 0.02, ask: 0.04, iv: 0.15 } }).map(({ cost, ...l }) => l);
  const a = analyse({ legs, spot: 773, t: T });
  close(a.net, 0.57, 'debit at the mids');
  close(a.maxP, 0.43, 'max profit');
  close(a.cross, 1.06 - 2 * 0.24 + 0.04, 'crossing the spread');
  assert.ok(a.today && Number.isFinite(a.today(773)));
});

test('no NaN reaches the chart\'s scale or path', () => {
  assert.deepEqual(extent([1, NaN, -2], null, [Infinity, 3]), [-2, 3]);
  const d = linePath([0, 1, 2, 3], [0, NaN, 1, 2], x => x * 10, y => y * 10);
  assert.equal(d, 'M0.0,0.0M20.0,10.0L30.0,20.0');
  assert.doesNotMatch(d, /NaN|Infinity/);
  assert.equal(linePath([0, 1], null, x => x, y => y), '');
});

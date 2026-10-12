// The options position being built, shared between the Spread builder (which
// prices it from the chain) and Candles (which draws it on the chart when
// shown, and moves a leg's strike when its line is dragged). The builder
// publishes it ('tt-position'); a chart edits a leg ('tt-position-edit') and
// the builder reprices and publishes again.

/**
 * @typedef {{ type: 'call' | 'put', strike: number, side: 1 | -1, qty: number }} Leg
 * @typedef {{ sym: string | null, expiry: string | null, legs: Leg[], strikes: { call: number[], put: number[] },
 *   shown: boolean, net: number, be: number[], pay: ((s: number) => number) | null }} Position
 */
/** @type {Position} */
export const position = { sym: null, expiry: null, legs: [], strikes: { call: [], put: [] }, shown: false, net: 0, be: [], pay: null };

/** @param {Partial<Position>} p */
export function publish(p) {
  Object.assign(position, p);
  document.dispatchEvent(new CustomEvent('tt-position'));
}

/** Move leg `i` to the listed strike nearest `price`; true if it changed. */
export function dragLeg(i, price) {
  const l = position.legs[i], ks = l && position.strikes[l.type];
  if (!ks?.length) return false;
  const k = ks.reduce((b, x) => Math.abs(x - price) < Math.abs(b - price) ? x : b, ks[0]);
  if (k === l.strike) return false;
  document.dispatchEvent(new CustomEvent('tt-position-edit', { detail: { i, strike: k } }));
  return true;
}

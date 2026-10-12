// The options position being built, shared between the Spread builder (which
// prices it from the chain) and Candles (which draws it on the chart when
// shown). The builder publishes it ('tt-position').

/**
 * @typedef {{ type: 'call' | 'put', strike: number, side: 1 | -1, qty: number, cost?: number }} Leg (`cost`: the entry price of a leg held)
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

/** A recommendation asked for (by the screener), kept until a builder takes it. */
export const wanted = {
  /** @type {{ sym: string, view?: string, row?: any, earnings?: string | null } | null} */ ask: null,
  /** Positions held, to load as legs at their entry prices (from Accounts). */
  /** @type {{ sym: string, expiry: string, account: string, legs: (Leg & { cost: number })[] } | null} */ held: null,
};

/** Ask the Spread builder to show positions held (their payoff at entry prices). */
export function askHeld(held) {
  wanted.held = held;
  document.dispatchEvent(new CustomEvent('tt-held'));
}

/** Ask the Spread builder for a suggested position on `ask.sym`. */
export function askRecommendation(ask) {
  wanted.ask = ask;
  document.dispatchEvent(new CustomEvent('tt-recommend'));
}

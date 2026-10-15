// Bars for charts, kept in the page: one sorted series per (service, symbol,
// bar size), shared by every Candles panel and kept across symbol switches
// and remounts, so going back to a chart costs a refresh of its last bars,
// not its history. A series grows at the end as bars arrive and at the start
// as a reader scrolls back (`older`, with the service's `end` parameter);
// the 12 most recently used series are kept, each up to 60,000 bars.
import { get } from './cache.js';

const MAX_SERIES = 12, MAX_BARS = 60_000;
// Trading days per request (weeks for weekly bars): the first load, each
// older chunk, and a refresh of the latest.
/** @type {Record<number, [number, number, number]>} */
const CHUNK = { 60: [2, 3, 1], 300: [10, 10, 1], 900: [20, 30, 1], 3600: [60, 120, 2], 86400: [400, 750, 5], 604800: [260, 520, 2] };

/**
 * @typedef {number[]} Bar [unix seconds, open, high, low, close, volume]
 * @typedef {{ bars: Bar[], done: boolean, note: string | null, tail: Promise<void> | null, back: Promise<number> | null }} Series
 */
/** @type {Map<string, Series>} */
const all = new Map();

/** The series for a key, most recently used last (the first is evicted). */
function series(service, symbol, size) {
  const key = `${service}|${symbol}|${size}`;
  let s = all.get(key);
  if (s) all.delete(key);
  else s = { bars: [], done: false, note: null, tail: null, back: null };
  all.set(key, s);
  while (all.size > MAX_SERIES) all.delete(/** @type {string} */ (all.keys().next().value));
  return s;
}

async function fetchBars(service, symbol, size, days, end) {
  const q = `symbol=${encodeURIComponent(symbol)}&days=${days}&interval=${size}${end ? `&end=${end}` : ''}`;
  const v = await get(`/api/bars/${encodeURIComponent(service)}?${q}`, { fresh: 2000 });
  return { bars: /** @type {Bar[]} */ (v.bars || []), note: v.note || null };
}

/** The series as held now, without fetching (empty if never loaded). */
export function held(service, symbol, size) {
  return all.get(`${service}|${symbol}|${size}`)?.bars || [];
}

/**
 * The series with its latest bars: the first chunk if empty, else a refresh
 * of the last session's, merged by time. Concurrent callers share one fetch.
 * @returns {Promise<Series>}
 */
export async function latest(service, symbol, size) {
  const s = series(service, symbol, size);
  if (!s.tail) {
    const [first, , tail] = CHUNK[size] || CHUNK[60];
    s.tail = (async () => {
      const got = await fetchBars(service, symbol, size, s.bars.length ? tail : first);
      s.note = got.note;
      if (!got.bars.length) return;
      if (!s.bars.length) { s.bars = got.bars; return; }
      // Replace from the first refreshed bar on (the last bar is still forming).
      const t0 = got.bars[0][0];
      let i = s.bars.length;
      while (i > 0 && s.bars[i - 1][0] >= t0) i--;
      s.bars = s.bars.slice(0, i).concat(got.bars);
      if (s.bars.length > MAX_BARS) { s.bars = s.bars.slice(-MAX_BARS); s.done = false; }
    })().finally(() => { s.tail = null; });
  }
  await s.tail;
  return s;
}

/**
 * Extends the series back by one chunk; resolves to how many bars were
 * added (0 when the source has nothing older, after which `done` is set).
 * @returns {Promise<number>}
 */
export async function older(service, symbol, size) {
  const s = series(service, symbol, size);
  if (s.done || !s.bars.length || s.bars.length >= MAX_BARS) return 0;
  if (!s.back) {
    const [, chunk] = CHUNK[size] || CHUNK[60];
    s.back = (async () => {
      const first = s.bars[0][0];
      const got = await fetchBars(service, symbol, size, chunk, first);
      // A source without `end` sends its latest again: nothing older there.
      const add = got.bars.filter(b => b[0] < first);
      if (!add.length) { s.done = true; return 0; }
      s.bars = add.concat(s.bars);
      return add.length;
    })().finally(() => { s.back = null; });
  }
  return s.back;
}

/** Whether a series has reached the start of its source's history. */
export const exhausted = (service, symbol, size) => !!all.get(`${service}|${symbol}|${size}`)?.done;

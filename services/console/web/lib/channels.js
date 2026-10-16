// Channels: the values widgets share (decision 0010; the linked channels of
// 0008 build on them). A channel holds its latest value. publish() sets it and
// tells its subscribers, only when it changed. subscribe() also gives the
// current value at once, so a widget added later starts from what's current;
// onChange() gives only later changes. A subscription made while a widget
// mounts ends when the widget is disposed (collect(), used by defineWidget).
// The names and shapes are in types/console.d.ts (interface Channels).
import { prof, channelPublished } from './profile.js';

/** The account picked for trading, as this browser kept it. */
const storedAccount = () => { try { return localStorage.getItem('tt-sb-account') || ''; } catch (_) { return ''; } };
/** @type {Record<string, { value: any, subs: Set<(v: any) => void> }>} */
const channels = {};
/** The channel, made at its first use. @param {string} name */
const chan = name => channels[name] ??= { value: name === 'account' ? storedAccount() : null, subs: new Set() };

/** The subscriptions being made by the widget that is mounting, if one is. @type {(() => void)[] | null} */
let mounting = null;

/** Sets a channel's value and tells its subscribers, if it changed.
 * @template {keyof Channels} K @param {K} name @param {Channels[K]} value */
export function publish(name, value) {
  const c = chan(name);
  if (Object.is(c.value, value)) return;
  c.value = value;
  if (prof.on) channelPublished(name);
  for (const fn of [...c.subs]) if (c.subs.has(fn)) fn(value);
}

/** A channel's current value. @template {keyof Channels} K @param {K} name @returns {Channels[K]} */
export const read = name => chan(name).value;

/** Calls `fn` with the current value now, and at each change. Returns how to stop.
 * @template {keyof Channels} K @param {K} name @param {(v: Channels[K]) => void} fn */
export function subscribe(name, fn) {
  chan(name).subs.add(fn);
  fn(chan(name).value);
  return keep(name, fn);
}

/** Calls `fn` at each change, not now (for a widget that already has the value). Returns how to stop.
 * @template {keyof Channels} K @param {K} name @param {(v: Channels[K]) => void} fn */
export function onChange(name, fn) {
  chan(name).subs.add(fn);
  return keep(name, fn);
}

// Ends with the mount that made it, if there is one; else only by its own stop.
/** @param {string} name @param {(v: any) => void} fn */
function keep(name, fn) {
  const off = () => { chan(name).subs.delete(fn); };
  if (mounting) mounting.push(off);
  return off;
}

/** Runs a widget's mount, collecting the subscriptions it makes. `end` ends them.
 * @template T @param {() => T} mount */
export function collect(mount) {
  const mine = [], outer = mounting;
  mounting = mine;
  try { return { value: mount(), end: () => mine.forEach(off => off()) }; } finally { mounting = outer; }
}

// TEMPORARY, until step 3 moves their publishers to channels: the events that
// still carry these values. tt-focus comes from widgets that don't call
// setFocus; tt-sb-account from accounts.js.
document.addEventListener('tt-focus', e => { const s = /** @type {CustomEvent} */ (e).detail; if (typeof s === 'string') publish('focus', s); });
document.addEventListener('tt-sb-account', e => { const n = /** @type {CustomEvent} */ (e).detail; if (typeof n === 'string' && n) publish('account', n); });

// The open workspace's theme, which its widgets follow: symbols (the
// watchlist, charts, news) and news keywords. A workspace without one
// leaves widgets as they are. Also the symbol picked last (in the
// watchlist), which charts follow: the focus channel (lib/channels.js).
import { publish, read } from './channels.js';

/** @typedef {{ symbols: string[], keywords: string[], layers?: string[] }} Theme */

/** The calendar's layers over its market sessions, and those shown without a theme. */
export const LAYERS = [['econ', 'Econ'], ['earnings', 'Earnings'], ['oil', 'Oil']];
export const DEFAULT_LAYERS = ['econ', 'earnings'];

/** @type {Theme | null} */
let current = null;

/** The open workspace's theme, if it has one. */
export const theme = () => current;

/** Set by the shell when a workspace opens or its theme is edited. @param {Theme | null} t */
export function setTheme(t) {
  current = t && (t.symbols.length || t.keywords.length) ? t : null;
  if (focused() && !current?.symbols.includes(focused())) publish('focus', null);
}

/** The symbol picked last, which charts follow. */
export const focused = () => read('focus');
/** @param {string | null} s */
export function setFocus(s) { publish('focus', s); }

/** "USO, xle  gld" as ["USO", "XLE", "GLD"]: letters, digits and dots, at most 50. @param {string} text */
export const parseSymbols = text => [...new Set(text.toUpperCase().split(/[\s,]+/).filter(s => /^[A-Z0-9.]{1,10}$/.test(s)))].slice(0, 50);
/** Comma-separated keywords, trimmed, at most 50. @param {string} text */
export const parseKeywords = text => [...new Set(text.split(',').map(s => s.trim()).filter(Boolean))].slice(0, 50);

/** Presets offered when editing a theme. @type {Record<string, Theme>} */
export const PRESETS = {
  'Oil & Iran': {
    // Crude, producers, tanker freight (BWET holds tanker freight futures) and
    // tanker owners, defense, gold.
    symbols: ['USO', 'BNO', 'XLE', 'XOP', 'OXY', 'XOM', 'CVX', 'BWET', 'FRO', 'DHT', 'STNG', 'INSW', 'ITA', 'LMT', 'NOC', 'RTX', 'GLD'],
    keywords: ['Iran', 'Israel', 'Hormuz', 'OPEC', 'crude', 'oil', 'tanker', 'Brent', 'Red Sea', 'Houthi', 'shipping', 'refinery', 'pipeline', 'LNG', 'sanctions'],
    layers: ['econ', 'oil'],
  },
};

/** Whether a story is on the theme: one of its symbols, or a keyword in its headline or summary.
 * @param {{ symbols?: string[], headline?: string, summary?: string }} n @param {Theme} t */
export function onTheme(n, t) {
  if ((n.symbols || []).some(s => t.symbols.includes(s))) return true;
  const text = `${n.headline || ''} ${n.summary || ''}`.toLowerCase();
  return t.keywords.some(k => new RegExp(`\\b${k.toLowerCase().replace(/[.*+?^${}()|[\]\\]/g, '\\$&')}\\b`).test(text));
}

// Watchlists: named lists of symbols, kept in this browser, with one picked
// at a time; the Watchlist and News widgets follow the pick (`tt-watchlist`).
// A first visit starts with a few to edit. The open view's theme, when it
// has symbols, is offered as a list of its own ("This view").

/** @typedef {{ name: string, symbols: string[] }} Watchlist */

const KEY = 'tinytrader-watchlists', PICK = 'tinytrader-watchlist';
/** The theme's own list, which isn't stored. */
export const THEME_LIST = '\u0000theme';

/** Lists to start from. @type {Watchlist[]} */
const STARTERS = [
  { name: 'Core', symbols: ['SPY', 'QQQ', 'IWM', 'DIA', 'TLT', 'GLD', 'USO', 'UUP'] },
  { name: 'Mega caps', symbols: ['NVDA', 'AAPL', 'MSFT', 'GOOGL', 'AMZN', 'META', 'AVGO', 'TSLA', 'BRK.B', 'JPM'] },
  { name: 'Nuclear', symbols: ['CCJ', 'CEG', 'VST', 'TLN', 'OKLO', 'SMR', 'NNE', 'LEU', 'BWXT', 'URA', 'URNM', 'NLR'] },
  { name: 'Oil', symbols: ['USO', 'BNO', 'XLE', 'XOP', 'XOM', 'CVX', 'COP', 'OXY', 'SLB', 'FRO', 'DHT', 'STNG'] },
  { name: 'Value', symbols: ['VTV', 'BRK.B', 'JPM', 'XOM', 'JNJ', 'PG', 'KO', 'CVX', 'PFE', 'VZ', 'BAC', 'WFC'] },
  { name: 'Semis', symbols: ['SMH', 'NVDA', 'AMD', 'AVGO', 'TSM', 'ASML', 'MU', 'INTC', 'QCOM', 'AMAT', 'LRCX', 'KLAC'] },
];

const read = () => { try { const v = JSON.parse(localStorage.getItem(KEY) || 'null'); return Array.isArray(v) ? v : null; } catch (_) { return null; } };
/** @type {Watchlist[]} */
let lists = read() || STARTERS.map(l => ({ ...l, symbols: [...l.symbols] }));
let pick = (() => { try { return localStorage.getItem(PICK); } catch (_) { return null; } })();

function save() {
  try { localStorage.setItem(KEY, JSON.stringify(lists)); if (pick != null) localStorage.setItem(PICK, pick); } catch (_) {}
  document.dispatchEvent(new CustomEvent('tt-watchlist'));
}
if (!read()) save();

/** Every stored list. */
export const watchlists = () => lists;
/** The picked list's name: a stored list, THEME_LIST, or the first. @param {boolean} themed whether the open view has a theme */
export function picked(themed) {
  if (pick === THEME_LIST && themed) return THEME_LIST;
  if (lists.some(l => l.name === pick)) return pick;
  return themed ? THEME_LIST : lists[0]?.name ?? null;
}
/** The symbols of a list by name (THEME_LIST: the theme's). @param {string | null} name @param {string[]} [themeSymbols] */
export function symbolsOf(name, themeSymbols = []) {
  if (name === THEME_LIST) return themeSymbols;
  return lists.find(l => l.name === name)?.symbols || [];
}
/** @param {string} name */
export function setPicked(name) { pick = name; save(); }

/** A new list, picked; its name made unique. @param {string} name @param {string[]} [symbols] */
export function addList(name, symbols = []) {
  let n = name.trim().slice(0, 40) || 'New list';
  for (let k = 2; lists.some(l => l.name === n); k++) n = `${name.trim().slice(0, 36)} ${k}`;
  lists.push({ name: n, symbols }); pick = n; save(); return n;
}
/** @param {string} from @param {string} to */
export function renameList(from, to) {
  const l = lists.find(x => x.name === from), n = to.trim().slice(0, 40);
  if (!l || !n || lists.some(x => x.name === n)) return false;
  l.name = n; if (pick === from) pick = n; save();
  document.dispatchEvent(new CustomEvent('tt-watchlist-renamed', { detail: { from, to: n } }));
  return true;
}
/** @param {string} name */
export function deleteList(name) {
  lists = lists.filter(l => l.name !== name);
  if (pick === name) pick = lists[0]?.name ?? null;
  save();
}
/** Add symbols to a list, keeping its order. @param {string} name @param {string[]} symbols */
export function addSymbols(name, symbols) {
  const l = lists.find(x => x.name === name); if (!l) return;
  for (const s of symbols) if (!l.symbols.includes(s)) l.symbols.push(s);
  save();
}
/** @param {string} name @param {string} symbol */
export function removeSymbol(name, symbol) {
  const l = lists.find(x => x.name === name); if (!l) return;
  l.symbols = l.symbols.filter(s => s !== symbol); save();
}
/** Back to the starter lists. */
export function resetLists() { lists = STARTERS.map(l => ({ ...l, symbols: [...l.symbols] })); pick = lists[0].name; save(); }

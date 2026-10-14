// Option structures, shared by the Spread builder and the Accounts order
// ticket: each a list of legs [type, strike offset, side, quantity], the
// offset in listed strikes from the anchor (the at-the-money strike, or one
// picked) and w the width in listed strikes; side +1 buys, -1 sells. Grouped
// for the menus, each with what it's for.

/** @typedef {[('call' | 'put'), number, (1 | -1), number]} Shape */
/** @type {{ group: string, items: Record<string, { what: string, make: (w: number) => Shape[] }> }[]} */
export const GROUPS = [
  { group: 'Verticals', items: {
    'Bull call spread (debit)': { what: 'Bullish: buy the call at the strike, sell one w higher. Pays to open; makes the width less the debit above the higher strike.', make: w => [['call', 0, 1, 1], ['call', w, -1, 1]] },
    'Bull put spread (credit)': { what: 'Bullish: sell the put at the strike, buy one w lower. Paid to open; keeps the credit above the strike.', make: w => [['put', 0, -1, 1], ['put', -w, 1, 1]] },
    'Bear put spread (debit)': { what: 'Bearish: buy the put at the strike, sell one w lower. Pays to open; makes the width less the debit below the lower strike.', make: w => [['put', 0, 1, 1], ['put', -w, -1, 1]] },
    'Bear call spread (credit)': { what: 'Bearish: sell the call at the strike, buy one w higher. Paid to open; keeps the credit below the strike.', make: w => [['call', 0, -1, 1], ['call', w, 1, 1]] },
  } },
  { group: 'Range (short volatility)', items: {
    'Iron condor': { what: 'Sell a put w below and a call w above, buy wings 2w out. Paid to open; keeps the credit between the short strikes.', make: w => [['call', w, -1, 1], ['put', -w, -1, 1], ['call', 2 * w, 1, 1], ['put', -2 * w, 1, 1]] },
    'Iron fly': { what: 'Sell the call and put at the strike, buy wings w out. Paid to open; most at the strike.', make: w => [['call', 0, -1, 1], ['put', 0, -1, 1], ['call', w, 1, 1], ['put', -w, 1, 1]] },
    'Call butterfly': { what: 'Buy a call w below, sell two at the strike, buy one w above. Pays a little to open; most at the strike.', make: w => [['call', -w, 1, 1], ['call', 0, -1, 2], ['call', w, 1, 1]] },
    'Put butterfly': { what: 'Buy a put w above, sell two at the strike, buy one w below. Pays a little to open; most at the strike.', make: w => [['put', w, 1, 1], ['put', 0, -1, 2], ['put', -w, 1, 1]] },
    'Short strangle': { what: 'Sell a call w above and a put w below. Paid to open; loss unlimited beyond them.', make: w => [['call', w, -1, 1], ['put', -w, -1, 1]] },
    'Short straddle': { what: 'Sell the call and put at the strike. Paid to open; loss unlimited either way.', make: () => [['call', 0, -1, 1], ['put', 0, -1, 1]] },
  } },
  { group: 'Breakout (long volatility)', items: {
    'Long straddle': { what: 'Buy the call and put at the strike. Pays to open; makes money on a move bigger than the debit either way.', make: () => [['call', 0, 1, 1], ['put', 0, 1, 1]] },
    'Long strangle': { what: 'Buy a call w above and a put w below. Pays less than a straddle; needs a bigger move.', make: w => [['call', w, 1, 1], ['put', -w, 1, 1]] },
    'Reverse iron condor': { what: 'Buy a put w below and a call w above, sell wings 2w out. Pays to open; makes money on a move past either long strike, capped.', make: w => [['call', w, 1, 1], ['put', -w, 1, 1], ['call', 2 * w, -1, 1], ['put', -2 * w, -1, 1]] },
  } },
  { group: 'Single', items: {
    'Long call': { what: 'Buy the call at the strike.', make: () => [['call', 0, 1, 1]] },
    'Long put': { what: 'Buy the put at the strike.', make: () => [['put', 0, 1, 1]] },
  } },
];
/** Every structure by name: its legs for a width. @type {Record<string, (w: number) => Shape[]>} */
export const STRUCTURES = Object.fromEntries(GROUPS.flatMap(g => Object.entries(g.items).map(([n, s]) => [n, s.make])));
// What the shapes that aren't presets are, matched by the start of their name.
const MORE = [
  ['Short call', 'Sell a call: paid to open; loses without limit above the strike plus the credit.'],
  ['Short put', 'Sell a put: paid to open; loses below the strike less the credit, down to zero.'],
  ['Call ratio spread', 'Buy calls and sell more calls at a higher strike: little or no debit; best at the short strike, loses without limit far above it.'],
  ['Put ratio spread', 'Buy puts and sell more puts at a lower strike: best at the short strike, loses far below it.'],
  ['Call backspread', 'Sell calls and buy more calls at a higher strike: profits from a big move up, may lose a little in between.'],
  ['Put backspread', 'Sell puts and buy more puts at a lower strike: profits from a big move down.'],
  ['Broken-wing', 'A butterfly with one wing wider: cheaper (or a credit) in exchange for risk on the wide side.'],
  ['Short call butterfly', 'Sell the wings, buy the body: paid a little to open; profits from a move away from the middle strike.'],
  ['Short put butterfly', 'Sell the wings, buy the body: paid a little to open; profits from a move away from the middle strike.'],
  ['Call condor', 'Buy the outer calls, sell the inner two: pays a little; most anywhere between the inner strikes.'],
  ['Put condor', 'Buy the outer puts, sell the inner two: pays a little; most anywhere between the inner strikes.'],
  ['Short call condor', 'The condor reversed: profits from a move beyond the outer strikes.'],
  ['Short put condor', 'The condor reversed: profits from a move beyond the outer strikes.'],
  ['Reverse iron fly', 'Buy the call and put at the strike, sell wings outside: pays to open; profits from a move past the debit either way, capped.'],
  ['Synthetic long', 'Buy a call and sell a put at one strike: moves like 100 shares of stock.'],
  ['Synthetic short', 'Sell a call and buy a put at one strike: moves like 100 shares short.'],
  ['Risk reversal (bullish)', 'Sell a put below, buy a call above: little or no cost; gains above the call, loses below the put.'],
  ['Risk reversal (bearish)', 'Sell a call above, buy a put below: gains below the put, loses above the call.'],
  ['Long guts', 'Buy an in-the-money call and put: like a strangle with strikes crossed; costs more.'],
  ['Short guts', 'Sell an in-the-money call and put: paid to open; most between the strikes.'],
];
/** What a structure is for. @param {string} name */
export const whatIs = name => GROUPS.map(g => g.items[name]).find(Boolean)?.what || MORE.find(([k]) => name.startsWith(k))?.[1] || '';
/** The menu's options, grouped. @param {string} picked @param {(s: string) => string} esc */
export const structureOptions = (picked, esc) => GROUPS.map(g => `<optgroup label="${esc(g.group)}">${Object.keys(g.items).map(n => `<option${n === picked ? ' selected' : ''}>${esc(n)}</option>`).join('')}</optgroup>`).join('');

/**
 * What a set of legs is, by shape rather than by how it was built: a name
 * from the list above where one fits (a vertical, straddle, strangle,
 * butterfly, condor, iron condor or fly), else a descriptive one (ratio
 * spread, broken-wing butterfly), else null. Strikes may sit anywhere; the
 * whole position may be any multiple of its unit. One expiry is assumed.
 * @param {{ type: 'call' | 'put', strike: number, side: number, qty: number }[]} legs
 * @returns {string | null}
 */
export function identify(legs) {
  const ls = legs.filter(l => l.qty > 0);
  if (!ls.length) return null;
  const gcd = (a, b) => b ? gcd(b, a % b) : a;
  const unit = ls.reduce((g, l) => gcd(g, l.qty), 0) || 1;
  const L = ls.map(l => ({ ...l, n: l.qty / unit })).sort((a, b) => a.strike - b.strike || (a.type === 'put' ? -1 : 1));
  const calls = L.filter(l => l.type === 'call'), puts = L.filter(l => l.type === 'put');
  const long = l => l.side > 0, all1 = xs => xs.every(l => l.n === 1);
  // Two legs of one type, opposite sides: a vertical (equal sizes) or a ratio spread.
  const vertical = (xs, type) => {
    const [lo, hi] = xs;
    if (xs.length !== 2 || long(lo) === long(hi)) return null;
    if (lo.n !== hi.n) {
      const more = long(lo.n > hi.n ? lo : hi);
      return `${type === 'call' ? 'Call' : 'Put'} ${more ? 'backspread' : 'ratio spread'} (${lo.n}:${hi.n})`;
    }
    if (lo.strike === hi.strike) return null;
    if (type === 'call') return long(lo) ? 'Bull call spread (debit)' : 'Bear call spread (credit)';
    return long(hi) ? 'Bear put spread (debit)' : 'Bull put spread (credit)';
  };
  if (L.length === 1) return `${long(L[0]) ? 'Long' : 'Short'} ${L[0].type}`;
  if (L.length === 2) {
    if (calls.length === 2) return vertical(calls, 'call');
    if (puts.length === 2) return vertical(puts, 'put');
    const [c, p] = [calls[0], puts[0]];
    if (c.n !== p.n) return null;
    // Opposite sides: long stock's shape (bullish) or short's.
    if (long(c) !== long(p)) {
      const bull = long(c);
      return c.strike === p.strike ? `Synthetic ${bull ? 'long' : 'short'} stock` : `Risk reversal (${bull ? 'bullish' : 'bearish'})`;
    }
    const side = long(c) ? 'Long' : 'Short';
    if (c.strike === p.strike) return `${side} straddle`;
    return p.strike < c.strike ? `${side} strangle` : `${side} guts`;
  }
  // Three legs of one type, wings around a doubled body: a butterfly.
  if (L.length === 3 && (calls.length === 3 || puts.length === 3)) {
    const [a, b, c] = L, kind = a.type === 'call' ? 'Call' : 'Put';
    if (a.n === 1 && b.n === 2 && c.n === 1 && long(a) === long(c) && long(a) !== long(b)) {
      const broken = b.strike - a.strike !== c.strike - b.strike;
      return `${long(a) ? '' : 'Short '}${broken ? 'broken-wing ' : ''}${kind.toLowerCase()} butterfly`.replace(/^./, s => s.toUpperCase());
    }
    return null;
  }
  if (L.length === 4 && all1(L)) {
    // Four of one type, outer pair against inner pair: a condor.
    if (calls.length === 4 || puts.length === 4) {
      const [a, b, c, d] = L;
      if (long(a) === long(d) && long(b) === long(c) && long(a) !== long(b)) return `${long(a) ? '' : 'Short '}${a.type} condor`.replace(/^./, s => s.toUpperCase());
      return null;
    }
    // Two puts below, two calls above: iron condor or fly (short inside: collects).
    if (calls.length === 2 && puts.length === 2) {
      const [p1, p2] = puts, [c1, c2] = calls;
      if (p2.strike > c1.strike) return null;
      const shortInside = !long(p2) && !long(c1) && long(p1) && long(c2);
      const longInside = long(p2) && long(c1) && !long(p1) && !long(c2);
      if (!shortInside && !longInside) return null;
      const fly = p2.strike === c1.strike;
      if (shortInside) return fly ? 'Iron fly' : 'Iron condor';
      return fly ? 'Reverse iron fly' : 'Reverse iron condor';
    }
  }
  return null;
}

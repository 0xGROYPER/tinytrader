// Liquidate's re-read (web/lib/liquidate.js), in Node with a stand-in fetch:
// `node --test services/console/tests/` (also checks.console-js in Nix).
import { test } from 'node:test';
import assert from 'node:assert/strict';

// Just enough of a browser for the page's modules to load.
Object.assign(globalThis, { window: globalThis, requestAnimationFrame: () => 0, addEventListener() {}, document: { addEventListener() {}, querySelector: () => null } });
const { reread } = await import('../web/lib/liquidate.js');

/** Answer every request with `answer(path, init)`, recording what was asked. */
function serve(answer) {
  const asked = [];
  globalThis.fetch = async (path, init = {}) => {
    asked.push(`${init.method || 'GET'} ${path}`);
    const [status, body] = answer(path, init);
    if (status === 0) throw new Error('connection refused');
    return new Response(JSON.stringify(body), { status, headers: { 'content-type': 'application/json' } });
  };
  return asked;
}

const account = extra => ({ name: 'paper-1', updated_at: 1000, age_ms: 120, positions: [{ symbol: 'SPY261016P00560000', qty: -2 }], orders: [], ...extra });

test('with no working orders, a fresh read from the broker is what the run closes on', async () => {
  const asked = serve(() => [200, account()]);
  const r = await reread('paper-1');
  assert.deepEqual(asked, ['POST /api/accounts/paper-1/read'], 'read from the broker, never /api/state');
  assert.equal(r.why, undefined);
  assert.equal(r.account.positions[0].qty, -2);
  // Read after the run's cancels: fine.
  assert.ok((await reread('paper-1', 1000)).account);
});

test('a reading that is still stale, or older than the cancels, is refused', async () => {
  serve(() => [200, account({ age_ms: 7000 })]);
  assert.deepEqual(await reread('paper-1'), { account: null, why: 'The positions are stale: last read 7 s ago (limit 5 s)' });
  serve(() => [200, account({ age_ms: null })]);
  assert.match((await reread('paper-1')).why, /reading time is unknown/);
  serve(() => [200, account()]);
  assert.match((await reread('paper-1', 1001)).why, /read before the run cancelled/);
  serve(() => [200, account({ orders: null })]);
  assert.equal((await reread('paper-1')).why, 'The positions could not be re-read');
});

test('a failed read from the broker refuses, saying why', async () => {
  serve(() => [502, { error: '/v2/positions: Alpaca answered 500 Internal Server Error' }]);
  assert.deepEqual(await reread('paper-1'), { account: null, why: 'The positions could not be re-read from Alpaca: /v2/positions: Alpaca answered 500 Internal Server Error' });
  serve(() => [0]);
  assert.equal((await reread('paper-1')).why, 'The positions could not be re-read from Alpaca: connection refused');
  serve(() => [404, { error: 'no accounts service serves this account' }]);
  assert.match((await reread('paper-1')).why, /no accounts service/);
});

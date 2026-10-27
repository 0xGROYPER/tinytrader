# Console handover: October 2026

Written when the sessions on the old Mac were closed. Nothing is in progress.

## Merged

- **#9** Console state channels (step 2). `lib/channels.js` with topology, focus and account. Merge commit `7dbb380`; branch commits `9dd7a27` and the review fixes.
- **#22** Focus goes through the focus channel only (step 3, part 1). Merged 17 Oct.
- **#24** Service lookups go through `serviceFor()` and `servicesFor()` (step 3, part 2). Merged 17 Oct.
- **#26** The trading journal. Merge commit `de7ab58`, branch commit `721d9e3` (rebased onto #19). Merged 18 Oct.

## Not merged, and not started

- **`tt-sb-account` bridge** in `lib/channels.js`. Stays until `widgets/accounts.js` publishes the `account` channel itself.
- **Lookups in `widgets/accounts.js` and `lib/liquidate.js`** still use `fleet.topo` directly. They move with the account work.
- **Blocked on `integrate/review-fixes`** (commit `9d96086`). Not merged. Its worktree is another agent's. Both items above wait on it.

## Parked, in the order agreed

1. Request channels. Held until one subscriber takes them. For `tt-recommend` and `tt-held`, which open the Spread builder and hand it work.
2. Remaining state channels: `theme`, `watchlists`, `alerts`, `position`, `board`, and `snapshot` (via `setSnapshot`, as topology was done).
3. Delete the dead listener `tt-search-show` in `widgets/search.js`.
4. Format helpers (`pct`, `money`, `tone`, `shortDate`). About 30 local copies. Each call site needs an output check, and any visible difference is listed for approval.
5. The terminal and notebook factory (`widgets/terminal.js`, `widgets/notebook.js`).
6. `occ.js`: five OCC regexes, `occLabel`, `shortSym`. Waits on `integrate/review-fixes`.
7. Data functions. Daily closes come four ways. Check whether alpaca-data's `/daily` and `/bars?interval=86400` agree before moving them. Not yet checked.
8. The day-data module (Calendar, Day, Earningsweek). Low value.

## Open questions and risks

- **The console has no login.** Settings, reminders and the journal take writes from anyone who can reach it. The journal matches settings for now. When the write gate resumes, the journal goes under the same login check.
- **The journal is not live on the mini** until the console is deployed there. Until then `/api/journal` returns 404 and the panel says it couldn't read the journal.
- **Journal file location.** Defaults to `journal.json` beside the reminders file. Confirm that the mini's compose file matches. `CONSOLE_JOURNAL` overrides it.
- **Upstream rate limits.** alpaca-data returns 429 when chain and snapshot calls come in bursts. This made browser comparisons noisy.
- **Accepted behaviour changes.** Re-picking the already-focused symbol no longer reloads the IV map's chain or refreshes Candles. A ticket's account pick now reaches other Spread builders.
- **Unverified.** The Liquidate picker (#8) was not checked on the live page. One Earnings workspace run logged a console error that wasn't captured; it did not recur in about ten later runs.
- **Deploys.** `scripts/deploy console` refuses on the old Mac, because the console is placed on the mini. Deploys belong to the migration session.

## Test harness and screenshots

The Playwright scripts were on a scratch area that was cleared before the
move, so they're gone from disk. Their surviving copies: `test.mjs` and
`sweep.mjs` are in the description of PR #9; the journal test's steps are in
the description of PR #26, and the script itself must be rewritten. A README
on branch `handover/console-harness` in the private ops repo says the same.
Journal screenshots are in `docs/screenshots/journal/`; the full day view
(with live P/L) was not kept.

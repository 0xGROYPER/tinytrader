# Observations

What changed character, in plain sentences, ranked by how unusual each is: across the [Cross-asset](crossasset.md) board's symbols and every [watchlist](watchlist.md)'s.

**Data:** the same daily closes as Cross-asset (alpaca-data's `daily` stream), read when shown and again when shown after ten minutes.

**The rules:**

- **Trend:** a close across the 20-, 50- or 200-day average, and when it was last on that side ("for the first time since Aug 14"). The longer the average, the higher it ranks.
- **Range:** a 52-week high or low, else a 20-day one.
- **Streak:** four or more up or down sessions running, and the change over them.
- **Move:** a day's move of two standard deviations or more against its last 20 days.
- **Volatility:** the VIX term structure turning inverted (the 9-day above the 30-day, or the 30-day above the 3-month), or back; and still inverted.
- **Rates:** the yield curve (the 10-year less the 13-week) changing sign over the week.

**Using it:** the chips filter by kind, with counts. Each line names its symbol; hover it to see where it comes from (the board or a watchlist), and click a stock or ETF to point the view's charts at it.

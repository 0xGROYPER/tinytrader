# Options screener

Stocks ranked by what their options expect: implied volatility, the expected move in percent and in dollars of market value, implied against realized volatility, with the day's movers and the most traded beside them.

**Data:** a service serving the screener (alpaca-data). The universe is Nasdaq's stock screener: every US listing with its price, the day's change and volume, sector and market capitalisation, refreshed every 10 minutes. Of the listings at or above the size picked and over $5, the 150 largest, the 25 biggest movers, the 25 most traded by dollar volume, and those on Alpaca's most-active list are scanned in the background:

- **IV 30d:** at-the-money implied volatility of the expiry nearest 30 days (computed from Alpaca's indicative quotes: the out-of-the-money side's mid, Black-76 on the parity forward).
- **Exp. move:** the at-the-money straddle's mid over the price, for the first expiry at least three sessions away (a straddle of a day or two is mostly noise, or one event). Ranked scaled to a week (five sessions), so names with only monthly expiries compare with weeklies. **E** marks an earnings report before that expiry: the move includes it.
- **Exp. $ move:** the weekly expected move times market capitalisation: where the most value is expected to move.
- **Skew:** the 25-delta risk reversal at the expiry nearest 30 days, from the chain's fitted smile: put less call implied volatility, in points. Positive: downside priced richer (usual for indexes); negative: upside richer. Blank when the chain is too thin to fit ([the method](../implied-distribution.md)).
- **RV 20d:** realized volatility of the last 20 daily returns (annualised); **IV / RV** over it: above 1.5 (amber) options are priced well above recent moves, below 0.8 (grey) below them.
- **Rel vol:** the day's volume over the 20-session average; 2× and over in amber.

Each stock is rescanned every 15 minutes while the screener is open; a first scan takes a few minutes, and rows fill in as it goes. The scan uses at most a third of Alpaca's request limit. Float isn't shown: no free source publishes it.

**Using it:**

- Pick a view along the top (Highest IV, Expected move, Expected $ move, Put skew, IV over realized, Top movers, Unusual volume, Most active), a minimum size, and a sector.
- Click a column heading to sort by it (again to reverse); picking a view returns to its ranking.
- Click a row to point the workspace's charts and IV heatmap at that stock and get a suggested position for it in the [Spread builder](spreads.md), opened if it isn't in the workspace. Hover a row for why it's listed.

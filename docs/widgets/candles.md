# Candles

Candlesticks and volume for a symbol at the bar size you pick (1m, 5m, 15m, 1h, 1D, 1W), with as much history as you scroll back to, and indicators listed over the chart as on TradingView. Times are New York time.

**Data:** A relay's bars where a relay carries the symbol and no service has bars for any symbol (minutes from its session, combined into larger bars); otherwise alpaca-data's: consolidated (SIP) history, with only the last 15 minutes from IEX. Bars are kept in the page: switching symbol or bar size and coming back costs a refresh of the latest bars, not the history.

**Using it:**

- Pick a bar size. The chart opens on the latest bars; drag it right, scroll back, or zoom out, and older bars load a chunk at a time until the source has no more ("start of history" in the legend).
- In a themed workspace it shows the theme's symbols; it follows the watchlist's selection.
- **Ticker box:** type any symbol and press Enter to chart it (needs a service with bars for any symbol, alpaca-data). Clicking a ticker in another widget (Watchlist, Options screener, Earnings week) charts it too.
- **Extended hours:** intraday bars cover 04:00-20:00 New York; pre-market and after-hours candles are drawn faded.

**Indicators (top left):** the symbol, bar size and last bar, then one row per indicator with its values: ◉ shows or hides it, ⚙ opens its settings, × removes it, and **+ Expected move** adds one. Settings are shared by every Candles panel and kept between visits.

- **Expected move:** how far the options market expects the price to move by a horizon, drawn as dashed lines at the price plus and minus it, whatever the chart's zoom. Its row shows the move and the range. Horizons:
  - **Trading days:** N trading days ahead. Each expiry's at-the-money straddle gives the move to that date; the horizon's is interpolated between the expiries either side (as total variance), so an earnings report inside it is included. Past the longest listed expiry it is extrapolated, and says so.
  - **To an expiry:** the straddle to a listed expiry you pick.
  - **After earnings:** to the first expiry after the company's next report.
  - **0DTE:** the straddle expiring today, the move expected to today's close. SPY, QQQ and SPX expire every trading day; most stocks only on Fridays.
- **Band** (in its settings):
  - **Straddle** (the default): ± the at-the-money straddle, the average size of the move (about 0.8 standard deviations). For an expiry, the row adds the chance of finishing inside it by the implied distribution (on a bell curve 57%; real chains, peaked with fat tails, usually 58-65%).
  - **68% range** and **95% range:** the range the options market gives those odds of finishing inside, from the expiry's implied distribution fitted to the whole chain (see [the method](../implied-distribution.md)). Lopsided where the smile is skewed: the row shows the change down and up. For N trading days, the distribution of the expiry after the horizon, scaled to it ("shape of …"). A chain too thin to fit falls back to the straddle and says so; a rough fit says so.
- Each has its own colour (pick one in its settings). The defaults: 5 trading days, and after earnings.

**A position on the chart:** with the [Spread builder](spreads.md)'s **Show on chart**, the position is drawn on its underlying: each strike as a line labelled with the profit or loss there at expiry (sold solid, bought dashed), the breakevens dotted, the prices where it profits at expiry shaded green and where it loses red, and its payoff sideways at the right edge on the same prices. The pointer's price shows its profit or loss at the bottom left. Legs are changed in the builder. Indexes have no volume, so no volume bars are drawn for them.

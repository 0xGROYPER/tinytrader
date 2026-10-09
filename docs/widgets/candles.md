# Candles

Candlesticks and volume for a symbol, with TradingView-style ranges (1D, 5D, 1M, 6M, 1Y, 5Y) and bar sizes (1m, 5m, 15m, 1h, 1D). Times are New York time.

**Data:** A relay's bars where a relay carries the symbol (it builds them from the trades it relays and fills earlier minutes from its source's history); otherwise bars for any symbol from alpaca-data: consolidated (SIP) history, with only the last 15 minutes from IEX.

**Using it:**

- Pick a range; the bar size follows it and can be changed.
- In a themed workspace it shows the theme's symbols; it follows the watchlist's selection.
- **IM** (on by default) draws the options market's implied move as dashed lines at the price plus and minus it, with the figures beside the last price:
  - **for the chart's range** (accent, thicker): the move expected over the next day, 5 days, month, 6 months, year or 5 years, matching the range picked. Each expiry's at-the-money straddle gives the move to that date; the horizon's is interpolated between the expiries either side (as total variance), so an earnings report inside it is included. Past the longest listed expiry it is extrapolated, and says so.
  - **0DTE** (green): the straddle expiring today, the move expected from now to today's close. SPY, QQQ and SPX expire every trading day; most stocks only on Fridays.
  - **earnings** (amber): to the first expiry after the company's next report.
- **Pos** (on by default) draws the strikes of the open option positions on the charted underlying in one account (`POS_ACCOUNT` in the widget, Paper 1 for now; SPX also shows SPXW): short strikes as solid lines, long strikes dotted, each labelled with the account, side, strike, put or call, quantity and expiry. Legs of the same strike and expiry are netted. They redraw when the positions change.

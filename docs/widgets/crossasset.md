# Cross-asset

A board of what moved beyond US stocks, grouped: rates, the dollar and FX, commodities, volatility, world markets, crypto and US indexes. Each row: the level, the change over a day, a week and a month, whether it's above (▲) or below (▼) its 20-, 50- and 200-day averages, and where it sits in its 52-week range.

**Data:** daily closes from alpaca-data's `daily` stream: stocks and ETFs from Alpaca (the full market's, up to 16 minutes late), crypto pairs from Alpaca's crypto data, and Cboe's indexes from Cboe (15 minutes delayed): VIX and its 9-day and 3-month terms, and the Treasury yields IRX (13-week), FVX (5-year), TNX (10-year) and TYX (30-year). Yields are shown in percent and move in basis points. Read when the board is shown, and again when it's shown after ten minutes.

**Using it:**

- The starting board: yields, TLT, HYG and LQD; UUP, FXE and FXY; USO, UNG, GLD, SLV and CPER; VIX9D, VIX and VIX3M; EWJ, FXI, EWG, EWU, EEM and INDA; BTC/USD, ETH/USD and SOL/USD; SPY, QQQ, IWM and RSP. Most are ETFs standing in for what they track.
- **Edit rows** changes each group's symbols (kept in this browser); **Back to the starting board** undoes it.
- Under the volatility rows, the VIX term structure's shape: upward sloping (calm) or inverted (the near term priced above the far: stress).
- A row is tagged when it's at a 20-day or 52-week high or low. Hover the arrows for each average's level and how long it's been on that side; hover the range for the 52-week low and high.
- Click a stock or ETF row to point the view's charts at it.
- Weeks and months are 5 and 21 sessions, or 7 and 30 days for crypto, which trades every day.

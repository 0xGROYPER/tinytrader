# Sector heatmap

The large companies the options screener scans, as a treemap after Finviz's map: a block per sector, sized by its companies' market value and headed by its value-weighted change, and a tile per company, sized by its market value and coloured by its change over the period picked from red through neutral to green.

**Data:** the rows of the options screener (alpaca-data): each company's sector, market value and its change over the day, a week and a month. They're read when the map is shown, and again when it's shown after ten minutes. It covers the screener's universe (about 190 of the largest US-listed companies), not every stock.

**Using it:**

- **1D** (the default), **1W** or **1M**: the day's change, or the change against the close 5 or 21 sessions back. Full colour at ±3%, ±6% and ±10%; the choice is remembered.

- Hover a tile for the company, its change, sector, market value and price; the line above the map shows it.
- Click a tile to point the workspace's charts (Candles, IV heatmap) at it. The workspace theme's symbols are outlined.
- It fills its panel; small tiles drop the change, then the symbol.

If the screener can't be read (its service down, or the console just started), the map tries again every 15 seconds. While the screener is still reading companies after a restart, the header says how many are left and the map refreshes every minute until it's complete.

# IV heatmap

An underlying's implied volatility by strike (rows) and expiry (columns), on the usual heatmap scale from blue (low) through green and yellow to red (high). The first rows are the term structure: at-the-money IV and the straddle's implied move per expiry.

**Data:** Option chains from alpaca-data, with IV and greeks from Alpaca's snapshots; refreshed every 30 seconds. Each cell is the out-of-the-money contract's IV (puts below spot, calls above).

**Using it:**

- The underlying is the one picked in a watchlist, else the theme's first symbol, else one typed in.

# Spread builder

An options position from a structure and the live chain, priced, and drawn on Candles to adjust by dragging. It builds and prices; it sends nothing.

**Data:** a service serving option chains (alpaca-data): quotes from Alpaca's indicative feed, with implied volatility computed by alpaca-data from each mid (Alpaca's own values are not used). Refreshed every 30 seconds.

**Using it:**

- Type an underlying in the ticker box, or click a ticker in another widget (the Options screener, Earnings week). Pick the expiry; each shows the move priced into it.
- Pick a structure (iron condor, iron fly, strangle, straddle, call or put spread, single call or put) and a **width** in listed strikes: legs are placed from the at-the-money strike outward.
- Adjust a leg: buy or sell, step its strike with ‹ ›, switch call or put, remove it, or add one. The structure becomes Custom.
- One line sums it up: the price at the mid and crossing the spread, maximum profit and loss, breakevens, and the chance of profit at expiry, per position of 100 shares a contract. The chance comes from the expiry's implied distribution, fitted to the whole chain with its skew and fat tails ([the method](../implied-distribution.md)), when the fit is good or rough; for a chain too thin to fit, from a lognormal at the at-the-money volatility. It says which.
- **Show on chart** draws the position on [Candles](candles.md) and points it at the underlying. Drag a strike line there to move that leg; the builder follows.

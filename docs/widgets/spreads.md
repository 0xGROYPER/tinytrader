# Spread builder

An options position from a structure and the live chain: its payoff at expiry and today, its price, maximum profit and loss, breakevens, chance of profit and net greeks. It builds and prices; it sends nothing.

**Data:** a service serving option chains (alpaca-data): quotes from Alpaca's indicative feed, with implied volatility and greeks. Refreshed every 30 seconds.

**Using it:**

- Type an underlying in the ticker box, or click a ticker in another widget. Pick the expiry.
- Pick a structure (single legs, verticals, short straddle and strangle, iron fly and condor, risk reversal, put ratio 1×2) and a **width** in listed strikes: legs are placed from the at-the-money strike outward.
- Edit any leg (buy or sell, quantity, call or put, strike), add or remove legs; the structure becomes Custom.
- The summary shows the price at the mid and crossing the spread, maximum profit and loss (or unlimited), breakevens, the chance of profit at expiry (lognormal at the at-the-money implied volatility) and net delta, gamma, theta per day and vega, per position (100 shares a contract).
- The chart: profit at expiry (solid; green above zero, red below), today's value (dashed), the current price, breakevens, and the expected move to the expiry (shaded).

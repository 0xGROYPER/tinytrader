# Candles

Candlesticks and volume for a symbol, with TradingView-style ranges (1D, 5D, 1M, 6M, 1Y, 5Y) and bar sizes (1m, 5m, 15m, 1h, 1D). Times are New York time.

**Data:** A relay's bars where a relay carries the symbol (it builds them from the trades it relays and fills earlier minutes from its source's history); otherwise bars for any symbol from alpaca-data: consolidated (SIP) history, with only the last 15 minutes from IEX.

**Using it:**

- Pick a range; the bar size follows it and can be changed.
- In a themed workspace it shows the theme's symbols; it follows the watchlist's selection.

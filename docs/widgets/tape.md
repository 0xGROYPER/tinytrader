# Tape

Time and sales from the IEX relay, a tick at a time: every trade with its price and size, newest at the top, marked by where it printed against the quote before it.

**Data:** the IEX relay's `/tape` ([relay API](../relay-api.md#get-tape)): the last 4,000 trades and 4,000 quote changes of each of its symbols (SPY and QQQ), held in memory, read twice a second while the panel is in sight. IEX is a few percent of US volume, so it's a sample of the flow, not all of it, and it's empty after the relay restarts until the feed runs again.

**Using it:**

- One line on top: a ticker box (type the ticker and press Enter; it starts on the one the view's charts follow when the relay carries it, and says so when the relay doesn't carry what's typed), the last price, which flashes green or red as it ticks up or down, the quote, the window's share bought at the ask against sold at the bid (a bar and a percent), its VWAP and trades a minute. On the right: the smallest size shown, **Q** for quote changes, and pause.
- The list is newest first, one tick a line: time (milliseconds smaller), price and size. A trade at or above the ask before it is green with a green edge (a buy); at or below the bid, red (a sell); between, plain. Prints of 1,000 shares or more are bold and shaded. Each tick flashes in its colour as it arrives (the newest 40 at most, so a burst of quotes doesn't light up the list); with reduced motion set, it doesn't.
- The quote is IEX's own book, often wider than the national best bid and offer, so many trades land between it.
- The list holds still while the pointer is over it, or paused; ticks keep arriving and show when it resumes.

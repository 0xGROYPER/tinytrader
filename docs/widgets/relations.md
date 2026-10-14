# Relationships

Line charts after matplotlib's look (plain axes with ticks, a light grid, the tab10 colours, a legend and labels at the lines' ends), from daily closes.

**Data:** alpaca-data's `daily` stream (stocks and ETFs, crypto pairs and Cboe's indexes), read when shown and again when shown after ten minutes.

**Using it:**

- **Correlation:** the rolling correlation of daily returns for pairs, over the last **20**, **60** or **120** sessions, from −1 (moving opposite) through 0 to +1 (moving together). It starts with stocks and bonds (SPY TLT), stocks and the dollar (SPY UUP), stocks and gold (SPY GLD), oil and energy stocks (USO XLE), and the Nasdaq and bitcoin (QQQ BTC/USD): the relationships the market leans on, and when they hold or break. A yield's daily change is in points rather than a return.
- **Performance:** each symbol indexed to 100 at the start of **3 months**, **6 months** or **1 year**.
- **Edit** changes the pairs (`SPY TLT, USO XLE`) or the symbols (`SPY, TLT, GLD`), kept in this browser.
- Hover the chart for each line's value on that day, in the legend.

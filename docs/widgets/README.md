# Widgets

A workspace is a set of panels, each showing one widget. A widget can be added more than once (several charts); the palette's **Copy this panel to…** and **Move this panel to…** put the active panel, with its settings, into another view. These pages say what each widget shows, where its data comes from, and how to use it. Widgets that need a service show a note when none is running.

## Markets

- [Watchlist](watchlist.md): Named lists of symbols (nuclear, oil, value…) with price and the day's change; News follows the list picked.
- [Candles](candles.md): Candlesticks and volume for a symbol, with TradingView-style ranges (1D, 5D, 1M, 6M, 1Y, 5Y) and bar sizes (1m, 5m, 15m, 1h, 1D).
- [IV heatmap](ivmap.md): An underlying's implied volatility by strike (rows) and expiry (columns), on the usual heatmap scale from blue (low) through green and yellow to red (high).
- [Correlation](correlation.md): How the theme's symbols move together: Pearson correlation of daily log returns over the last 20, 60, 120 or 250 trading days, as a matrix from blue (−1) through white (0) to red (+1), with each symbol's annualised realised volatility.
- [Earnings](earnings.md): Upcoming reports with what options expect against what has happened before, to judge whether the expected move is rich or cheap: the jump implied by the term structure, the straddle, and the company's past moves, EPS and surprises.
- [Earnings week](earningsweek.md): The week's earnings reports as a grid, before the open and after the close, largest companies first.
- [Spread builder](spreads.md): An options position from a structure and the live chain, priced, with Show on chart to draw it on Candles and drag its strikes.
- [Options screener](screener.md): Stocks ranked by implied volatility, expected move (in percent and dollars of market value), IV over realized, the day's move and volume.
- [Day](day.md): One market day hour by hour (04:00 to 20:00 New York time, trimmed to what happens): releases at their times and earnings before the open and after the close; opened from the Calendar.
- [Cross-asset](crossasset.md): Rates, the dollar and FX, commodities, volatility, world markets, crypto and US indexes: each with its change over a day, week and month, its side of the 20-, 50- and 200-day averages and its 52-week range.
- [Observations](observations.md): Trend breaks, new highs and lows, streaks and unusual moves across the Cross-asset board and the watchlists, in plain sentences, ranked.
- [Relationships](relations.md): Matplotlib-style line charts: the rolling correlation of pairs (stocks and bonds, oil and energy…), or performance indexed to 100.
- [Sector heatmap](heatmap.md): The screener's large companies as a treemap by sector, sized by market value and coloured by the change over a day, a week or a month.
- [Calendar](calendar.md): The market month's weekdays: trading days, early closes and holidays, with scheduled economic releases, FOMC decisions and earnings as layers.
- [Alerts](alerts.md): Price alerts per symbol (a price to rise or fall to, or a move on the day), checked while a console page is open; fired ones go under the bell and to a notification.
- [Tape](tape.md): Time and sales from the IEX relay (SPY, QQQ): each trade's price and size, marked a buy at the ask or a sell at the bid, with the window's buy and sell volume and VWAP.
- [News](news.md): Recent stories, newest first, filtered by symbol with the chips along the top.
- [Prices](prices.md): One card per series the deployment's topology declares (the relays' trade prices), with the latest value, its change and a chart.

## Accounts and data

- [Accounts](accounts.md): A card per brokerage account, live first: equity, the day's change, balances, an equity chart and the largest positions.
- [Data](data.md): What the store holds, as a tree (dataset, symbol, schema) merged across drives, with each level's size and which drives hold it.
- [Storage](storage.md): The store's drives as one pool: free space first, then each drive against the store's thresholds, then any new drive that could be approved.
- [Jobs](jobs.md): Databento batch jobs: a request form that prices a request before it can be submitted, and every job through to filed in the store.

## System

- [System graph](graph.md): What runs where and how data flows.
- [Services](services.md): Every service with its container state, health, uptime, restarts and resources.
- [Schedule](schedule.md): One line per bundle of services sharing hours, then one per other service: its state and a strip of today in New York time (active hours, pauses, always running).
- [Queue](queue.md): Work that runs later, or when the machine can spare it, from the deployment's job templates.
- [Shell](shell.md): A terminal for read-only commands over this deployment's state (`help` lists them).

## Workspace tools

- [Search](search.md): A ticker search whose results (price, options, earnings, news) take the panel's place.
- [Docs](docs.md): The documentation's home: the tinytrader mark, a search over every document's title and path, and where to start.
- [Reminders](reminders.md): Reminders after iOS Reminders: lists on the left (with All, Flagged and Completed), the chosen list's items on the right.
- [Notebook](notebook.md): The deployment's notebook server (JupyterLab), embedded.
- [Terminal](terminal.md): A terminal program in the browser, such as Emacs, embedded in a panel.

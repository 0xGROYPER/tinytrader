# Alerts

Price alerts per symbol, kept in this browser: a price to rise to, a price to fall to, or a move on the day (either way) of at least a percent. Each fires once when it's met.

**Data:** stock snapshots from alpaca-data (IEX prices, the same the Watchlist shows), read every 15 seconds for the symbols with armed alerts while any console page is open. Nothing watches with every console page closed, and nothing is sent anywhere else.

**Using it:**

- Set one from any of these: the dot after a symbol in a [Watchlist](watchlist.md); **Alert** beside the view's name in the header on a ticker's view (a search in the header opens one); **Alert** in [Search](search.md)'s results; or this widget, by typing a symbol and **Next**.
- Pick *rises to*, *falls to* or *moves (either way) by*, type the price (or the percent for a move) and **Set**.
- A fired alert goes under the bell in the header, which counts the ones not yet looked at, and to a notification when the browser allows them (it asks once, when the first alert is made). It's listed with the time and price it fired at.
- **Re-arm** starts a fired alert again; **×** or **Remove** deletes it. Click a symbol here to point the view's charts at it.
- Open in several tabs, the alerts are shared and each fires once.

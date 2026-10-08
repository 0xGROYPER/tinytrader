# Accounts

A card per brokerage account, live first: equity, the day's change, balances, an equity chart and the largest positions.

**Data:** Every service serving an `account` stream (alpaca-account); positions are valued from the relays' prices. Flatten and Resume act through the account's order gateway (the console's `CONSOLE_GATEWAYS`, with the operator's admin token).

**Using it:**

- **Flatten** (paper accounts with an order gateway): closes every position at market. Click it, check the positions listed, type the account's name and click Flatten now. The gateway halts new opening orders, cancels every open order, then asks the broker to close each position with a market order, in one request it never resends. The card shows each position's result. Options close at whatever the spread is.
- **Resume opening** appears while the account's gateway is halting opens (after a Flatten or a kill), and lets opening orders through again.
- A live account has no gateway, so no Flatten; its card says so.

# Accounts

A card per brokerage account, live first: equity, the day's change, balances, an equity chart, the largest positions and the open orders.

**Data:** Every service serving an `account` stream (alpaca-account); positions are valued from the relays' prices. Flatten and Resume act through the account's order gateway (the console's `CONSOLE_GATEWAYS`, with the operator's admin token).

**Using it:**

- **Flatten** (paper accounts with an order gateway): closes every position at market. Click it, check the positions listed, type the account's name and click Flatten now. The gateway halts new opening orders, cancels every open order, then asks the broker to close each position with a market order, in one request it never resends. The card shows each position's result. Options close at whatever the spread is.
- **Resume opening** appears while the account's gateway is halting opens (after a Flatten or a kill), and lets opening orders through again.
- A live account is never flattened from the console; its card says so.

**Open orders:** every open order on the account as the broker has it, however it was placed: from the console, another client of the gateway, or Alpaca's own app (replacements included). Each shows its legs, limit (`cr` for a credit), filled quantity, status and age. The console's own orders (ids `tt-`) have **Cancel**, through the account's order gateway; others say *elsewhere*: change or cancel them where they were placed. If the orders can't be read the card says so in red, rather than showing none. They're read with the balances, so they follow the account service's hours.

**Payoff:** each account's options are grouped by underlying and expiry, each with a **Payoff** button that loads the legs into the [Spread builder](spreads.md) at their entry prices, for the position's payoff at expiry, maximum loss and chance of profit.

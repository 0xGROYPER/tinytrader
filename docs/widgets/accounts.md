# Accounts

A card per brokerage account, live first: equity, the day's change, balances, an equity chart, the largest positions and the open orders.

**Data:** Every service serving an `account` stream (alpaca-account); positions are valued from the relays' prices. Flatten and Resume act through the account's order gateway (the console's `CONSOLE_GATEWAYS`, with the operator's admin token).

**Using it:**

- **Flatten** (paper accounts with an order gateway): closes every position at market. Click it, check the positions listed, type the account's name and click Flatten now. The gateway halts new opening orders, cancels every open order, then asks the broker to close each position with a market order, in one request it never resends. The card shows each position's result. Options close at whatever the spread is.
- **Resume opening** appears while the account's gateway is halting opens (after a Flatten or a kill), and lets opening orders through again.
- A live account is never flattened from the console; its card says so.

**Open orders:** every open order on the account as the broker has it, however it was placed: from the console, another client of the gateway, or Alpaca's own app (replacements included). Each shows its legs, limit (`cr` for a credit), filled quantity, status and age. The console's own orders (ids `tt-`) have **Cancel**, through the account's order gateway; others say *elsewhere*: change or cancel them where they were placed. If the orders can't be read the card says so in red, rather than showing none. They're read with the balances, so they follow the account service's hours.

**Order ticket** (accounts with an order gateway): one option order of one to four legs, sent as the console's trade client (`POST /api/orders/{account}`), opening or closing. A paper account's order needs only the console's login allowlist. A live account's order asks once for the console's order token (`CONSOLE_ORDER_TOKEN`), kept for that tab only; the server refuses a live order without it, and a refused token is forgotten so the next live order asks again.

- **Close…** beside each option group's Payoff button opens it to close that group: each leg reversed (a long sold, a short bought back), ratios in lowest terms with the quantity carrying the rest, priced at the natural.
- **Spread builder** on a card with an order gateway opens the [Spread builder](spreads.md) with that account picked for its Trade review: build the order there (from a structure, or leg by leg), then Trade. The order ticket here is for closing what's held.
- **Prices:** each leg's bid and ask from the chain service (alpaca-data), read every 5 seconds while the ticket is open. The net is per unit (the quantity multiplies it): positive pays (a debit), negative receives (a credit, shown `cr`); a single leg's limit is its own price. **Natural** pays the ask on buys and takes the bid on sells, so it's the marketable price; **Mid** is the legs' mids. A close starts at the natural, an open at the mid; the buttons set either, and −0.05, −0.01, +0.01, +0.05 nudge the limit. The note under it says whether the limit is marketable (at or past the natural) and how far it is from the mid. A limit you set stays while the quotes refresh.
- **Review** shows the account (the card's, changeable; a warning if it isn't the one the legs came from), the account's open orders, every contract with its count, and what it can cost: receive at least, or pay at most. A live account says so in red and needs its name typed before Send. Nothing is sent until **Send order**. The result says whether the broker accepted or rejected it; if it's not known whether it was sent, don't send it again: check the account's open orders.
- One effect for the whole order: a roll (closing some legs and opening others in one order) isn't possible from the console yet.

**Payoff:** each account's options are grouped by underlying and expiry, each with a **Payoff** button that loads the legs into the [Spread builder](spreads.md) at their entry prices, for the position's payoff at expiry, maximum loss and chance of profit.

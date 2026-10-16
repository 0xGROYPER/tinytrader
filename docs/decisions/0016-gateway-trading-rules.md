# 0016: Trading-rule profiles in the order gateway

- **Status:** Proposed
- **Date:** 2026-10-16

## Context

[The gateway README](../../services/order-gateway/README.md) describes the
gateway as a broker-access passthrough. It has "no strategy, no portfolio and
no planning", and clients own their sizing, positions and P&L. Its only limits
are mechanical: submit rates, open-order and notional caps, a kill switch, and
the start gate. Those limits are the same for every client.

The operator wants rules that protect a human trader from their own bad days:

- a cap on the maximum loss at risk, per correlation group;
- an automatic protective exit on every opening fill;
- a daily loss limit that refuses openings once reached.

These rules must hold whatever device or tool sends the order. A check in the
console alone can be bypassed, so the rules belong where every order passes:
the gateway. That changes the gateway's role. It now enforces portfolio-level
limits, and it holds state for them: start-of-day equity, the day's P&L,
open-position maximum losses and the protective stops it placed.

## Decision

**Mechanism in tinytrader, numbers in the deployment.**

- The gateway reads a **trading-rule profiles file** at startup. The file is
  mounted read-only. Its format is `tinytrader.gateway-profiles/1` (TOML). It
  holds:
  - profiles, each a set of rules and their numbers;
  - the profile bound to each `(account, client)` pair;
  - correlation groups, as lists of underlyings.
- The file lives in the deployment repository (`tinytrader-ops`), never in
  tinytrader. Tinytrader's code names no group, client, account or number.
- A file that fails to parse, or names an unknown client or group, stops the
  gateway at startup. It never starts with a partial set of rules.
- The file's hash is reported in `welcome.pins`.
- Changing the rules means editing the file and restarting the gateway. There
  is no override from the console or any other client.

**Profiles.**

- `user` is for a human trader's client. It applies:
  - a maximum loss at risk per correlation group, as a percentage of
    start-of-day equity (the default is set in ops);
  - an automatic protective exit, at a loss of a set percentage of equity, on
    each position (the default is set in ops);
  - a daily loss limit, as a percentage of start-of-day equity (the default is
    set in ops). It counts **realised losses plus open positions at current
    prices**. Counting only realised losses would let an open loss run without
    limit;
  - limit-only opening orders, so openings are never market orders.
- `automated` is for strategy clients. It applies only the mechanical limits in
  the gateway's existing caps, unless the file gives it more.
- Closes are allowed under every profile. That includes the protective stops'
  own orders, cancelling and replacing a stop, and market closes such as
  Liquidate's.

**Maximum loss** is computed by one shared function in the gateway (the
`tinytrader-ab` work on `intent.rs`). Its shape rules are:

- long options and debit spreads: the premium;
- credit spreads: width × 100 × qty − credit;
- long stock: the full cost;
- stock shorts: refused, as unbounded;
- naked short options: refused;
- an order whose loss cannot be computed, including a market order, gets an
  explicit unknown result. The opening is refused. It never falls back to a
  premium.

**Protective stops.**

- There is **one protective stop per position**, not one per fill. Partial
  fills, adds and a second order on the same contract all feed the same
  position. Whenever the position's quantity changes, the stop is cancelled
  and replaced at the new size. Several stops of different sizes on one
  position are never allowed.
- A single-leg long option gets a broker `stop_limit` sell to close, GTC,
  placed when the position first opens. Its stop price puts the loss at the
  profile's stop percentage of equity.
- A multi-leg position cannot have a broker stop, so the gateway watches it.
  Quotes come from Alpaca option snapshots, called by the gateway with the
  same credentials. This keeps the gateway independent of the console and the
  relays.
- A watched position is never triggered on a stale quote. A quote older than a
  few seconds, or missing, marks that position **stop unwatched**. The gateway
  says so in health and in the console, and keeps polling.
- A stop only closes. If its position is already gone, the close check refuses
  it, so a stop never opens a position.

**Halts.**

- `kill halt_all` does not cancel protective stops. A stop can only close, so
  it only reduces risk. A halt should not leave positions unprotected.
- `halt_all` takes an explicit option to cancel stops as well. It exists for
  the case where the stops are misbehaving.
- The halt status shows each stop's state.

**Live trading.** The live binding is written in the rules file but disabled.
The operator enables it after reviewing paper evidence. Until then the
profiles are tested on paper accounts only, with tiny sizes.

## Consequences

- The gateway's README role section changes. It no longer says the gateway has
  no portfolio. This record supersedes that statement.
- The gateway gains a rules module, persistent state (start-of-day equity,
  the day's P&L, the stops it placed) and an options quote client that is
  separate from the market-data relays.
- The request budget is shared with orders. Quote checks for protective stops
  take priority over other requests and are never shed. Anything else is shed
  first. Watched spread stops matter most under load, so they are not dropped
  when the budget is tight.
- The `not_sent` contract gains a `rule` reason with the rule's name and its
  numbers. Its `detail` fields are additive.
- The start-of-day equity is journaled. A restart reloads it, so a restart
  cannot reset the limit.
- Positions opened outside the gateway, such as those held at rollout or
  opened in Alpaca's app, count toward their group's maximum loss when it can
  be computed from the broker's data: the cost basis of long options and
  stock. Only a position whose maximum loss cannot be computed blocks openings
  in its group. Such a block names the position and the reason.
- A partial close is not assumed to carry zero risk. Its rule comes from the
  shared function, once it lands.

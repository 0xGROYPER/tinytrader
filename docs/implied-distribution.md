# The implied distribution

What the options market prices for where a stock finishes at an expiry: the whole distribution, skew and fat tails included, not only the at-the-money straddle. alpaca-data fits it for every expiry of a chain it serves (`services/alpaca-data/src/smile.rs`); Candles, the Spread builder and the Options screener use it.

## Method

1. **Forward.** Put-call parity at the strikes nearest the price, `F = K + C − P`, the median of up to three; dividends and carry are in it. Zero interest rates otherwise.
2. **Quotes.** At each strike the out-of-the-money option (puts below the forward, calls at or above), with a bid, not crossed, a mid of at least $0.02 and a spread of at most 60% of the mid. Its implied volatility is inverted from the mid with Black's formula on the forward (European exercise; Alpaca's own IVs use another model), and kept between 2% and 400%.
3. **Smile.** Total variance `w = σ²T` against log-moneyness `k = ln(K/F)`, fitted as raw SVI, `w = a + c(k − m) + d·√((k − m)² + s²)`, by weighted least squares over a grid of `m` and `s`. Fits breaking `d ≥ 0`, `|c| ≤ d`, the wing-slope bound `d + |c| ≤ 2` (Lee) or with negative variance are rejected; residuals are weighted to equalise errors in volatility, and towards tight quotes. Without an SVI fit, a quadratic in `k`, continued linearly past the quoted strikes.
4. **Distribution.** `P(S_T > K) = N(d2) − φ(d2)·w′(k) / (2√w)`: Black's digital plus the smile's slope correction (from differentiating call prices by strike). Computed on a grid of ±7 at-the-money standard deviations and made monotone.
5. **Quality.** `good`: at least 8 usable strikes, 3 or more each side of the forward, and an RMS volatility error of at most 2 points. `rough`: at least 5 strikes, some each side, at most 5 points. Else `poor`: consumers fall back to the straddle.

## What each expiry carries

In the chain response (`/api/stream/alpaca-data/chain`), each entry of `expiries` gains `distribution`:

| Field | Meaning |
|---|---|
| `forward` | The forward used |
| `quality`, `reason` | `good`, `rough` or `poor` (and why, when poor) |
| `model`, `rms_iv` | `svi` or `quadratic`; RMS error in volatility over the strikes used |
| `strikes`, `puts`, `calls`, `usable` | Strikes used, each side, and the share of out-of-the-money quotes usable |
| `quantiles` | Prices at 2.5, 16, 25, 50, 75, 84 and 97.5% (`p2_5` … `p97_5`) |
| `p_inside_straddle` | Chance of finishing within ± the at-the-money straddle of the price |
| `mean_abs_move` | Expected absolute move over the price |
| `rr25` | 25-delta risk reversal: put less call volatility at the fitted smile |
| `tail_ratio` | Distance to the 2.5% quantile over distance to the 97.5% (above 1: a longer left tail) |
| `cdf` | 81 points `[price, P(S_T ≤ price)]` between the 0.1% and 99.9% quantiles |

The screener's rows gain `skew`, the 30-day expiry's `rr25` when the fit isn't poor.

## Reading it

- These are the market's prices, not forecasts. Options carry a premium (implied volatility above what follows, on average), largest for indexes, so stocks finish inside the ranges somewhat more often than priced.
- On 9 October 2026's close: SPY's weeklies were peaked, with 62-63% inside the straddle (57% on a bell curve), puts over calls by 1-2 points, a longer left tail; NFLX through its 20 October report was close to symmetric, slightly upside-skewed; thinly quoted names (BSP) were `poor`.
- Alpaca's option quotes are its indicative feed: wide for far-from-the-money strikes, which the spread filter drops. Short-dated wings rest on few quotes.

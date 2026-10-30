"""Order intents in the gateway's form (order-gateway.md, section 6). Prices are
decimal strings; the gateway refuses a market order that opens a position."""

from __future__ import annotations


def _price(p: float | str) -> str:
    return p if isinstance(p, str) else f"{p:.2f}"


def _instrument(symbol: str) -> dict:
    s = symbol.replace(" ", "")
    is_option = len(s) > 15 and s[-9] in "CP" and s[-8:].isdigit()
    return {"class": "option" if is_option else "equity", "symbol": s}


def single(symbol: str, side: str, effect: str, qty: int, limit: float | str, tif: str = "day") -> dict:
    """One leg: ``side`` ``buy``/``sell``, ``effect`` ``open``/``close``."""
    return {
        "shape": "single",
        "instrument": _instrument(symbol),
        "side": side,
        "effect": effect,
        "qty": qty,
        "type": "limit",
        "limit_price": _price(limit),
        "time_in_force": tif,
    }


def combo(legs: list[tuple[str, int, str, str]], qty: int, limit: float | str) -> dict:
    """Several option legs, each ``(symbol, ratio, side, effect)``, at a net
    ``limit`` per unit: positive a debit, negative a credit."""
    return {
        "shape": "combo",
        "legs": [
            {"instrument": _instrument(s), "ratio": r, "side": side, "effect": eff}
            for s, r, side, eff in legs
        ],
        "qty": qty,
        "type": "limit",
        "limit_price": _price(limit),
        "time_in_force": "day",
    }

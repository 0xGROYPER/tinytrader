"""The top of book of every instrument the data stream has quoted: a view over
delivered records, kept for convenience. A strategy that wants something else
keeps its own from ``on_record``."""

from __future__ import annotations

import datetime as dt
from dataclasses import dataclass

UNDEF_PRICE = 2**63 - 1
# DBN rtypes (dbn's RType).
MBP_1 = 0x01
SYMBOL_MAPPING = 0x16
CMBP_1 = 0xB1
CBBO_1S = 0xC0
CBBO_1M = 0xC1
TCBBO = 0xC2
BBO_1S = 0xC3
BBO_1M = 0xC4
QUOTES = {MBP_1, CMBP_1, CBBO_1S, CBBO_1M, BBO_1S, BBO_1M}


@dataclass(frozen=True)
class Quote:
    """A top of book: prices in dollars, ``None`` for a side that isn't there;
    ``ts`` when it was received (ns), for its age."""

    bid: float | None
    ask: float | None
    bid_size: int
    ask_size: int
    ts: int

    @property
    def mid(self) -> float | None:
        if self.bid is None or self.ask is None:
            return None
        return (self.bid + self.ask) / 2

    @property
    def usable(self) -> bool:
        """Both sides there and not crossed."""
        return self.bid is not None and self.ask is not None and self.bid <= self.ask


def compact(symbol: str) -> str:
    """An OPRA raw symbol without its padding: ``SPY   261027C00580000`` →
    ``SPY261027C00580000``."""
    return symbol.replace(" ", "")


def occ(symbol: str) -> tuple[str, dt.date, str, float] | None:
    """Root, expiry, ``C``/``P`` and strike of a compact OCC symbol, else None."""
    s = compact(symbol)
    if len(s) < 16 or s[-9] not in "CP" or not s[-8:].isdigit() or not s[-15:-9].isdigit():
        return None
    ymd = s[-15:-9]
    try:
        expiry = dt.date(2000 + int(ymd[:2]), int(ymd[2:4]), int(ymd[4:]))
    except ValueError:
        return None
    return s[:-15], expiry, s[-9], int(s[-8:]) / 1000


def _px(raw: int) -> float | None:
    return None if raw == UNDEF_PRICE else raw / 1e9


class Book:
    """Instruments by id (from symbol mappings and definitions) and their last
    quote. A ``tcbbo`` record's quote (the one before its trade) counts only for an
    instrument no quote record has quoted, as in the simulator."""

    def __init__(self) -> None:
        self.symbols: dict[int, str] = {}
        self._ids: dict[str, int] = {}
        self._quotes: dict[int, Quote] = {}
        self._quoted: set[int] = set()

    def apply(self, rec) -> None:
        """Takes one delivered record (a ``databento_dbn`` record)."""
        rtype = int(getattr(rec, "rtype", -1))
        iid = rec.instrument_id
        if rtype == SYMBOL_MAPPING:
            self._map(iid, rec.stype_out_symbol)
        elif hasattr(rec, "raw_symbol") and hasattr(rec, "strike_price"):
            self._map(iid, rec.raw_symbol)
        elif rtype in QUOTES or rtype == TCBBO:
            if rtype == TCBBO and iid in self._quoted:
                return
            if rtype != TCBBO:
                self._quoted.add(iid)
            lv = rec.levels[0]
            ts = rec.ts_recv if rtype != MBP_1 else rec.ts_event
            self._quotes[iid] = Quote(_px(lv.bid_px), _px(lv.ask_px), lv.bid_sz, lv.ask_sz, ts)

    def _map(self, iid: int, symbol: str) -> None:
        self.symbols[iid] = symbol
        self._ids[compact(symbol)] = iid

    def quote(self, symbol: str) -> Quote | None:
        """The last quote of a symbol (raw or compact OCC, or a stock)."""
        iid = self._ids.get(compact(symbol))
        return None if iid is None else self._quotes.get(iid)

    def expiries(self, root: str) -> list[dt.date]:
        """The expiries of a root's quoted options, in order."""
        out = set()
        for iid in self._quotes:
            o = occ(self.symbols.get(iid, ""))
            if o and o[0] == root:
                out.add(o[1])
        return sorted(out)

    def chain(self, root: str, expiry: dt.date) -> dict[tuple[float, str], Quote]:
        """One expiry's quoted options: ``(strike, "C"|"P") → Quote``."""
        out = {}
        for iid, q in self._quotes.items():
            o = occ(self.symbols.get(iid, ""))
            if o and o[0] == root and o[1] == expiry:
                out[(o[3], o[2])] = q
        return out

"""A strategy as a client of a tinytrader session (decision 0022, section 7).

Write a :class:`Strategy` subclass and hand it to :func:`run`::

    class Watch(Strategy):
        name = "watch"

        def on_start(self, ctx):
            ctx.every(60, start="09:31", end="15:59")

        def on_timer(self, ctx, timer_id, ts):
            q = ctx.book.quote("SPY")

    run(Watch())   # the session's control address from TT_SESSION

The session decides time. A strategy sees only records already delivered, its
orders take effect after the step it sends them in, and every report of a step
is in hand before the strategy acts on that step.
"""

from .book import Book, Quote
from .intent import combo, single
from .session import Context, SessionError, Strategy, run

__all__ = ["Book", "Context", "Quote", "SessionError", "Strategy", "combo", "run", "single"]

"""The session loop: ``tinytrader.session/1`` on the control connection, the
data stream (``tinytrader.relay/1``, decoded with ``databento-dbn``) and one
gateway (``tinytrader.gateway/1``) per venue, in lockstep (docs/api/session.md).

Each step, in this order:

1. its records are read from the data stream and given to ``on_record``, after
   the book has taken them;
2. each venue's events up to the step's ``reports`` go to ``on_report``;
3. its timers go to ``on_timer``;
4. ``ready`` answers with the orders sent and the timers set.
"""

from __future__ import annotations

import datetime as dt
import json
import os
import socket
import time
from typing import Any
from zoneinfo import ZoneInfo

import databento_dbn

from .book import Book

PROTOCOL = "tinytrader.session/1"
GATEWAY = "tinytrader.gateway/1"
# A gateway closes a client silent for 30 s of wall time; a slow strategy keeps
# each connection alive between its own messages.
HEARTBEAT_S = 10.0


class SessionError(RuntimeError):
    """The session or a venue refused something, or a connection failed."""


class _Lines:
    """Newline-delimited JSON over a socket."""

    def __init__(self, addr: str):
        host, port = addr.rsplit(":", 1)
        self.sock = socket.create_connection((host, int(port)))
        self.file = self.sock.makefile("r", encoding="utf-8")

    def send(self, msg: dict) -> None:
        self.sock.sendall((json.dumps(msg, separators=(",", ":")) + "\n").encode())

    def close(self) -> None:
        self.file.close()
        self.sock.close()

    def read(self) -> dict:
        line = self.file.readline()
        if not line:
            raise SessionError("connection closed")
        return json.loads(line)


class _Venue:
    """One venue's gateway connection, as the strategy's client."""

    def __init__(self, name: str, spec: dict):
        self.name = name
        self.prefix = spec["prefix"]
        self.conn = _Lines(spec["addr"])
        self._send({"type": "hello", "client": spec["client"], "token": spec["token"],
                    "resume_after_seq": None, "cancel_on_disconnect": False})
        self.welcome = self.conn.read()
        if self.welcome.get("type") != "welcome":
            raise SessionError(f"venue {name}: {self.welcome}")
        self.sent = 0          # order messages: submits and cancels
        self.seen = 0          # the last event read
        self.pending: list[dict] = []
        self.requests = 0
        self.orders = 0
        self.last_send = time.monotonic()

    def _send(self, msg: dict) -> None:
        self.conn.send({"schema": GATEWAY, **msg})
        self.last_send = time.monotonic()

    def request(self, kind: str, **fields: Any) -> dict:
        """Sends a request and returns its response; events read meanwhile wait for
        their step."""
        self.requests += 1
        rid = f"r{self.requests}"
        self._send({"type": kind, "request_id": rid, **fields})
        if kind in ("submit", "cancel"):
            self.sent += 1
        while True:
            m = self.conn.read()
            if m.get("type") == "response" and m.get("request_id") == rid:
                return m
            if m.get("type") == "error":
                raise SessionError(f"venue {self.name}: {m}")
            self._keep(m)

    def _keep(self, m: dict) -> None:
        if m.get("type") == "event":
            self.pending.append(m)
            self.seen = max(self.seen, m.get("seq", 0))

    def events_to(self, seq: int) -> list[dict]:
        """Every event up to ``seq``, in order, reading as needed."""
        while self.seen < seq:
            m = self.conn.read()
            if m.get("type") == "error":
                raise SessionError(f"venue {self.name}: {m}")
            self._keep(m)
        out = [e for e in self.pending if e.get("seq", 0) <= seq]
        self.pending = [e for e in self.pending if e.get("seq", 0) > seq]
        return out

    def keep_alive(self) -> None:
        if time.monotonic() - self.last_send > HEARTBEAT_S:
            self._send({"type": "heartbeat"})


class Context:
    """What a strategy acts through: the time, the book, its venues and its timers."""

    def __init__(self, welcome: dict, venues: dict[str, _Venue]):
        self.session = welcome["session"]
        self.days = [dt.date.fromisoformat(d) for d in welcome["days"]]
        self.now = 0
        self.book = Book()
        self._venues = venues
        self._timers: list[tuple[int, str]] = []
        self._every: list[tuple[int, int, int, str, ZoneInfo]] = []
        self._last = max(
            int(dt.datetime.combine(self.days[-1] + dt.timedelta(days=1), dt.time(), dt.timezone.utc).timestamp())
            * 1_000_000_000 - 1,
            0,
        )
        self.series: list[dict] = []

    @property
    def venues(self) -> list[str]:
        return list(self._venues)

    def _venue(self, name: str | None) -> _Venue:
        if name is None:
            if len(self._venues) != 1:
                raise SessionError("name the venue: there are several")
            return next(iter(self._venues.values()))
        return self._venues[name]

    # Orders.
    def submit(self, intent: dict, venue: str | None = None, tag: str = "") -> str:
        """Submits an order (see :mod:`tinytrader_client.intent`); returns its
        client order id. It takes effect after this step."""
        v = self._venue(venue)
        v.orders += 1
        cid = f"{v.prefix}{tag}{v.orders}"
        r = v.request("submit", intent={"client_order_id": cid, **intent})
        if r.get("outcome") not in ("accepted", "duplicate"):
            raise SessionError(f"submit {cid}: {r}")
        return cid

    def cancel(self, client_order_id: str, venue: str | None = None) -> dict:
        return self._venue(venue).request("cancel", client_order_id=client_order_id)

    def positions(self, venue: str | None = None) -> list[dict]:
        return self._venue(venue).request("positions")["positions"]

    def account(self, venue: str | None = None) -> dict:
        return self._venue(venue).request("account")["account"]

    def open_orders(self, venue: str | None = None) -> list[dict]:
        return self._venue(venue).request("open_orders")["orders"]

    # Timers.
    def set_timer(self, at: int, timer_id: str) -> None:
        """Calls ``on_timer`` at ``at`` (ns): after this step, by the run's end."""
        if at <= self.now:
            raise SessionError(f"timer {timer_id} at {at} isn't after now ({self.now})")
        self._timers.append((at, timer_id))

    def every(self, seconds: int, start: str, end: str, timer_id: str = "every",
              tz: str = "America/New_York") -> None:
        """A timer every ``seconds`` from ``start`` to ``end`` (``HH:MM``, both
        included, in ``tz``) on every day of the run, one at a time."""
        h0, m0 = map(int, start.split(":"))
        h1, m1 = map(int, end.split(":"))
        self._every.append((seconds, h0 * 60 + m0, h1 * 60 + m1, timer_id, ZoneInfo(tz)))

    def _next_every(self) -> list[tuple[int, str]]:
        out = []
        for seconds, start, end, tid, zone in self._every:
            t = self._next_tick(seconds, start, end, zone)
            if t is not None:
                out.append((t, tid))
        return out

    def _next_tick(self, seconds: int, start: int, end: int, zone: ZoneInfo) -> int | None:
        now = dt.datetime.fromtimestamp(self.now / 1e9, dt.timezone.utc) if self.now else None
        for day in self.days:
            t = dt.datetime.combine(day, dt.time(start // 60, start % 60), zone)
            last = dt.datetime.combine(day, dt.time(end // 60, end % 60), zone)
            while t <= last:
                if now is None or t > now:
                    ns = int(t.timestamp()) * 1_000_000_000
                    return ns if ns <= self._last else None
                t += dt.timedelta(seconds=seconds)
        return None

    def record(self, series: str, value: Any) -> None:
        """Keeps a named value at the current time; :func:`run` returns them."""
        self.series.append({"ts": self.now, "series": series, "value": value})


class Strategy:
    """Override what you need. ``name`` and ``revision`` identify the code in the
    run record."""

    name = "strategy"
    revision = ""

    def on_start(self, ctx: Context) -> None: ...

    def on_record(self, ctx: Context, rec) -> None: ...

    def on_report(self, ctx: Context, venue: str, event: dict) -> None: ...

    def on_timer(self, ctx: Context, timer_id: str, ts: int) -> None: ...

    def on_end(self, ctx: Context, end: dict) -> None: ...


def run(strategy: Strategy, control: str | None = None) -> dict:
    """Runs ``strategy`` in the session at ``control`` (default ``TT_SESSION``)
    until it ends. Returns the session's ``end`` with the strategy's series."""
    addr = control or os.environ.get("TT_SESSION")
    if not addr:
        raise SessionError("no session: pass its control address or set TT_SESSION")
    ctl = _Lines(addr)
    ctl.send({"type": "hello", "protocol": PROTOCOL, "strategy": strategy.name,
              "revision": strategy.revision})
    welcome = ctl.read()
    if welcome.get("type") != "welcome":
        raise SessionError(f"session: {welcome}")
    data = _Lines(welcome["data"])
    data.send({"protocol": "tinytrader.relay/1", "client": strategy.name})
    venues = {n: _Venue(n, spec) for n, spec in welcome["orders"].items()}
    try:
        return _loop(strategy, ctl, data, venues, welcome)
    finally:
        for conn in [ctl, data, *(v.conn for v in venues.values())]:
            conn.close()


def _loop(strategy: Strategy, ctl: _Lines, data: _Lines, venues: dict[str, _Venue], welcome: dict) -> dict:
    decoder = databento_dbn.DBNDecoder()
    ctx = Context(welcome, venues)
    strategy.on_start(ctx)
    armed: set[tuple[int, str]] = set(ctx._timers + ctx._next_every())
    ctx._timers.clear()
    ctl.send({"type": "ready", "as_of": 0, "sent": {n: v.sent for n, v in venues.items()},
              "timers": [{"at": at, "id": tid} for at, tid in sorted(armed)]})
    pending: list = []
    metadata = True
    while True:
        msg = ctl.read()
        if msg.get("type") == "end":
            for name, seq in (msg.get("reports") or {}).items():
                for e in venues[name].events_to(seq):
                    strategy.on_report(ctx, name, e)
            strategy.on_end(ctx, msg)
            return {**msg, "series": ctx.series}
        if msg.get("type") != "step":
            raise SessionError(f"session: {msg}")
        ctx.now = msg["as_of"]
        want = msg["records"]
        while len(pending) < want:
            chunk = data.sock.recv(1 << 20)
            if not chunk:
                raise SessionError("the data stream closed")
            decoder.write(chunk)
            recs = decoder.decode()
            if metadata and recs and isinstance(recs[0], databento_dbn.Metadata):
                recs = recs[1:]
                metadata = False
            pending.extend(recs)
        for rec in pending[:want]:
            ctx.book.apply(rec)
            strategy.on_record(ctx, rec)
        del pending[:want]
        for name, seq in msg["reports"].items():
            for e in venues[name].events_to(seq):
                strategy.on_report(ctx, name, e)
        for tid in msg["timers"]:
            armed.discard((ctx.now, tid))
            strategy.on_timer(ctx, tid, ctx.now)
        new = [t for t in ctx._timers + ctx._next_every() if t not in armed]
        ctx._timers.clear()
        armed.update(new)
        for v in venues.values():
            v.keep_alive()
        ctl.send({"type": "ready", "as_of": ctx.now,
                  "sent": {n: v.sent for n, v in venues.items()},
                  "timers": [{"at": at, "id": tid} for at, tid in new]})

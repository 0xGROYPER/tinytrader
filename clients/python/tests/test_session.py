"""The client against a real session: the session binary and its fixture example,
built by cargo (`cargo build -p session --bins --examples`). Their paths come from
TT_SESSION_BIN and TT_FIXTURE_BIN, or the workspace's target directory."""

import os
import subprocess
import threading
import unittest
from pathlib import Path

from tinytrader_client import Strategy, run, single

ROOT = Path(__file__).resolve().parents[3]
TARGET = Path(os.environ.get("CARGO_TARGET_DIR", ROOT / "target")) / "debug"
SESSION = Path(os.environ.get("TT_SESSION_BIN", TARGET / "tinytrader-session"))
FIXTURE = Path(os.environ.get("TT_FIXTURE_BIN", TARGET / "examples" / "fixture"))
# Gateway state may not live under /tmp, so runs go beside the build.
WORK = Path(os.environ.get("TT_TEST_ROOT", TARGET.parent / "python-tests"))

T0 = 1_793_109_600_000_000_000  # 2026-10-27 10:00 New York
MIN = 60_000_000_000


class Toy(Strategy):
    """Buys one call at 1.25 at 10:00, watching each minute to 10:05."""

    name = "toy"
    revision = "test"

    def __init__(self):
        self.timers, self.fills, self.records = [], [], 0

    def on_start(self, ctx):
        ctx.every(60, start="10:00", end="10:05")

    def on_record(self, ctx, rec):
        self.records += 1

    def on_timer(self, ctx, timer_id, ts):
        self.timers.append(ts)
        if ts == T0:
            ctx.submit(single("SPY261027C00580000", "buy", "open", 1, 1.25))
        q = ctx.book.quote("SPY   261027C00580000")
        ctx.record("ask", q.ask if q else None)

    def on_report(self, ctx, venue, event):
        if event.get("event") == "fill":
            self.fills.append((event["fill"]["price"], event["ts_broker"], ctx.now))


def session(name: str, strategy: Strategy) -> dict:
    base = WORK / f"{name}-{os.getpid()}"
    subprocess.run(["rm", "-rf", str(base)], check=True)
    subprocess.run([str(FIXTURE), str(base)], check=True)
    proc = subprocess.Popen(
        [str(SESSION), "run", str(base / "session.toml"), "--dir", str(base / "run")],
        stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, text=True,
    )
    ready = proc.stdout.readline()
    control = dict(kv.split("=", 1) for kv in ready.split()[1:])["control"]
    result = {}
    t = threading.Thread(target=lambda: result.update(run(strategy, control)), daemon=True)
    t.start()
    t.join(60)
    proc.wait(60)
    proc.stdout.close()
    return result


@unittest.skipUnless(SESSION.exists() and FIXTURE.exists(), "build the session binary and its examples first")
class SessionTest(unittest.TestCase):
    def test_a_strategy_runs_in_lockstep(self):
        toy = Toy()
        end = session("lockstep", toy)
        self.assertEqual(end["status"], "complete", end)
        self.assertEqual(toy.timers, [T0 + k * MIN for k in range(6)])
        # Sent at 10:00, filled when the ask comes down at 10:02, and in hand by the
        # time the 10:02 timer runs.
        self.assertEqual(toy.fills, [("1.24", T0 + 2 * MIN, T0 + 2 * MIN)])
        self.assertEqual([s["value"] for s in end["series"]], [1.30, 1.30, 1.24, 1.24, 1.24, 1.24])
        self.assertEqual(toy.records, 7)  # the day's mapping and six quotes

    def test_a_refused_order_is_the_strategy_s_to_handle(self):
        class Refused(Toy):
            def on_timer(self, ctx, timer_id, ts):
                self.timers.append(ts)
                if ts == T0:
                    # A put the venue's profile doesn't allow: refused, not fatal.
                    self.result = ctx.submit(single("QQQ261027P00500000", "buy", "open", 1, 1.0))
                    self.why = ctx.last_response

        r = Refused()
        end = session("refused", r)
        self.assertEqual(end["status"], "complete", end)
        self.assertIsNone(r.result)
        self.assertIn(r.why["outcome"], ("not_sent", "rejected"))

    def test_the_same_run_twice_gives_the_same_audit(self):
        a, b = session("twice-a", Toy()), session("twice-b", Toy())
        self.assertEqual(a["audit"], b["audit"])


if __name__ == "__main__":
    unittest.main()

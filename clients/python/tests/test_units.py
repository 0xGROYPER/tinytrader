import datetime as dt
import unittest

from tinytrader_client import combo, single
from tinytrader_client.book import Quote, compact, occ


class Units(unittest.TestCase):
    def test_occ_symbols(self):
        self.assertEqual(compact("SPY   261027C00580000"), "SPY261027C00580000")
        self.assertEqual(occ("SPY   261027C00580000"), ("SPY", dt.date(2026, 10, 27), "C", 580.0))
        self.assertEqual(occ("SPXW  261027P05812500"), ("SPXW", dt.date(2026, 10, 27), "P", 5812.5))
        self.assertIsNone(occ("SPY"))

    def test_quotes(self):
        q = Quote(1.20, 1.24, 10, 10, 0)
        self.assertAlmostEqual(q.mid, 1.22)
        self.assertTrue(q.usable)
        self.assertFalse(Quote(1.30, 1.24, 1, 1, 0).usable)
        self.assertIsNone(Quote(None, 1.24, 0, 1, 0).mid)

    def test_intents(self):
        s = single("SPY", "buy", "open", 10, 580.1)
        self.assertEqual(s["instrument"], {"class": "equity", "symbol": "SPY"})
        self.assertEqual(s["limit_price"], "580.10")
        c = combo([("SPY261027P00570000", 1, "sell", "open"), ("SPY261027P00565000", 1, "buy", "open")], 2, -0.4)
        self.assertEqual(c["limit_price"], "-0.40")
        self.assertEqual(c["legs"][0]["instrument"]["class"], "option")
        self.assertEqual(single("SPY", "buy", "open", 1, "1.25")["limit_price"], "1.25")
        with self.assertRaises(ValueError):
            single("SPY", "buy", "open", 1, 1.234)
        with self.assertRaises(ValueError):
            combo([("SPY261027P00570000", 1, "sell", "open")], 1, -0.004)


if __name__ == "__main__":
    unittest.main()

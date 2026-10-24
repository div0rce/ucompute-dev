"""Offline checks of the soak's verdict logic (no daemon): python3 -m unittest discover -s tests/soak"""
import unittest

from local_soak import classify, p95, verdict


def sample(elapsed, rss=100_000, fd=40, orphans=()):
    return {"elapsed_s": elapsed, "rss_kib": rss, "fds": fd, "orphans": list(orphans)}


class Verdict(unittest.TestCase):
    def test_classify(self):
        self.assertEqual(classify(200, {}), "ok")
        self.assertEqual(classify(429, {"error": {"code": "overloaded"}}), "overloaded")
        self.assertEqual(classify(500, {"error": {"code": "made_up"}}), "untyped:500")
        self.assertEqual(classify(502, "<html>bad gateway</html>"), "untyped:502")
        self.assertEqual(classify(500, {"error": "a string"}), "untyped:500")

    def test_p95(self):
        self.assertIsNone(p95([]))
        self.assertEqual(p95([5]), 5)
        self.assertEqual(p95(list(range(1, 101))), 95)
        self.assertEqual(p95(list(range(1, 21))), 19)

    def test_clean_run_passes(self):
        self.assertEqual(verdict([sample(60), sample(3600), sample(7200, rss=110_000, fd=50)], 0, 0, 0, 64, 16), [])

    def test_each_bound_fails(self):
        base = [sample(3600), sample(7200)]
        self.assertTrue(verdict(base, 1, 0, 0, 64, 16))
        self.assertTrue(verdict(base, 0, 1, 0, 64, 16))
        self.assertTrue(verdict(base, 0, 0, 1, 64, 16))
        self.assertTrue(verdict([sample(60, orphans=[123])] + base, 0, 0, 0, 64, 16))
        self.assertTrue(verdict([sample(3600), sample(7200, rss=100_000 + 65 * 1024)], 0, 0, 0, 64, 16))
        self.assertTrue(verdict([sample(3600), sample(7200, fd=57)], 0, 0, 0, 64, 16))

    def test_first_hour_growth_is_warmup(self):
        self.assertEqual(verdict([sample(60, rss=10_000, fd=5), sample(3600), sample(7200)], 0, 0, 0, 64, 16), [])


if __name__ == "__main__":
    unittest.main()

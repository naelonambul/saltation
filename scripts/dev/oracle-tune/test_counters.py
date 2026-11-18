"""Regression witnesses for connection turnover and retained lifetime stats."""
import tempfile
from pathlib import Path
import unittest

import counters


class CounterTests(unittest.TestCase):
    def test_retired_session_cannot_look_like_a_gain(self):
        before = {('1', '10', 'user_calls'): 100}
        after = {('2', '11', 'user_calls'): 5}
        self.assertIsNone(counters.session_delta(before, after))
        self.assertEqual(counters.rate(None, 10), '?')

    def test_same_session_and_reset(self):
        before = {('1', '10', 'user_calls'): 100}
        self.assertEqual(counters.session_delta(before, {('1', '10', 'user_calls'): 130}), {'user_calls': 30})
        self.assertIsNone(counters.session_delta(before, {('1', '10', 'user_calls'): 90}))
        self.assertIsNone(counters.session_delta({}, {}))

    def test_lifetime_stats_survive_connection_turnover(self):
        with tempfile.TemporaryDirectory() as directory:
            start, end = Path(directory) / 'start', Path(directory) / 'end'
            start.write_text('startup same\nsys user_calls 100\nsst 1 10 user_calls 100\n')
            end.write_text('startup same\nsys user_calls 140\nsst 2 11 user_calls 5\n')
            self.assertEqual(counters.database_delta(start, end), {'user_calls': 40})
            end.write_text('startup restarted\nsys user_calls 5\n')
            self.assertIsNone(counters.database_delta(start, end))
            end.write_text('startup same\nsys user_calls 90\n')
            self.assertIsNone(counters.database_delta(start, end))
            end.write_text('sys user_calls 140\n')
            self.assertIsNone(counters.database_delta(start, end))


if __name__ == '__main__':
    unittest.main()

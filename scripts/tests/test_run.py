"""`scripts/tests/run.py`: a run passes only if unittest finished, ran tests, and reported OK."""

import subprocess
import sys
import tempfile
import textwrap
import unittest
from pathlib import Path

RUN = Path(__file__).resolve().parent / "run.py"


class Runner(unittest.TestCase):
    def setUp(self):
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        self.dir = Path(tmp.name)

    def module(self, name: str, body: str):
        (self.dir / f"{name}.py").write_text(textwrap.dedent(body))

    def run_tests(self, *extra):
        return subprocess.run(
            [sys.executable, str(RUN), "--start", str(self.dir), "--top", str(self.dir), *extra],
            capture_output=True, text=True, timeout=60,
        )

    def assertVerdict(self, proc, code, fragment):
        self.assertEqual(proc.returncode, code, proc.stderr)
        self.assertIn(fragment, proc.stderr)

    def test_passing_tests_pass(self):
        self.module("test_ok", """
            import unittest
            class T(unittest.TestCase):
                def test_one(self):
                    pass
                def test_two(self):
                    pass
        """)
        proc = self.run_tests()
        self.assertVerdict(proc, 0, "Ran 2 tests")
        self.assertNotIn("FAILED", proc.stderr)

    def test_a_failing_test_fails(self):
        self.module("test_bad", """
            import unittest
            class T(unittest.TestCase):
                def test_one(self):
                    self.fail("no")
        """)
        self.assertVerdict(self.run_tests(), 1, "FAILED: unittest reported FAILED")

    def test_an_early_exit_with_status_zero_fails(self):
        self.module("test_exit", """
            import os, unittest
            class T(unittest.TestCase):
                def test_one(self):
                    os._exit(0)
        """)
        self.assertVerdict(self.run_tests(), 1, "no final summary")

    def test_zero_tests_fail(self):
        self.module("test_empty", "import unittest\n")
        self.assertVerdict(self.run_tests(), 1, "FAILED")

    def test_an_import_error_fails(self):
        self.module("test_import", "import no_such_module_here\n")
        self.assertVerdict(self.run_tests(), 1, "FAILED: unittest reported FAILED")

    def test_a_skipped_test_is_reported_and_passes(self):
        self.module("test_skip", """
            import unittest
            class T(unittest.TestCase):
                @unittest.skip("not here")
                def test_one(self):
                    pass
        """)
        self.assertVerdict(self.run_tests(), 0, "OK (skipped=1)")

    def test_unknown_arguments_go_to_unittest(self):
        self.module("test_pick", """
            import unittest
            class T(unittest.TestCase):
                def test_alpha(self):
                    pass
                def test_beta(self):
                    pass
        """)
        self.assertVerdict(self.run_tests("-k", "alpha"), 0, "Ran 1 test")


class Verdict(unittest.TestCase):
    def setUp(self):
        sys.path.insert(0, str(RUN.parent))
        self.addCleanup(sys.path.remove, str(RUN.parent))
        import run  # noqa: E402

        self.verdict = run.verdict

    def test_the_last_summary_decides(self):
        text = "Ran 3 tests in 0.1s\n\nOK\nRan 1 test in 0.0s\n\nFAILED (failures=1)\n"
        self.assertIn("FAILED", self.verdict(1, text))

    def test_ok_with_a_nonzero_exit_fails(self):
        self.assertIn("exit 2", self.verdict(2, "Ran 1 test in 0.0s\n\nOK\n"))

    def test_ok_passes(self):
        self.assertIsNone(self.verdict(0, "...\nRan 4 tests in 0.2s\n\nOK (skipped=1)\n"))


if __name__ == "__main__":
    unittest.main()

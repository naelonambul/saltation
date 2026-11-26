import contextlib
import io
import json
from pathlib import Path
import subprocess
from tempfile import TemporaryDirectory
import unittest

from scripts.ops.check_new_comments import findings, main, new_annotations


class AnnotationTests(unittest.TestCase):
    def test_existing_inline_comment_survives_code_edits_and_line_shifts(self):
        before = b"x = 1  # existing\n"
        after = b"prefix = 0\nx = 2  # existing\n"
        self.assertEqual(list(new_annotations(before, after, set())), [])

    def test_added_duplicate_is_detected_when_existing_line_stays(self):
        before = b"x = 1  # existing\n"
        after = b"y = 1  # existing\nx = 1  # existing\n"
        self.assertEqual(list(new_annotations(before, after, set())), [(1, "comment")])

    def test_only_new_lines_of_docstrings_and_comments_are_reported(self):
        before = b'def f():\n    """existing\n    """\n    return 1\n'
        after = b'def f():\n    """existing\n    additional text\n    """\n    return "# literal"  # added\n'
        self.assertEqual(
            list(new_annotations(before, after, set())),
            [(3, "docstring"), (5, "comment")],
        )
        self.assertEqual(
            list(new_annotations(before, after, {"additional text", 'return "# literal"  # added'})),
            [],
        )

    def test_detects_nested_and_module_docstrings(self):
        source = b'"""module"""\nclass C:\n    "class"\n    async def f(self):\n        "method"\n'
        self.assertEqual(
            list(new_annotations(b"", source, set())),
            [(1, "docstring"), (3, "docstring"), (5, "docstring")],
        )


    def test_unrelated_docstring_removal_cannot_hide_an_addition(self):
        before = b'def first():\n    """alpha"""\n    return 1\n\ndef second():\n    return 2\n'
        after = b'def first():\n    return 1\n\ndef second():\n    """alpha"""\n    return 2\n'
        self.assertEqual(list(new_annotations(before, after, set())), [(5, "docstring")])

    def test_unrelated_inline_comment_removal_cannot_hide_an_addition(self):
        before = b'first = 1  # units\nsecond = 2\nthird = 3\n'
        after = b'first = 1\nsecond = 2\nthird = 3  # units\n'
        self.assertEqual(list(new_annotations(before, after, set())), [(3, "comment")])


class RepositoryTests(unittest.TestCase):
    def setUp(self):
        directory = TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        self.root = Path(directory.name)
        self.git("init", "-q", "-b", "main")
        self.git("config", "user.name", "Ratchet Test")
        self.git("config", "user.email", "ratchet@example.invalid")
        self.write("checks.json", '{"baseline_branch": "main"}')
        self.write("product.py", "# existing\nvalue = 1\n")
        self.git("add", ".")
        self.git("commit", "-qm", "Baseline")

    def git(self, *args):
        subprocess.run(["git", "-C", self.root, *args], check=True, capture_output=True)

    def write(self, path, content):
        target = self.root / path
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_text(content)

    def test_main_compares_head_and_ignores_untracked_and_scripts(self):
        self.write("product.py", "# existing\nvalue = 2\n")
        self.write("new file.py", "# staged\n")
        self.write("ignored.py", "# untracked\n")
        self.write("scripts/check.py", "# handled by control-plane\n")
        self.git("add", "new file.py", "scripts/check.py")
        self.assertEqual([p.split(": new")[0] for p in findings(self.root)], ["new file.py:1"])
        stderr = io.StringIO()
        with contextlib.redirect_stderr(stderr):
            self.assertEqual(main(self.root), 1)
        self.assertIn("new file.py:1: new comment", stderr.getvalue())

    def test_branch_checks_committed_changes_against_configured_baseline(self):
        self.git("branch", "trunk")
        self.git("switch", "-qc", "candidate")
        self.write("checks.json", '{"baseline_branch": "trunk"}')
        self.write("product.py", "# existing\nvalue = 1  # added\n")
        self.git("add", ".")
        self.git("commit", "-qm", "Candidate")
        self.assertEqual([p.split(": new")[0] for p in findings(self.root)], ["product.py:2"])
        registry = {"schema": 1, "exceptions": [
            {"path": "product.py", "text": "value = 1  # added"},
        ]}
        self.write("comment-exceptions.json", json.dumps(registry))
        with self.assertRaisesRegex(ValueError, "category, reason"):
            list(findings(self.root))
        registry["exceptions"][0].update(category="directive", reason="Consumed by the fixture toolchain")
        self.write("other.py", "value = 1  # added\n")
        self.git("add", "other.py")
        self.git("commit", "-qm", "Other product")
        registry["exceptions"][0]["path"] = "other.py"
        self.write("comment-exceptions.json", json.dumps(registry))
        self.assertEqual([p.split(": new")[0] for p in findings(self.root)], ["product.py:2"])
        registry["exceptions"][0]["path"] = "product.py"
        registry["exceptions"][0]["text"] = " value = 1  # added "
        self.write("comment-exceptions.json", json.dumps(registry))
        self.assertEqual([p.split(": new")[0] for p in findings(self.root)], ["other.py:1"])

    def test_pinned_pr_base_takes_precedence_over_remote_ref(self):
        before = subprocess.check_output(["git", "-C", self.root, "rev-parse", "HEAD"], text=True).strip()
        self.write("product.py", "# existing\nvalue = 1  # added\n")
        self.git("add", "product.py")
        self.git("commit", "-qm", "Candidate")
        self.git("update-ref", "refs/remotes/origin/main", "HEAD")
        self.git("switch", "--detach", "-q")
        self.assertEqual(list(findings(self.root)), [])
        self.assertEqual([p.split(": new")[0] for p in findings(self.root, before)], ["product.py:2"])

    def test_detached_checkout_uses_remote_baseline_ref(self):
        self.git("update-ref", "refs/remotes/origin/main", "HEAD")
        self.git("switch", "--detach", "-q")
        self.git("branch", "-D", "main")
        self.write("product.py", "# existing\nvalue = 1  # added\n")
        self.assertEqual([p.split(": new")[0] for p in findings(self.root)], ["product.py:2"])

    def test_rename_inherits_existing_comments(self):
        self.git("mv", "product.py", "renamed.py")
        self.assertEqual(list(findings(self.root)), [])
        self.write("renamed.py", "# existing\nvalue = 1\n# added\n")
        self.assertEqual([p.split(": new")[0] for p in findings(self.root)], ["renamed.py:3"])


if __name__ == "__main__":
    unittest.main()

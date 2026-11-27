from __future__ import annotations

import ast
import io
import subprocess
import tokenize
import unittest
from pathlib import Path

PROJECT = Path(__file__).resolve().parents[2]
SHEBANG = "#!/usr/bin/env python3"
INSTEAD = "delete it; put a durable fact in a test, a name, an assertion message, or scripts/README.md"


def findings(path: str, text: str, executable: bool) -> list[str]:
    try:
        tokens = list(tokenize.generate_tokens(io.StringIO(text).readline))
        tree = ast.parse(text)
    except (SyntaxError, tokenize.TokenError) as exc:
        return [f"{path}: cannot be checked for comments: {exc}"]
    found = []
    for token in tokens:
        if token.type != tokenize.COMMENT:
            continue
        line = token.start[0]
        if line == 1 and token.start[1] == 0 and token.string == SHEBANG and executable:
            continue
        what = "shebang on a file that is not mode 100755" if line == 1 and token.string.startswith("#!") else "comment"
        found.append((line, what))
    for node in ast.walk(tree):
        if isinstance(node, ast.Expr) and isinstance(node.value, ast.Constant) and isinstance(node.value.value, str):
            found.append((node.lineno, "docstring or bare string statement"))
    return [f"{path}:{line}: {what}; {INSTEAD}" for line, what in sorted(found)]


def owned_python() -> list[tuple[str, bool]]:
    out = subprocess.run(
        ["git", "ls-files", "-s", "--", "scripts/*.py"], cwd=PROJECT, capture_output=True, text=True, check=True
    ).stdout
    files = []
    for line in out.splitlines():
        meta, path = line.split("\t", 1)
        files.append((path, meta.split()[0] == "100755"))
    return files


class OwnedPythonHasNoComments(unittest.TestCase):
    def test_every_owned_python_file_is_comment_free(self):
        files = owned_python()
        self.assertGreaterEqual(len(files), 10, f"expected the scripts/ Python files, git listed {files}")
        found = []
        for path, executable in files:
            found += findings(path, (PROJECT / path).read_text(), executable)
        self.assertEqual(found, [], "\n" + "\n".join(found))


class Rule(unittest.TestCase):
    def check(self, text, executable=False):
        return findings("f.py", text, executable)

    def test_a_hash_comment_is_found(self):
        self.assertEqual(len(self.check("x = 1  # one\n# alone\n")), 2)

    def test_module_class_and_function_docstrings_are_found(self):
        text = '"""m"""\nclass C:\n    """c"""\n    def f(self):\n        """f"""\n        return 1\n'
        self.assertEqual([f.split(":")[1] for f in self.check(text)], ["1", "3", "5"])

    def test_a_bare_string_statement_is_found(self):
        self.assertEqual(len(self.check("x = 1\n'stray'\n")), 1)

    def test_a_shebang_on_an_executable_file_is_allowed(self):
        self.assertEqual(self.check(SHEBANG + "\nx = 1\n", executable=True), [])

    def test_a_shebang_on_a_non_executable_file_is_found(self):
        self.assertIn("not mode 100755", self.check(SHEBANG + "\nx = 1\n")[0])

    def test_a_shebang_not_on_line_one_is_found(self):
        self.assertEqual(len(self.check("x = 1\n" + SHEBANG + "\n", executable=True)), 1)

    def test_another_interpreter_shebang_is_found(self):
        self.assertEqual(len(self.check("#!/usr/bin/python\nx = 1\n", executable=True)), 1)

    def test_hash_inside_strings_is_not_a_comment(self):
        text = 'a = "# no"\nb = f"{a} # no"\nc = r"^\\s*#.*$"\nd = """\n# no\n"""\n'
        self.assertEqual(self.check(text), [])

    def test_strings_used_as_values_are_not_found(self):
        text = 'import argparse\nUSAGE = """u"""\nargparse.ArgumentParser(description="d")\nprint("p")\n'
        self.assertEqual(self.check(text), [])

    def test_a_file_that_does_not_parse_is_found(self):
        self.assertIn("cannot be checked", self.check("def (:\n")[0])


if __name__ == "__main__":
    unittest.main()

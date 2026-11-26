import ast
from collections import Counter
from difflib import SequenceMatcher
import io
import json
import os
from pathlib import Path
import subprocess
import sys
import tokenize


ROOT = Path(__file__).resolve().parents[2]


def git(root, *args):
    return subprocess.check_output(["git", "-C", root, *args])


def changed_paths(root, baseline):
    fields = iter(git(
        root, "diff", "--name-status", "-z", "--find-renames",
        "--diff-filter=AMRT", baseline, "--",
    ).split(b"\0")[:-1])
    for status in fields:
        path = os.fsdecode(next(fields))
        if status.startswith(b"R"):
            yield os.fsdecode(next(fields)), path
        else:
            yield path, None if status == b"A" else path


def source_text(source):
    encoding, _ = tokenize.detect_encoding(io.BytesIO(source).readline)
    return source.decode(encoding).replace("\r\n", "\n").replace("\r", "\n")


def comments(text):
    tokens = tokenize.generate_tokens(io.StringIO(text).readline)
    return {
        token.start[0] - 1: token.string.strip()
        for token in tokens if token.type == tokenize.COMMENT
    }


def docstrings(text):
    owners, found = Counter(), {}

    def visit(node, parent):
        owner = parent
        if isinstance(node, (ast.ClassDef, ast.FunctionDef, ast.AsyncFunctionDef)):
            key = (*parent, type(node).__name__, node.name)
            owner = (*key, owners[key])
            owners[key] += 1
        if isinstance(node, (ast.Module, ast.ClassDef, ast.FunctionDef, ast.AsyncFunctionDef)):
            if ast.get_docstring(node, clean=False) is not None:
                literal = node.body[0].value
                fragments = ast.get_source_segment(text, literal).split("\n")
                found[owner] = tuple(
                    (line, part.strip())
                    for line, part in enumerate(fragments, literal.lineno - 1)
                    if part.strip()
                )
        for child in ast.iter_child_nodes(node):
            visit(child, owner)

    visit(ast.parse(text), ())
    return found


def added_lines(before, after):
    original = [text for _, text in before]
    candidate = [text for _, text in after]
    for tag, _, _, start, end in SequenceMatcher(None, original, candidate, autojunk=False).get_opcodes():
        if tag in {"insert", "replace"}:
            yield from (line for line, _ in after[start:end])


def new_annotations(before, after, allowed):
    old_text, new_text = source_text(before), source_text(after)
    old_lines, new_lines_of_source = old_text.split("\n"), new_text.split("\n")
    old_comments, new_comments = comments(old_text), comments(new_text)
    changes = []
    matcher = SequenceMatcher(None, old_lines, new_lines_of_source, autojunk=False)
    for tag, old_start, old_end, new_start, new_end in matcher.get_opcodes():
        if tag == "equal":
            changes.extend(
                (new, "comment")
                for old, new in zip(range(old_start, old_end), range(new_start, new_end))
                if new in new_comments and new_comments[new] != old_comments.get(old)
            )
        elif tag in {"insert", "replace"}:
            previous = [(line, old_comments[line]) for line in range(old_start, old_end) if line in old_comments]
            current = [(line, new_comments[line]) for line in range(new_start, new_end) if line in new_comments]
            changes.extend((line, "comment") for line in added_lines(previous, current))
    old_docs = docstrings(old_text)
    for owner, fragments in docstrings(new_text).items():
        changes.extend((line, "docstring") for line in added_lines(old_docs.get(owner, ()), fragments))
    for line, kind in sorted(changes):
        if new_lines_of_source[line].strip() not in allowed:
            yield line + 1, kind


def baseline_ref(root, branch, pr_base):
    if pr_base:
        return git(root, "rev-parse", "--verify", "--end-of-options", f"{pr_base}^{{commit}}").decode().strip()
    if git(root, "rev-parse", "--abbrev-ref", "HEAD").decode().strip() == branch:
        return "HEAD"
    refs = (f"refs/remotes/origin/{branch}", f"refs/heads/{branch}")
    for ref in refs:
        result = subprocess.run(
            ["git", "-C", root, "rev-parse", "--verify", "--quiet", f"{ref}^{{commit}}"],
            capture_output=True, text=True,
        )
        if result.returncode == 0:
            return result.stdout.strip()
    raise ValueError(f"Cannot find the comment ratchet baseline branch {branch!r}")


def exception_entries(root):
    registry = root / "comment-exceptions.json"
    if not registry.is_file():
        return []
    if str(ROOT) not in sys.path:
        sys.path.insert(0, str(ROOT))
    from scripts.repo import exception_problems

    problems = exception_problems(root, registry)
    if problems:
        raise ValueError(f"{registry.name}: " + "\n".join(problems))
    return json.loads(registry.read_text())["exceptions"]


def findings(root, pr_base=None):
    config = json.loads((root / "checks.json").read_text())
    baseline = baseline_ref(root, config.get("baseline_branch", "main"), pr_base)
    exceptions = exception_entries(root)
    for path, previous in changed_paths(root, baseline):
        candidate = root / path
        if not path.endswith(".py") or path.startswith("scripts/") or candidate.is_symlink() or not candidate.is_file():
            continue
        before = b""
        if previous and previous.endswith(".py"):
            mode = git(root, "--literal-pathspecs", "ls-tree", baseline, "--", previous).split()[0]
            if mode in (b"100644", b"100755"):
                before = git(root, "show", f"{baseline}:{previous}")
        allowed = {entry["text"].strip() for entry in exceptions if entry["path"] == path}
        try:
            for lineno, kind in new_annotations(before, candidate.read_bytes(), allowed):
                yield f"{path}:{lineno}: new {kind}; use code, a test, or an approved comment exception"
        except (SyntaxError, tokenize.TokenError, UnicodeError) as exc:
            raise ValueError(f"{path}: unable to inspect Python source: {exc}") from exc


def main(root=ROOT, pr_base=None):
    try:
        problems = list(findings(root, pr_base))
    except (OSError, ValueError, subprocess.CalledProcessError) as exc:
        print(exc, file=sys.stderr)
        return 1
    for problem in problems:
        print(problem, file=sys.stderr)
    return int(bool(problems))


if __name__ == "__main__":
    sys.exit(main(pr_base=os.environ.get("PR_BASE_SHA")))

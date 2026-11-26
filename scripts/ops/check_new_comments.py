import ast
from collections import Counter, defaultdict
from difflib import SequenceMatcher
import io
from itertools import zip_longest
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import tokenize


ROOT = Path(__file__).resolve().parents[2]
STRING_OPENING = re.compile(r"""[rRuU]?('''|"{3}|'|")""")
CODE_START = re.compile(r"\w+(?:\.\w+)*")


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


def annotated_lines(text):
    tokens = tokenize.generate_tokens(io.StringIO(text).readline)
    comments = {
        token.start[0] - 1: (token.line[:token.start[1]].strip(), token.string.strip())
        for token in tokens if token.type == tokenize.COMMENT
    }
    return [comments.get(line, (code.strip(), None)) for line, code in enumerate(text.split("\n"))]


def string_fragments(text, literal):
    source = ast.get_source_segment(text, literal)
    tokens = tokenize.generate_tokens(io.StringIO(f"({source}\n)").readline)
    fragments = defaultdict(list)
    for token in tokens:
        if token.type != tokenize.STRING:
            continue
        opening = STRING_OPENING.match(token.string)
        contents = token.string[opening.end():].removesuffix(opening[1])
        for line, part in enumerate(contents.split("\n"), literal.lineno + token.start[0] - 2):
            if part.strip():
                fragments[line].append(part.strip())
    return tuple((line, " ".join(parts)) for line, parts in fragments.items()) or ((literal.lineno - 1, ""),)


def docstrings(text):
    owners, found = Counter(), defaultdict(list)

    def visit(node, parent):
        owner = parent
        if isinstance(node, (ast.ClassDef, ast.FunctionDef, ast.AsyncFunctionDef)):
            key = (*parent, type(node).__name__, node.name)
            owner = (*key, owners[key])
            owners[key] += 1
        if isinstance(node, ast.Expr) and isinstance(node.value, ast.Constant) and isinstance(node.value.value, str):
            found[owner].append(string_fragments(text, node.value))
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


def added_docstring_lines(before, after):
    old_strings = [tuple(text for _, text in fragments) for fragments in before]
    new_strings = [tuple(text for _, text in fragments) for fragments in after]
    matcher = SequenceMatcher(None, old_strings, new_strings, autojunk=False)
    for tag, old_start, old_end, new_start, new_end in matcher.get_opcodes():
        if tag in {"insert", "replace"}:
            for old, new in zip_longest(before[old_start:old_end], after[new_start:new_end], fillvalue=()):
                yield from added_lines(old, new)


def compare_comments(before, after, new_start):
    for offset, ((_, previous), (_, current)) in enumerate(zip_longest(before, after, fillvalue=("", None))):
        if current is not None and current != previous:
            yield new_start + offset


def code_anchor(code, comment):
    start = CODE_START.match(code)
    return start[0] if start else (code, comment)


def added_comment_lines(before, after):
    original = [(code, comment if not code else None) for code, comment in before]
    candidate = [(code, comment if not code else None) for code, comment in after]
    matcher = SequenceMatcher(None, original, candidate, autojunk=False)
    for tag, old_start, old_end, new_start, new_end in matcher.get_opcodes():
        previous, current = before[old_start:old_end], after[new_start:new_end]
        if tag == "equal":
            yield from compare_comments(previous, current, new_start)
        elif tag == "insert":
            yield from compare_comments((), current, new_start)
        elif tag == "replace":
            anchors = SequenceMatcher(
                None, [code_anchor(*line) for line in previous],
                [code_anchor(*line) for line in current], autojunk=False,
            )
            for match, left, right, start, end in anchors.get_opcodes():
                retained = previous[left:right] if match == "equal" else ()
                yield from compare_comments(retained, current[start:end], new_start + start)


def new_annotations(before, after, allowed):
    old_text, new_text = source_text(before), source_text(after)
    new_lines_of_source = new_text.split("\n")
    changes = [(line, "comment") for line in added_comment_lines(annotated_lines(old_text), annotated_lines(new_text))]
    old_docs = docstrings(old_text)
    for owner, fragments in docstrings(new_text).items():
        changes.extend((line, "docstring") for line in added_docstring_lines(old_docs.get(owner, ()), fragments))
    for line, kind in sorted(changes):
        if new_lines_of_source[line].strip() not in allowed:
            yield line + 1, kind


def resolve_commit(root, ref):
    result = subprocess.run(
        ["git", "-C", root, "rev-parse", "--verify", "--quiet", "--end-of-options", f"{ref}^{{commit}}"],
        capture_output=True, text=True,
    )
    return result.stdout.strip() if result.returncode == 0 else None


def comparison_commit(root, commit):
    result = subprocess.run(
        ["git", "-C", root, "merge-base", commit, "HEAD"],
        capture_output=True, text=True,
    )
    return result.stdout.strip() if result.returncode == 0 else commit


def push_before():
    before = os.environ.get("GITHUB_EVENT_BEFORE")
    payload = os.environ.get("GITHUB_EVENT_PATH")
    if not before and payload:
        try:
            event = json.loads(Path(payload).read_bytes())
        except (OSError, ValueError):
            return None
        if isinstance(event, dict):
            before = event.get("before")
    if isinstance(before, str) and re.fullmatch(r"[0-9a-fA-F]{40}|[0-9a-fA-F]{64}", before) and before.strip("0"):
        return before
    return None


def baseline_ref(root, branch, pr_base):
    if pr_base:
        commit = git(root, "rev-parse", "--verify", "--end-of-options", f"{pr_base}^{{commit}}").decode().strip()
        return comparison_commit(root, commit)
    on_baseline_branch = git(root, "rev-parse", "--abbrev-ref", "HEAD").decode().strip() == branch
    if os.environ.get("GITHUB_EVENT_NAME") == "push":
        before = push_before()
        commit = resolve_commit(root, before) if before else None
        return commit or resolve_commit(root, "HEAD~1") or "HEAD"
    if on_baseline_branch:
        if not git(root, "--no-optional-locks", "status", "--porcelain"):
            return resolve_commit(root, "HEAD~1") or "HEAD"
        return "HEAD"
    refs = (f"refs/remotes/origin/{branch}", f"refs/heads/{branch}")
    for ref in refs:
        commit = resolve_commit(root, ref)
        if commit:
            return comparison_commit(root, commit)
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

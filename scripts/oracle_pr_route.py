"""Classify an exact GitHub pull request candidate for the Oracle CI job."""

from __future__ import annotations

import argparse
import re
import sys
from pathlib import Path

from repo import GitError, ID_RE, git, parse_pr_change_ids


SHA = re.compile(r"[0-9a-f]{40}")
READMES = {"README.md", "README.ko.md"}
REGULAR_CHANGES = {
    (b"A", b"000000", b"100644"),
    (b"M", b"100644", b"100644"),
    (b"D", b"100644", b"000000"),
}


def is_document(path: str) -> bool:
    return path in READMES or (
        path.startswith("docs/") and path.endswith(".md") and path.isprintable()
    )


def documented_paths(root: Path, start: str, end: str, packet: str) -> list[str]:
    raw = git(
        root, "diff", "--raw", "-z", "--no-renames", "--no-ext-diff",
        "--ignore-submodules=none", "--no-relative", start, end, "--", binary=True,
    )
    fields = raw.split(b"\0")
    if fields[-1] or len(fields) % 2 != 1:
        raise ValueError("unexpected Git diff format")

    packet_files = {f"changes/{packet}/change.json", f"changes/{packet}/plan.md"}
    paths = []
    for header, filename in zip(fields[0:-1:2], fields[1:-1:2]):
        old_mode, new_mode, _, _, status = header[1:].split()
        path = filename.decode()
        if not header.startswith(b":") or (status, old_mode, new_mode) not in REGULAR_CHANGES:
            raise ValueError(f"file mode or change type requires OCI: {ascii(path)}")
        if not (is_document(path) or path in packet_files):
            raise ValueError(f"path requires OCI: {ascii(path)}")
        paths.append(path)

    if not any(is_document(path) for path in paths):
        raise ValueError("comparison contains no documentation change")
    return paths


def classify(root: Path, event: str, base: str, head: str, candidate: str, body: Path) -> tuple[str, str]:
    if event != "pull_request":
        return "full", "event is not a pull request"
    try:
        if not all(SHA.fullmatch(sha) for sha in (base, head, candidate)) or base == head:
            raise ValueError("missing or invalid event commits")
        if git(root, "rev-parse", "HEAD").strip() != candidate:
            raise ValueError("checkout differs from the event candidate")
        if git(root, "rev-list", "--parents", "-1", candidate).split() != [candidate, base, head]:
            raise ValueError("candidate parents differ from the event base and head")
        merge_base = git(root, "merge-base", "--all", base, head).strip()
        if not SHA.fullmatch(merge_base):
            raise ValueError("comparison has no unique merge base")
        ids = parse_pr_change_ids(body.read_text(encoding="utf-8"))
        if len(ids) != 1 or not ID_RE.fullmatch(ids[0]):
            raise ValueError("PR body does not identify exactly one valid change")

        paths = set(documented_paths(root, merge_base, head, ids[0]))
        paths.update(documented_paths(root, base, candidate, ids[0]))
    except (GitError, OSError):
        return "full", "unable to read the Git comparison or PR body"
    except ValueError as exc:
        return "full", str(exc)

    return "docs-only", ", ".join(ascii(path) for path in sorted(paths))


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    for field in ("event", "base", "head", "candidate"):
        parser.add_argument(f"--{field}", required=True)
    parser.add_argument("--pr-body", required=True, type=Path)
    args = parser.parse_args()
    mode, reason = classify(Path.cwd(), args.event, args.base, args.head, args.candidate, args.pr_body)
    print(f"Oracle routing: {mode}. {reason}", file=sys.stderr)
    print(f"Oracle routing commits: base={ascii(args.base)} head={ascii(args.head)} "
          f"candidate={ascii(args.candidate)}", file=sys.stderr)
    print(mode)


if __name__ == "__main__":
    main()

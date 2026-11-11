#!/usr/bin/env python3
"""Run the control-plane tests with native unittest, and fail a run that did not finish.

`python3 -m unittest discover` exits 0 when a test ends its process early with `os._exit(0)`.
This wrapper runs discovery in a child process and passes only if the child exited 0 and printed
unittest's final summary: `Ran N test(s)` with N > 0, then a result line that starts with `OK`.
Arguments it does not know go to unittest, for example `-v` or `-k NAME`.
"""

from __future__ import annotations

import argparse
import re
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
RAN = re.compile(r"^Ran (\d+) tests? in ")


def verdict(returncode: int, stderr: str) -> str | None:
    """Why the run did not pass, or None when it passed."""
    lines = [line.strip() for line in stderr.splitlines() if line.strip()]
    ran = [i for i, line in enumerate(lines) if RAN.match(line)]
    if not ran:
        return "unittest printed no final summary: the run ended early or never started"
    count = int(RAN.match(lines[ran[-1]]).group(1))
    result = lines[ran[-1] + 1] if ran[-1] + 1 < len(lines) else ""
    if count == 0:
        return "no tests ran"
    if returncode != 0 or not result.startswith("OK"):
        return f"unittest reported {result or 'no result'} (exit {returncode})"
    return None


def main(argv=None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--start", default="scripts/tests", help="discovery start directory (default: scripts/tests)")
    parser.add_argument("--top", default=".", help="top-level directory for imports (default: the repository root)")
    args, extra = parser.parse_known_args(argv)
    command = [sys.executable, "-m", "unittest", "discover", "-s", args.start, "-t", args.top, *extra]
    proc = subprocess.Popen(command, cwd=ROOT, stderr=subprocess.PIPE, text=True, errors="replace")
    captured = []
    for line in proc.stderr:
        sys.stderr.write(line)
        captured.append(line)
    returncode = proc.wait()
    problem = verdict(returncode, "".join(captured))
    if problem:
        print(f"FAILED: {problem}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())

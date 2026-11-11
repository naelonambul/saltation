"""Throwaway git repositories for control-plane fixtures."""

from __future__ import annotations

import atexit
import hashlib
import json
import os
import shutil
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

REPO_PY = Path(__file__).resolve().parents[1] / "repo.py"
UNESTABLISHED = "<!-- sdlc:baseline-unestablished -->\n"
GIT_ENV = {
    "GIT_AUTHOR_NAME": "fixture",
    "GIT_AUTHOR_EMAIL": "fixture@example.invalid",
    "GIT_COMMITTER_NAME": "fixture",
    "GIT_COMMITTER_EMAIL": "fixture@example.invalid",
    "GIT_CONFIG_GLOBAL": os.devnull,
    "GIT_CONFIG_NOSYSTEM": "1",
    # `git commit` would start detached auto maintenance, which creates and removes
    # `objects/maintenance.lock` after commit returns and races copytree and cleanup.
    "GIT_CONFIG_COUNT": "1",
    "GIT_CONFIG_KEY_0": "maintenance.auto",
    "GIT_CONFIG_VALUE_0": "false",
}
# Snapshot before any test patches GIT_ENV, so the shared template never sees a patch.
DEFAULT_GIT_ENV = dict(GIT_ENV)
_TEMPLATE: Path | None = None


def digest(data: str | bytes) -> str:
    if isinstance(data, str):
        data = data.encode()
    return "sha256:" + hashlib.sha256(data).hexdigest()


def _template() -> Path:
    """The initial repository, built once per process under DEFAULT_GIT_ENV and copied per test."""
    global _TEMPLATE
    if _TEMPLATE is None:
        tmp = tempfile.TemporaryDirectory()
        atexit.register(tmp.cleanup)
        root = Path(tmp.name) / "repo"
        root.mkdir()
        env = {**os.environ, **DEFAULT_GIT_ENV}

        def git(*args):
            subprocess.run(["git", *args], cwd=root, env=env, check=True, capture_output=True)

        git("init", "-q", "-b", "main")
        # Every test repository is a copy, so Git calls made without GIT_ENV stay covered too.
        git("config", "maintenance.auto", "false")
        (root / "AGENTS.md").write_text("# Repository Agent Instructions\n")
        (root / "intent.md").write_text(UNESTABLISHED + "# Intent\n")
        (root / "spec.md").write_text(UNESTABLISHED + "# Specification\n")
        git("add", "-A")
        git("commit", "-q", "--allow-empty", "-m", "initial")
        _TEMPLATE = root
    return _TEMPLATE


# -- running control-plane scripts ---------------------------------------------
#
# Tests run repo.py and guard.py as real subprocesses, the way a user or CI does.


def run_subprocess(script: Path, args, stdin, cwd) -> subprocess.CompletedProcess:
    return subprocess.run(
        [sys.executable, str(script), *args],
        cwd=cwd,
        env={**os.environ, **GIT_ENV},
        input=stdin,
        capture_output=True,
        text=True,
    )


class RepoCase(unittest.TestCase):
    """Each test gets a fresh repository on `main` with an initial commit."""

    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory()
        self.root = Path(self._tmp.name) / "repo"
        shutil.copytree(_template(), self.root, symlinks=True)

    def tearDown(self):
        self._tmp.cleanup()

    # -- git / files ---------------------------------------------------------

    def git(self, *args, cwd=None) -> str:
        env = {**os.environ, **GIT_ENV}
        proc = subprocess.run(["git", *args], cwd=cwd or self.root, env=env, capture_output=True, text=True)
        if proc.returncode != 0:
            raise AssertionError(f"git {args} failed: {proc.stderr}")
        return proc.stdout.strip()

    def write(self, rel: str, text: str):
        path = self.root / rel
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(text)

    def read(self, rel: str) -> str:
        return (self.root / rel).read_text()

    def commit(self, message: str) -> str:
        self.git("add", "-A")
        self.git("commit", "-q", "--allow-empty", "-m", message)
        return self.git("rev-parse", "HEAD")

    # -- change packets --------------------------------------------------------

    def packet(self, cid, kind="implementation", scope=(), baseline=None, approvals=(), artifacts=None, **extra):
        artifacts = artifacts if artifacts is not None else {"plan.md": f"# Plan for {cid}\n"}
        for name, text in artifacts.items():
            self.write(f"changes/{cid}/{name}", text)
        if baseline is None and kind not in ("product-init", "repository"):
            baseline = {n: digest(self.read(n)) for n in ("intent.md", "spec.md")}
        data = {
            "schema": 1,
            "id": cid,
            "kind": kind,
            "base": {"ref": "main", "commit": ""},
            "baseline": baseline or {},
            "write_scope": list(scope),
            "approvals": list(approvals),
            **extra,
        }
        self.write(f"changes/{cid}/change.json", json.dumps(data, indent=2) + "\n")

    def set_scope(self, cid, scope):
        path = self.root / "changes" / cid / "change.json"
        data = json.loads(path.read_text())
        data["write_scope"] = list(scope)
        path.write_text(json.dumps(data, indent=2) + "\n")

    def approve(self, cid, *names, at="2026-09-23T10:00:{:02d}Z", by="owner", bind_scope=True):
        path = self.root / "changes" / cid / "change.json"
        data = json.loads(path.read_text())
        for i, name in enumerate(names):
            claim = {"artifact": name, "sha256": digest(self.read(f"changes/{cid}/{name}")), "by": by, "at": at.format(i)}
            if name == "plan.md" and bind_scope:
                claim["write_scope_sha256"] = digest(json.dumps(sorted(data["write_scope"]), separators=(",", ":")))
            data["approvals"].append(claim)
        path.write_text(json.dumps(data, indent=2) + "\n")

    def establish_baseline(self):
        """Merge a product-init change on `main`, so root intent/spec are established and its packet is merged."""
        self.packet(
            "init",
            kind="product-init",
            artifacts={"intent.md": "# Intent\nreal\n", "spec.md": "# Spec\nreal\n", "plan.md": "# Plan\n"},
        )
        self.approve("init", "intent.md", "spec.md", "plan.md")
        self.write("intent.md", "# Intent\nreal\n")
        self.write("spec.md", "# Spec\nreal\n")
        return self.commit("merge init")

    # -- running repo.py -------------------------------------------------------

    def run_script(self, script: Path, args, stdin=None, cwd=None):
        return run_subprocess(script, args, stdin, cwd or self.root)

    def run_repo(self, *args, cwd=None):
        return self.run_script(REPO_PY, args, cwd=cwd)

    def run_repo_subprocess(self, *args, cwd=None):
        return run_subprocess(REPO_PY, args, None, cwd or self.root)

    def status(self, *args, cwd=None):
        proc = self.run_repo("status", "--json", "--ci", *args, cwd=cwd)
        try:
            result = json.loads(proc.stdout)
        except json.JSONDecodeError:
            raise AssertionError(f"no JSON from repo.py (exit {proc.returncode}): {proc.stdout}{proc.stderr}")
        result["exit"] = proc.returncode
        result["by_id"] = {c["id"]: c for c in result["changes"]}
        return result

    def codes(self, result, cid):
        return {r["code"] for r in result["by_id"][cid]["reasons"]}

    def assertFailed(self, result, fragment):
        self.assertEqual(result["exit"], 1, result)
        self.assertTrue(any(fragment in f for f in result["failures"]), result["failures"])

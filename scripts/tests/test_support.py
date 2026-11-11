"""Tests for the test harness itself: the subprocess contract and fixture isolation."""

from __future__ import annotations

import json
import os
import subprocess
import unittest
from pathlib import Path
from unittest import mock

from scripts.tests import support
from scripts.tests.support import REPO_PY, RepoCase

GUARD = REPO_PY.parent / "hooks" / "guard.py"
PROJECT = REPO_PY.parents[1]
SETTINGS = PROJECT / ".claude" / "settings.json"


class SubprocessContract(RepoCase):
    """Both entry points keep working as real processes."""

    def real(self, script, *args, stdin=None):
        return support.run_subprocess(script, args, stdin, self.root)

    def test_repo_status_json(self):
        proc = self.real(REPO_PY, "status", "--json", "--ci")
        self.assertEqual(proc.returncode, 0, proc.stderr)
        self.assertTrue(json.loads(proc.stdout)["ok"])

    def test_repo_refusal(self):
        proc = self.real(REPO_PY, "approve", "nope", "plan.md", "--by", "owner")
        self.assertEqual(proc.returncode, 1)
        self.assertIn("not found", proc.stderr)

    def test_repo_unknown_subcommand(self):
        proc = self.real(REPO_PY, "frobnicate")
        self.assertEqual(proc.returncode, 2)
        self.assertIn("usage: repo.py", proc.stderr)

    def test_guard_usage_names_the_script(self):
        proc = self.real(GUARD)
        self.assertEqual(proc.returncode, 2)
        self.assertIn("usage: guard.py", proc.stderr)

    def test_guard_path_deny_and_allow(self):
        proc = self.real(GUARD, "path", "intent.md", "--proposed", "-", stdin="x")
        self.assertEqual((proc.returncode, json.loads(proc.stdout)["decision"]), (2, "deny"))
        proc = self.real(GUARD, "path", "src/app.py", "--proposed", "-", stdin="print(1)\n")
        self.assertEqual((proc.returncode, json.loads(proc.stdout)["decision"]), (0, "allow"))

    def test_guard_command_ask(self):
        proc = self.real(GUARD, "command", "git push --force")
        self.assertEqual((proc.returncode, json.loads(proc.stdout)["decision"]), (3, "ask"))

    def test_guard_claude_bash_payload(self):
        payload = json.dumps({"tool_name": "Bash", "tool_input": {"command": "git push --force"}, "cwd": str(self.root)})
        proc = self.real(GUARD, "claude", stdin=payload)
        self.assertEqual(proc.returncode, 0)
        self.assertEqual(json.loads(proc.stdout)["hookSpecificOutput"]["permissionDecision"], "ask")

    def test_settings_hook_command_runs_the_guard(self):
        """Run each hook command from .claude/settings.json through a shell, as Claude Code does."""
        data = json.loads(SETTINGS.read_text())
        commands = {h["command"] for entry in data["hooks"]["PreToolUse"] for h in entry["hooks"]}
        payload = json.dumps(
            {"tool_name": "Edit", "tool_input": {"file_path": str(self.root / "intent.md"), "old_string": "# Intent", "new_string": "# Hacked"}, "cwd": str(self.root)}
        )
        for command in commands:
            with self.subTest(command=command):
                proc = subprocess.run(
                    command,
                    shell=True,
                    cwd=self.root,
                    env={**os.environ, **support.GIT_ENV, "CLAUDE_PROJECT_DIR": str(PROJECT)},
                    input=payload,
                    capture_output=True,
                    text=True,
                )
                self.assertEqual(proc.returncode, 2, proc.stderr)
                self.assertIn("root-baseline", proc.stderr)


class NoAutoMaintenance(RepoCase):
    """Fixture Git commands never start detached auto maintenance, which races copies and cleanup."""

    def test_commit_starts_no_maintenance(self):
        trace = Path(self._tmp.name) / "trace"
        self.write("a.txt", "a\n")
        with mock.patch.dict(support.GIT_ENV, {"GIT_TRACE": str(trace)}):
            self.commit("traced")
        self.assertIn("commit", trace.read_text())
        self.assertNotIn("maintenance run", trace.read_text())

    def test_template_config_disables_maintenance(self):
        env = {**os.environ, "GIT_CONFIG_GLOBAL": os.devnull, "GIT_CONFIG_NOSYSTEM": "1"}
        out = subprocess.run(["git", "config", "--get", "maintenance.auto"], cwd=self.root, env=env, capture_output=True, text=True)
        self.assertEqual(out.stdout.strip(), "false")


if __name__ == "__main__":
    unittest.main()

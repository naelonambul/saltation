"""Tests for the agent-neutral hook guard and its Claude Code adapter."""

from __future__ import annotations

import json
import os
import tempfile
import unittest
from pathlib import Path

from scripts.tests.support import RepoCase

GUARD = Path(__file__).resolve().parents[1] / "hooks" / "guard.py"
SETTINGS = Path(__file__).resolve().parents[2] / ".claude" / "settings.json"


class GuardCase(RepoCase):
    def run_guard(self, *args, stdin=None, cwd=None):
        return self.run_script(GUARD, args, stdin=stdin, cwd=cwd)

    def path(self, rel, proposed=None):
        args = ["path", rel]
        stdin = None
        if proposed is not None:
            args += ["--proposed", "-"]
            stdin = proposed
        proc = self.run_guard(*args, stdin=stdin)
        return proc.returncode, json.loads(proc.stdout)

    def command(self, text):
        proc = self.run_guard("command", text)
        return proc.returncode, json.loads(proc.stdout)

    def claude(self, payload):
        stdin = payload if isinstance(payload, str) else json.dumps(payload)
        return self.run_guard("claude", stdin=stdin)

    def change_json(self, cid, **override):
        data = json.loads(self.read(f"changes/{cid}/change.json"))
        data.update(override)
        return json.dumps(data, indent=2) + "\n"


class PathRuleTests(GuardCase):
    def test_root_intent_and_spec_denied(self):
        for name in ("intent.md", "spec.md"):
            code, out = self.path(name, "# changed\n")
            self.assertEqual((code, out["decision"]), (2, "deny"), name)
            self.assertIn("root-baseline", out["reason"])

    def test_root_intent_denied_by_absolute_path(self):
        code, out = self.path(str(self.root / "intent.md"), "x")
        self.assertEqual((code, out["decision"]), (2, "deny"))

    def test_change_local_intent_allowed(self):
        code, out = self.path("changes/x/intent.md", "# local\n")
        self.assertEqual((code, out["decision"]), (0, "allow"))

    def test_approvals_changed_denied(self):
        self.packet("x")
        forged = self.change_json("x", approvals=[{"artifact": "plan.md", "sha256": "sha256:0", "by": "me", "at": "now"}])
        code, out = self.path("changes/x/change.json", forged)
        self.assertEqual((code, out["decision"]), (2, "deny"))
        self.assertIn("change-approvals", out["reason"])

    def test_approvals_removed_denied(self):
        self.packet("x")
        self.write("changes/x/plan.md", "# Plan\n")
        self.approve("x", "plan.md")
        code, out = self.path("changes/x/change.json", self.change_json("x", approvals=[]))
        self.assertEqual((code, out["decision"]), (2, "deny"))

    def test_write_scope_only_change_allowed(self):
        self.packet("x")
        code, out = self.path("changes/x/change.json", self.change_json("x", write_scope=["src/**"]))
        self.assertEqual((code, out["decision"]), (0, "allow"))

    def test_write_scope_change_after_plan_approval_denied(self):
        self.packet("imp", kind="repository", scope=["src/**", "docs/**"])
        self.approve("imp", "plan.md")
        code, out = self.path("changes/imp/change.json", self.change_json("imp", write_scope=["src/**", "docs/**", "**"]))
        self.assertEqual((code, out["decision"]), (2, "deny"))
        self.assertIn("change-scope", out["reason"])

    def test_write_scope_reorder_after_plan_approval_allowed(self):
        self.packet("imp", kind="repository", scope=["src/**", "docs/**"])
        self.approve("imp", "plan.md")
        code, out = self.path("changes/imp/change.json", self.change_json("imp", write_scope=["docs/**", "src/**"]))
        self.assertEqual((code, out["decision"]), (0, "allow"))

    def test_existing_approvals_kept_allowed(self):
        self.packet("x")
        self.approve("x", "plan.md")
        code, out = self.path("changes/x/change.json", self.change_json("x", title="renamed"))
        self.assertEqual((code, out["decision"]), (0, "allow"))

    def test_no_proposed_content_denied(self):
        self.packet("x")
        code, out = self.path("changes/x/change.json")
        self.assertEqual((code, out["decision"]), (2, "deny"))

    def test_unparseable_proposed_denied(self):
        self.packet("x")
        code, out = self.path("changes/x/change.json", "{not json")
        self.assertEqual((code, out["decision"]), (2, "deny"))

    def test_new_change_json_without_approvals_allowed(self):
        code, out = self.path("changes/new/change.json", json.dumps({"schema": 1, "approvals": []}))
        self.assertEqual((code, out["decision"]), (0, "allow"))

    def test_proposed_from_file(self):
        self.packet("x")
        target = self.root.parent / "proposed.json"
        target.write_text(self.change_json("x", write_scope=["b"]))
        proc = self.run_guard("path", "changes/x/change.json", "--proposed", str(target))
        self.assertEqual(proc.returncode, 0, proc.stdout)

    def test_merged_packet_denied(self):
        self.packet("x")
        self.commit("merged x")
        self.git("switch", "-q", "-c", "followup")
        code, out = self.path("changes/x/plan.md", "# edited\n")
        self.assertEqual((code, out["decision"]), (2, "deny"))
        self.assertIn("merged-packet", out["reason"])

    def test_open_packet_allowed_until_it_merges(self):
        self.git("switch", "-q", "-c", "change/x")
        self.packet("x")
        self.commit("open x on its branch")
        code, out = self.path("changes/x/plan.md", "# edited\n")
        self.assertEqual((code, out["decision"]), (0, "allow"))

    def test_template_packet_is_never_merged(self):
        self.write("changes/_template/change.json", "{}\n")
        self.commit("template")
        code, out = self.path("changes/_template/plan.md", "# edited\n")
        self.assertEqual((code, out["decision"]), (0, "allow"))

    def test_merged_uses_checks_json_baseline_branch(self):
        self.write("checks.json", json.dumps({"baseline_branch": "trunk"}))
        self.packet("x")
        self.commit("x on main")
        self.git("switch", "-q", "-c", "followup")
        code, out = self.path("changes/x/plan.md", "# edited\n")
        self.assertEqual((code, out["decision"]), (0, "allow"))
        self.git("branch", "trunk", "main")
        code, out = self.path("changes/x/plan.md", "# edited\n")
        self.assertEqual((code, out["decision"]), (2, "deny"))

    def test_path_outside_repository_allowed(self):
        code, out = self.path("/etc/hosts", "x")
        self.assertEqual((code, out["decision"]), (0, "allow"))
        code, out = self.path("../intent.md", "x")
        self.assertEqual((code, out["decision"]), (0, "allow"))

    def test_ordinary_source_file_allowed(self):
        code, out = self.path("src/app.py", "print(1)\n")
        self.assertEqual((code, out["decision"]), (0, "allow"))

    def test_no_repository_allows_with_note(self):
        with tempfile.TemporaryDirectory() as tmp:
            proc = self.run_guard("path", "intent.md", "--proposed", "-", stdin="x", cwd=tmp)
        self.assertEqual(proc.returncode, 0)
        self.assertEqual(json.loads(proc.stdout)["decision"], "allow")
        self.assertTrue(proc.stderr.strip())


class CommandRuleTests(GuardCase):
    ASK = [
        "git push --force origin x",
        "git push -f origin x",
        "git push origin x --force-with-lease",
        "git push origin +main",
        "git reset --hard HEAD~1",
        "git clean -fd",
        "git clean -fdx",
        "git branch -D old",
        "git rebase main",
        "git commit --amend --no-edit",
        "git filter-branch --all",
        "git push --delete origin old",
        "git push origin :old",
        "git -C sub push --force",
        "cd sub && git reset --hard",
        "python3 scripts/repo.py approve x plan.md --by me",
        "echo '{}' > changes/x/change.json",
        "sed -i 's/a/b/' changes/x/change.json",
        "echo x | tee changes/x/change.json",
        "mv /tmp/c.json changes/x/change.json",
        "cp /tmp/c.json changes/x/change.json",
        "rm changes/x/change.json",
        "python3 -c 'open(\"changes/x/change.json\", \"w\")'",
        "git status\ngit reset --hard",
        "git status\ngit push -f origin x",
        "bash -c 'git reset --hard'",
        "sh -c \"git push -f origin x\"",
        "python3 scripts/repo.py  approve x plan.md --by me",
        "jq . /tmp/in.json > changes/x/change.json",
    ]
    ALLOW = [
        "git status",
        "git push origin HEAD",
        "git push -u origin feature",
        "git reset --soft HEAD~1",
        "git clean -n",
        "git branch -d merged",
        "git commit -m 'git push --force and rebase'",
        "git commit -m 'docs: repo.py approve'",
        "grep 'repo.py approve' README.md",
        "cat changes/x/change.json",
        "cat changes/x/change.json 2>/dev/null",
        "jq . changes/x/change.json > /dev/null",
        "git diff changes/x/change.json > /tmp/p",
        "cp changes/x/change.json /tmp/backup.json",
        "python3 scripts/repo.py status --change x",
        "ls 2>&1",
    ]

    def test_ask_commands(self):
        for text in self.ASK:
            with self.subTest(command=text):
                code, out = self.command(text)
                self.assertEqual((code, out["decision"]), (3, "ask"))
                self.assertTrue(out["reason"])

    def test_allow_commands(self):
        for text in self.ALLOW:
            with self.subTest(command=text):
                code, out = self.command(text)
                self.assertEqual((code, out["decision"]), (0, "allow"))


class CommentHandlingTests(GuardCase):
    ASK = (
        "# note\ngit reset --hard",
        "# note\ngit push --force origin main",
        "# record it\npython3 scripts/repo.py approve x plan.md",
        "# fix claim\necho x > changes/x/change.json",
        "# don't stop\ngit reset --hard",
        "echo start # note\ngit push --force origin main",
        "bash -c '# note\ngit reset --hard'",
        "bash -c 'echo start # note\ngit push --force origin main'",
    )
    ALLOW = (
        "echo a#b",
        "git log --grep '#1'",
        "git commit -m 'title\n\n# not a comment'",
        "git commit -m \"it's #1\"",
        "# git reset --hard",
        "echo ok # git push --force",
    )

    def test_commands_after_a_comment_are_inspected(self):
        for command in self.ASK:
            self.assertEqual(self.command(command)[1]["decision"], "ask", command)

    def test_hash_that_hides_nothing_is_allowed(self):
        for command in self.ALLOW:
            self.assertEqual(self.command(command)[1]["decision"], "allow", command)


class CommandGapTests(GuardCase):
    ASK = (
        "bash -lc 'git reset --hard'",
        "sh -ec 'git push --force origin main'",
        "bash --login -c 'git reset --hard'",
        "bash -o pipefail -c 'git reset --hard'",
        "git branch --delete --force x",
        "git branch -d -f x",
        "git checkout -- .",
        "git checkout .",
        "git checkout main -- src/a.py",
        "git restore .",
        "git restore src/a.py",
        "git restore --staged --worktree src/a.py",
        "env git reset --hard",
        "sudo git reset --hard",
        "xargs git reset --hard",
        "find . -exec git reset --hard ;",
    )
    ALLOW = (
        "echo git reset --hard",
        "printf 'git push --force'",
        "git branch -d x",
        "git checkout main",
        "git checkout -b new",
        "git restore --staged src/a.py",
        "bash -lc 'ls'",
        "bash script.sh -c x",
    )

    def test_ask_commands(self):
        for text in self.ASK:
            with self.subTest(command=text):
                self.assertEqual(self.command(text)[1]["decision"], "ask")

    def test_allow_commands(self):
        for text in self.ALLOW:
            with self.subTest(command=text):
                self.assertEqual(self.command(text)[1]["decision"], "allow")


class McpToolTests(GuardCase):
    def mcp(self, tool="mcp__x__replace_content", **tool_input):
        return self.claude({"tool_name": tool, "tool_input": tool_input, "cwd": str(self.root)})

    def test_edit_of_protected_paths_denied(self):
        for key, path in (("relative_path", "intent.md"), ("file_path", "changes/x/change.json"), ("path", "spec.md")):
            with self.subTest(path=path):
                proc = self.mcp(**{key: path})
                self.assertEqual(proc.returncode, 2)
                self.assertTrue(proc.stderr)

    def test_edit_of_change_json_denied_without_proposed_content(self):
        self.packet("x")
        self.assertEqual(self.mcp(relative_path="changes/x/change.json").returncode, 2)

    def test_ordinary_path_no_path_and_outside_path_allowed(self):
        self.assertEqual(self.mcp(relative_path="src/a.py").returncode, 0)
        self.assertEqual(self.mcp(pattern="x").returncode, 0)
        self.assertEqual(self.mcp(file_path="/tmp/outside.txt").returncode, 0)

    def test_read_only_tool_allowed_on_protected_path(self):
        self.assertEqual(self.mcp("mcp__x__find_symbol", relative_path="intent.md").returncode, 0)


class ClaudeAdapterTests(GuardCase):
    def test_edit_root_intent_denied_with_stderr(self):
        proc = self.claude(
            {"tool_name": "Edit", "tool_input": {"file_path": str(self.root / "intent.md"), "old_string": "# Intent", "new_string": "# Hacked"}, "cwd": str(self.root)}
        )
        self.assertEqual(proc.returncode, 2)
        self.assertIn("root-baseline", proc.stderr)
        self.assertEqual(proc.stdout, "")

    def test_write_changing_approvals_denied(self):
        self.packet("x")
        forged = self.change_json("x", approvals=[{"artifact": "plan.md", "sha256": "sha256:0", "by": "me", "at": "now"}])
        proc = self.claude(
            {"tool_name": "Write", "tool_input": {"file_path": str(self.root / "changes/x/change.json"), "content": forged}, "cwd": str(self.root)}
        )
        self.assertEqual(proc.returncode, 2)
        self.assertIn("change-approvals", proc.stderr)

    def test_edit_keeping_approvals_allowed(self):
        self.packet("x")
        self.approve("x", "plan.md")
        proc = self.claude(
            {
                "tool_name": "Edit",
                "tool_input": {"file_path": str(self.root / "changes/x/change.json"), "old_string": '"commit": ""', "new_string": '"commit": "abc"'},
                "cwd": str(self.root),
            }
        )
        self.assertEqual((proc.returncode, proc.stdout), (0, ""))

    def test_edit_changing_approvals_denied(self):
        self.packet("x")
        proc = self.claude(
            {
                "tool_name": "Edit",
                "tool_input": {"file_path": str(self.root / "changes/x/change.json"), "old_string": '"approvals": []', "new_string": '"approvals": [{"artifact": "plan.md"}]'},
                "cwd": str(self.root),
            }
        )
        self.assertEqual(proc.returncode, 2)

    def test_multiedit_applies_edits_in_order(self):
        self.packet("x")
        edits = [
            {"old_string": '"write_scope": []', "new_string": '"write_scope": ["a"]'},
            {"old_string": '"approvals": []', "new_string": '"approvals": ["forged"]'},
        ]
        proc = self.claude(
            {"tool_name": "MultiEdit", "tool_input": {"file_path": str(self.root / "changes/x/change.json"), "edits": edits}, "cwd": str(self.root)}
        )
        self.assertEqual(proc.returncode, 2)
        proc = self.claude(
            {"tool_name": "MultiEdit", "tool_input": {"file_path": str(self.root / "changes/x/change.json"), "edits": edits[:1]}, "cwd": str(self.root)}
        )
        self.assertEqual(proc.returncode, 0)

    def test_edit_of_missing_file_uses_new_string(self):
        proc = self.claude(
            {"tool_name": "Edit", "tool_input": {"file_path": str(self.root / "changes/n/change.json"), "old_string": "", "new_string": '{"approvals": ["x"]}'}, "cwd": str(self.root)}
        )
        self.assertEqual(proc.returncode, 2)

    def test_bash_force_push_asks(self):
        proc = self.claude({"tool_name": "Bash", "tool_input": {"command": "git push --force"}, "cwd": str(self.root)})
        self.assertEqual(proc.returncode, 0)
        out = json.loads(proc.stdout)["hookSpecificOutput"]
        self.assertEqual(out["hookEventName"], "PreToolUse")
        self.assertEqual(out["permissionDecision"], "ask")
        self.assertIn("force-push", out["permissionDecisionReason"])

    def test_bash_cwd_defaults_to_process_cwd(self):
        proc = self.claude({"tool_name": "Bash", "tool_input": {"command": "git reset --hard"}})
        self.assertEqual(json.loads(proc.stdout)["hookSpecificOutput"]["permissionDecision"], "ask")

    def test_bash_benign_allowed_silently(self):
        proc = self.claude({"tool_name": "Bash", "tool_input": {"command": "git status"}, "cwd": str(self.root)})
        self.assertEqual((proc.returncode, proc.stdout), (0, ""))

    def test_unknown_tool_allowed(self):
        proc = self.claude({"tool_name": "Read", "tool_input": {"file_path": str(self.root / "intent.md")}, "cwd": str(self.root)})
        self.assertEqual((proc.returncode, proc.stdout), (0, ""))

    def test_garbage_stdin_allowed_with_note(self):
        for garbage in ("not json", "", "[1, 2]"):
            with self.subTest(stdin=garbage):
                proc = self.claude(garbage)
                self.assertEqual((proc.returncode, proc.stdout), (0, ""))
                self.assertTrue(proc.stderr.strip())

    def test_malformed_tool_input_allowed_with_note(self):
        self.packet("x")
        payloads = [
            {"tool_name": "MultiEdit", "tool_input": {"file_path": "changes/x/change.json", "edits": "oops"}, "cwd": str(self.root)},
            {"tool_name": "Edit", "tool_input": {"file_path": "changes/x/change.json", "old_string": 5, "new_string": None}, "cwd": str(self.root)},
            {"tool_name": "Bash", "tool_input": {"command": ["not", "a", "string"]}, "cwd": str(self.root)},
        ]
        for payload in payloads:
            with self.subTest(tool=payload["tool_name"]):
                proc = self.claude(payload)
                self.assertEqual((proc.returncode, proc.stdout), (0, ""), proc.stderr)
                self.assertNotIn("Traceback", proc.stderr)


class SettingsAdapterTests(unittest.TestCase):
    def test_settings_holds_only_hooks_calling_the_guard(self):
        data = json.loads(SETTINGS.read_text())
        self.assertEqual(list(data), ["hooks"])
        self.assertEqual(list(data["hooks"]), ["PreToolUse"])
        commands = [h["command"] for entry in data["hooks"]["PreToolUse"] for h in entry["hooks"]]
        self.assertTrue(commands)
        for command in commands:
            self.assertTrue(command.endswith("scripts/hooks/guard.py claude"), command)
        matchers = {e["matcher"] for e in data["hooks"]["PreToolUse"]}
        self.assertEqual(matchers, {"Edit|Write|MultiEdit", "Bash", "mcp__.*(write|edit|replace|insert|create|delete|rename).*"})

    def test_guard_is_executable_python(self):
        self.assertTrue(os.access(GUARD, os.X_OK))
        self.assertTrue(GUARD.read_text().startswith("#!/usr/bin/env python3\n"))


if __name__ == "__main__":
    unittest.main()

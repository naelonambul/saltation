"""Use real Git objects to exercise the Oracle job's routing boundary."""

from __future__ import annotations

import os
import re
import shlex
import subprocess
import sys
import tempfile
import textwrap
import unittest
from pathlib import Path

SCRIPTS = Path(__file__).resolve().parents[1]
WORKFLOW = SCRIPTS.parent / ".github/workflows/repository.yml"
sys.path.insert(0, str(SCRIPTS))
from oracle_pr_route import classify


class OraclePrRoute(unittest.TestCase):
    def setUp(self):
        directory = tempfile.TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        self.root = Path(directory.name) / "git"
        self.root.mkdir()
        self.body = Path(directory.name) / "body"
        self.body.write_text("Change-ID: docs-guide\n")
        self.git("init", "-q", "-b", "main")
        self.git("config", "user.email", "fixture@example.test")
        self.git("config", "user.name", "Git Fixture")
        self.write("README.md", "base\n")
        self.write("docs/old.md", "original guide\n")
        self.write("bank/src/main.rs", "existing code\n")
        self.commit()
        self.git("switch", "-qc", "pr")

    def git(self, *args):
        return subprocess.run(
            ["git", *args], cwd=self.root, check=True, capture_output=True, text=True,
        ).stdout.strip()

    def write(self, path, content="changed\n"):
        file = self.root / path
        file.parent.mkdir(parents=True, exist_ok=True)
        file.write_text(content)
        return file

    def commit(self, allow_empty=False):
        self.git("add", "-A")
        args = ["commit", "-qm", "fixture"]
        if allow_empty:
            args.append("--allow-empty")
        self.git(*args)
        return self.git("rev-parse", "HEAD")

    def install_classifier(self):
        for name in ("oracle_pr_route.py", "repo.py"):
            self.write(f"scripts/{name}", (SCRIPTS / name).read_text())

    def candidate(self, *, allow_empty=False, extra=None, classifier_on_base=False):
        head = self.commit(allow_empty)
        self.git("switch", "-q", "main")
        self.write("README.ko.md", "base branch moved\n")
        if classifier_on_base:
            self.install_classifier()
        base = self.commit()
        self.git("merge", "--no-ff", "--no-commit", head)
        if extra:
            self.write(extra)
        candidate = self.commit()
        return base, head, candidate

    def decide(self, commits, *, event="pull_request"):
        return classify(self.root, event, *commits, self.body)

    def test_guide_and_active_packet(self):
        self.write("docs/guides/agent.md")
        self.write("changes/docs-guide/change.json", "{}\n")
        self.write("changes/docs-guide/plan.md")
        mode, reason = self.decide(self.candidate())
        self.assertEqual(mode, "docs-only")
        self.assertIn("docs/guides/agent.md", reason)
        self.assertNotIn("README.ko.md", reason)

    def test_document_rename_and_root_readme(self):
        self.git("mv", "docs/old.md", "docs/new.md")
        self.write("README.md")
        mode, reason = self.decide(self.candidate())
        self.assertEqual(mode, "docs-only")
        self.assertIn("docs/old.md", reason)
        self.assertIn("docs/new.md", reason)

    def test_code_renamed_into_docs_requires_oci(self):
        self.git("mv", "bank/src/main.rs", "docs/sample.md")
        self.assertEqual(self.decide(self.candidate())[0], "full")

    def test_deleted_code_with_doc_requires_oci(self):
        self.git("rm", "bank/src/main.rs")
        self.write("README.md")
        self.assertEqual(self.decide(self.candidate())[0], "full")

    def test_any_unapproved_path_in_a_mixed_pr_requires_oci(self):
        for path in (
            "bank/src/main.rs", ".github/workflows/repository.yml", "checks.json",
            "intent.md", "AGENTS.md", "docs/script.sh", "unmapped.txt",
            "changes/other/plan.md", "changes/docs-guide/spec.md",
        ):
            with self.subTest(path=path):
                self.write("README.md")
                file = self.root / path
                old = file.read_bytes() if file.exists() else None
                self.write(path)
                tip = self.git("rev-parse", "HEAD")
                base = self.git("rev-parse", "main")
                self.assertEqual(self.decide(self.candidate())[0], "full")
                self.git("reset", "--hard", base)
                self.git("switch", "-q", "pr")
                self.git("reset", "--hard", tip)
                if old is not None:
                    self.assertEqual(file.read_bytes(), old)

    def test_packet_only_requires_oci(self):
        self.write("changes/docs-guide/plan.md")
        self.assertEqual(self.decide(self.candidate())[0], "full")

    def test_executable_doc_requires_oci(self):
        self.write("docs/old.md").chmod(0o755)
        self.git("update-index", "--chmod=+x", "docs/old.md")
        self.assertEqual(self.decide(self.candidate())[0], "full")

    def test_symlink_in_docs_requires_oci(self):
        (self.root / "docs/new.md").symlink_to("../bank/src/main.rs")
        self.assertEqual(self.decide(self.candidate())[0], "full")

    def test_change_present_only_in_merge_candidate_requires_oci(self):
        self.write("README.md")
        self.assertEqual(self.decide(self.candidate(extra="bank/src/main.rs"))[0], "full")

    def test_empty_comparison_requires_oci(self):
        self.assertEqual(self.decide(self.candidate(allow_empty=True))[0], "full")

    def test_non_pr_missing_and_mismatching_identities_require_oci(self):
        self.write("README.md")
        base, head, candidate = self.candidate()
        for commits, event in (
            ((base, head, candidate), "push"),
            ((base, head, candidate), "schedule"),
            (("", head, candidate), "pull_request"),
            (("f" * 40, head, candidate), "pull_request"),
            ((head, base, candidate), "pull_request"),
            ((base, head, head), "pull_request"),
        ):
            with self.subTest(commits=commits, event=event):
                self.assertEqual(self.decide(commits, event=event)[0], "full")
        self.body.unlink()
        self.assertEqual(self.decide((base, head, candidate))[0], "full")

    def test_git_error_requires_oci(self):
        self.write("README.md")
        commits = self.candidate()
        (self.root / ".git").rename(self.root / "git-backup")
        self.assertEqual(self.decide(commits)[0], "full")

    def status_workflow_decision(self, commits):
        base, head, candidate = commits
        output = self.body.with_name("outputs")
        step = re.search(
            r"(?m)^      - name: Select Oracle runner\n(?:        [^\n]*\n)*?        run: \|\n((?:          .*\n)+)",
            WORKFLOW.read_text(),
        ).group(1)
        script = textwrap.dedent(step).replace("/tmp/pr-body.md", shlex.quote(str(self.body)))
        env = {
            **os.environ,
            "GITHUB_EVENT_NAME": "pull_request",
            "GITHUB_SHA": candidate,
            "PR_BASE_SHA": base,
            "PR_HEAD_SHA": head,
            "GITHUB_OUTPUT": str(output),
        }
        subprocess.run(["bash", "-e", "-c", script], cwd=self.root, env=env, check=True, capture_output=True)
        return output.read_text()

    def test_introducing_pr_with_no_base_classifier_requires_oci(self):
        self.write("README.md")
        self.assertEqual(self.status_workflow_decision(self.candidate()), "mode=full\n")

    def test_status_workflow_accepts_real_documentation_change(self):
        self.write("README.md")
        commits = self.candidate(classifier_on_base=True)
        self.assertEqual(self.status_workflow_decision(commits), "mode=docs-only\n")

    def test_status_workflow_does_not_execute_candidate_classifier(self):
        self.git("switch", "-q", "main")
        self.install_classifier()
        self.commit()
        self.git("switch", "-qC", "pr", "main")
        self.write("README.md")
        self.write("bank/src/main.rs")
        # A candidate-only classifier would unconditionally route this code change away from OCI.
        self.write("scripts/oracle_pr_route.py", "print('docs-only')\n")
        self.write("scripts/repo.py", "raise SystemExit('candidate module must not be imported')\n")
        commits = self.candidate()
        self.assertEqual(self.status_workflow_decision(commits), "mode=full\n")

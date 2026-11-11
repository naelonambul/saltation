"""Fixtures for `repo.py verify`: routing, statuses, evidence."""

import json
import os
import subprocess
import sys
import unittest
from pathlib import Path
from unittest import mock

from scripts.tests.support import REPO_PY, RepoCase, digest

PY = sys.executable


def check(cid, code="pass", paths=("src/**",), group="core", timeout=30, requires=(), cwd="."):
    return {
        "id": cid,
        "group": group,
        "argv": [PY, "-c", code] if code != "pass" else [PY, "-c", "print('ok')"],
        "cwd": cwd,
        "timeout_seconds": timeout,
        "paths": list(paths),
        "requires": list(requires),
    }


class Verify(RepoCase):
    def configure(self, *checks):
        self.write("checks.json", json.dumps({"schema": 1, "baseline_branch": "main", "checks": list(checks)}))
        self.commit("checks")
        self.git("switch", "-q", "-c", "work")

    def verify(self, *args):
        evidence_dir = self.root.parent / "evidence"
        proc = self.run_repo("verify", "--json", "--evidence-dir", str(evidence_dir), *args)
        try:
            result = json.loads(proc.stdout)
        except json.JSONDecodeError:
            result = {"checks": []}
        result["exit"] = proc.returncode
        result["stderr"] = proc.stderr
        result["by_id"] = {c["id"]: c for c in result["checks"]}
        return result

    def test_routed_check_runs_and_other_is_not_applicable_with_reason(self):
        self.configure(check("src"), check("web", paths=["web/**"]))
        self.write("src/a.txt", "x\n")
        result = self.verify()
        self.assertEqual(result["exit"], 0)
        self.assertEqual(result["by_id"]["src"]["status"], "passed")
        web = result["by_id"]["web"]
        self.assertEqual(web["status"], "not-applicable")
        self.assertIn("no changed path matches", web["reason"])

    def test_nonzero_exit_fails(self):
        self.configure(check("src", code="import sys; sys.exit(3)"))
        self.write("src/a.txt", "x\n")
        result = self.verify()
        self.assertEqual((result["exit"], result["by_id"]["src"]["status"], result["by_id"]["src"]["exit_code"]), (1, "failed", 3))

    def test_timeout_fails(self):
        self.configure(check("src", code="import time; time.sleep(30)", timeout=1))
        self.write("src/a.txt", "x\n")
        result = self.verify()
        self.assertEqual(result["exit"], 1)
        self.assertIn("timed out", result["by_id"]["src"]["reason"])

    def test_missing_executable_is_blocked_not_passed(self):
        self.configure(check("src", requires=["definitely-not-installed-tool"]))
        self.write("src/a.txt", "x\n")
        result = self.verify()
        self.assertEqual((result["exit"], result["by_id"]["src"]["status"]), (1, "blocked"))

    def test_missing_working_directory_is_blocked(self):
        self.configure(check("src", cwd="gone"))
        self.write("src/a.txt", "x\n")
        self.assertEqual(self.verify()["by_id"]["src"]["status"], "blocked")

    def test_unmapped_path_selects_full_suite(self):
        self.configure(check("src"), check("web", paths=["web/**"]))
        self.write("README.md", "unmapped\n")
        result = self.verify()
        self.assertEqual({c["status"] for c in result["checks"]}, {"passed"})
        self.assertEqual(result["routing"]["unmapped_paths"], ["README.md"])

    def test_full_runs_everything(self):
        self.configure(check("src"), check("web", paths=["web/**"]))
        self.assertEqual({c["status"] for c in self.verify("--full")["checks"]}, {"passed"})

    def test_group_filter_reports_not_run(self):
        self.configure(check("src"), check("web", paths=["web/**"], group="web"))
        result = self.verify("--full", "--group", "core")
        self.assertEqual(result["by_id"]["web"]["status"], "not-run")
        self.assertEqual(result["exit"], 0)
        self.assertFalse(result["complete"], "a group-filtered run must never claim complete verification")
        self.assertTrue(self.verify("--full")["complete"])

    def test_evidence_records_log_digest(self):
        self.configure(check("src", code="print('hello evidence')"))
        self.write("src/a.txt", "x\n")
        result = self.verify()
        record = result["by_id"]["src"]
        log = Path(self.root.parent / "evidence" / result["run_id"] / "src.log")
        self.assertEqual(record["log_sha256"], digest(log.read_bytes()))
        self.assertIn("hello evidence", log.read_text())
        self.assertEqual(result["source"]["dirty"], True)
        stored = json.loads((log.parent / "evidence.json").read_text())
        self.assertEqual(stored["checks"][0]["status"], "passed")

    def test_evidence_binds_the_verified_files(self):
        self.configure(check("src"))
        self.write("src/a.txt", "x\n")
        first = self.verify()["source"]["tree_sha256"]
        self.assertEqual(self.verify()["source"]["tree_sha256"], first)
        self.write("src/a.txt", "y\n")
        self.assertNotEqual(self.verify()["source"]["tree_sha256"], first)

    def test_files_changed_during_the_run_leave_it_incomplete(self):
        self.configure(check("src", code="open('src/made.txt', 'w').write('x')"))
        self.write("src/a.txt", "x\n")
        self.assertFalse(self.verify()["complete"])

    def test_files_changed_during_the_run_fail_it(self):
        self.configure(check("src", code="open('src/made.txt', 'w').write('x')"))
        self.write("src/a.txt", "x\n")
        proc = self.run_repo("verify", "--full")
        self.assertEqual(proc.returncode, 1, proc.stdout)
        self.assertIn("files changed during the run", proc.stdout)

    def test_a_partial_group_run_still_exits_zero(self):
        self.configure(check("src"), check("web", group="web"))
        self.write("src/a.txt", "x\n")
        self.assertEqual(self.run_repo("verify", "--group", "core").returncode, 0)

    def test_digest_binds_a_submodule_commit(self):
        sys.path.insert(0, str(REPO_PY.parent))
        import repo  # noqa: E402

        self.configure(check("src"))
        head = self.git("rev-parse", "HEAD")
        (self.root / "sub").mkdir()
        self.git("update-index", "--add", "--cacheinfo", f"160000,{head},sub")
        first = repo.candidate_digest(self.root, None)
        self.git("update-index", "--cacheinfo", f"160000,{'1' * 40},sub")
        self.assertNotEqual(repo.candidate_digest(self.root, None), first)

    def test_malformed_config_blocks(self):
        self.write("checks.json", "{bad")
        proc = self.run_repo("verify", "--full")
        self.assertEqual(proc.returncode, 1)
        self.assertIn("blocked", proc.stderr)
        self.assertNotIn("Traceback", proc.stderr)

    def test_a_non_executable_check_is_blocked_with_evidence(self):
        self.write("c.sh", "#!/bin/sh\n")
        self.configure({**check("src"), "argv": ["./c.sh"]})
        self.write("src/a.txt", "x\n")
        result = self.verify("--full")
        self.assertEqual(result["exit"], 1, result["stderr"])
        self.assertEqual(result["by_id"]["src"]["status"], "blocked")
        self.assertNotIn("Traceback", result["stderr"])

    def test_printed_evidence_path_is_relative(self):
        self.configure(check("src"))
        self.write("src/a.txt", "x\n")
        proc = self.run_repo("verify")
        self.assertEqual(proc.returncode, 0, proc.stderr)
        self.assertRegex(proc.stdout, r"(?m)^evidence: \.evidence/\S+/evidence\.json$")

    def test_shell_string_argv_is_rejected(self):
        bad = check("src")
        bad["argv"] = f"{PY} -c 'print(1)'"
        self.configure(bad)
        result = self.verify("--full")
        self.assertEqual(result["exit"], 1)
        self.assertIn("never a shell string", result["stderr"])

    def test_missing_evidence_after_success_is_blocked(self):
        self.configure(check("src"))
        sys.path.insert(0, str(REPO_PY.parent))
        import repo

        ctx = repo.Context(root=self.root)
        with mock.patch.object(repo, "log_digest", return_value=None):
            record = repo.run_check(ctx, check("src"), self.root.parent / "ev" / "src.log")
        self.assertEqual(record["status"], "blocked")
        self.assertIn("evidence log is missing", record["reason"])


class FixtureIsolation(unittest.TestCase):
    def test_importing_the_test_package_drops_repository_locating_git_variables(self):
        code = "import os, scripts.tests; print(sorted({'GIT_DIR', 'GIT_WORK_TREE'} & set(os.environ)))"
        env = {**os.environ, "GIT_DIR": "/nonexistent/git", "GIT_WORK_TREE": "/nonexistent/tree"}
        proc = subprocess.run([PY, "-c", code], cwd=REPO_PY.parents[1], env=env, capture_output=True, text=True)
        self.assertEqual(proc.returncode, 0, proc.stderr)
        self.assertEqual(proc.stdout.strip(), "[]")


if __name__ == "__main__":
    unittest.main()

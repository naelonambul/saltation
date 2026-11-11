"""Invariants of the repository's own CI workflow (plain text; no YAML dependency)."""

import json
import re
import sys
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
WORKFLOW = ROOT / ".github/workflows/repository.yml"


MUTERS = ("repo.py", "continue-on-error", "|| true", "shell:")


def job_block(text: str, name: str) -> str | None:
    match = re.search(rf"(?ms)^  {re.escape(name)}:\n(.*?)(?=^  \S|\Z)", text)
    return match.group(1) if match else None


def direct_run_findings(text: str, argv: list[str]) -> list[str]:
    """Why the control-plane-tests job might not run the tests directly, or let a failure pass, or []."""
    block = job_block(text, "control-plane-tests")
    if block is None:
        return ["no control-plane-tests job"]
    block = re.sub(r"(?m)(^|\s)#.*$", "", block)
    findings = []
    runs = [r.strip() for r in re.findall(r"(?m)^\s*(?:-\s+)?run:[ \t]*(\S.*)$", block)]
    if " ".join(argv) not in runs:
        findings.append(f"no run step equals the control-plane check {' '.join(argv)!r}: {runs}")
    findings += [f"the job contains {m!r}" for m in MUTERS if m in block]
    if re.search(r"(?m)^\s+(?:-\s+)?if:", block):
        findings.append("a step has an if: condition")
    return findings


@unittest.skipUnless(WORKFLOW.is_file(), "repository has no CI workflow")
class WorkflowInvariants(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.text = WORKFLOW.read_text()
        on = cls.text.split("\non:", 1)[1].split("\npermissions:", 1)[0]
        cls.triggers = on
        jobs = cls.text.split("\njobs:\n", 1)[1]
        cls.jobs = re.findall(r"(?m)^  ([a-z0-9-]+):\n", jobs)

    def test_no_path_filters(self):
        self.assertNotRegex(self.triggers, r"paths(-ignore)?:")
        self.assertNotRegex(self.triggers, r"branches-ignore:")

    def test_pull_request_types_include_edited(self):
        types = re.search(r"types:\s*\[([^\]]*)\]", self.triggers).group(1)
        self.assertTrue({"opened", "reopened", "synchronize", "edited"} <= {t.strip() for t in types.split(",")})

    def test_actions_are_pinned_to_commit_shas(self):
        uses = re.findall(r"uses:\s*(\S+)", self.text)
        self.assertTrue(uses)
        for ref in uses:
            self.assertRegex(ref, r"@[0-9a-f]{40}$", ref)

    def test_every_checkout_fetches_full_history(self):
        checkouts = self.text.count("uses: actions/checkout@")
        self.assertEqual(checkouts, len(re.findall(r"fetch-depth:\s*0\b", self.text)))

    def test_concurrency_cancels_superseded_runs(self):
        self.assertRegex(self.text, r"(?m)^concurrency:\n(?:  .*\n)*  cancel-in-progress: true")

    def test_no_job_is_skipped_by_condition(self):
        # Job-level `if:` would turn "not applicable" into a provider skip; only the summary may use always().
        job_ifs = re.findall(r"(?m)^    if: (.*)$", self.text)
        self.assertEqual(job_ifs, ["always()"])

    def test_summary_needs_every_job_and_requires_success(self):
        needs = re.search(r"(?m)^  summary:\n(?:    .*\n)*?    needs: \[([^\]]*)\]", self.text).group(1)
        self.assertEqual({n.strip() for n in needs.split(",")}, set(self.jobs) - {"summary"})
        self.assertIn('v["result"] == "success"', self.text)

    def test_identity_job_compares_the_author_with_the_variable(self):
        self.assertIn("identity", self.jobs)
        self.assertIn("AGENT_LOGIN: ${{ vars.AGENT_LOGIN }}", self.text)
        self.assertIn("PR_AUTHOR: ${{ github.event.pull_request.user.login }}", self.text)
        self.assertIn('[ "$PR_AUTHOR" != "$AGENT_LOGIN" ]', self.text)

    def test_control_plane_job_runs_the_registered_command_without_repo_py(self):
        """The job's pass/fail must not depend on repo.py verify, and nothing in it may mute a failure."""
        argv = next(c for c in json.loads((ROOT / "checks.json").read_text())["checks"] if c["id"] == "control-plane")["argv"]
        self.assertEqual(direct_run_findings(self.text, argv), [])
        runner = " ".join(argv)
        for name, new in {
            "muted with || true": f"run: {runner} || true",
            "through repo.py": "run: python3 scripts/repo.py verify --group core",
            "echoed": f"run: echo {runner}",
            "step if": f"if: false\n        run: {runner}",
            "continue-on-error": f"continue-on-error: true\n        run: {runner}",
            "shell": f"shell: bash\n        run: {runner}",
        }.items():
            with self.subTest(name):
                self.assertEqual(self.text.count(f"run: {runner}"), 1)
                self.assertNotEqual(direct_run_findings(self.text.replace(f"run: {runner}", new), argv), [])

    def test_every_check_group_is_run(self):
        sys.path.insert(0, str(ROOT / "scripts"))
        import repo  # noqa: E402

        findings = repo.config_findings(repo.Context(root=ROOT))
        self.assertEqual([f.message for f in findings if f.code == "checks-unwired"], [])

if __name__ == "__main__":
    unittest.main()

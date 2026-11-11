"""`repo.py approve`: mechanical, order-checked, never an authority; `close` no longer exists."""

import json

from scripts.tests.support import UNESTABLISHED, RepoCase, digest


class Approve(RepoCase):
    def test_records_digest_of_current_bytes(self):
        self.packet("imp", kind="repository", scope=["src/**"])
        proc = self.run_repo("approve", "imp", "plan.md", "--by", "owner", "--note", "reviewed in chat")
        self.assertEqual(proc.returncode, 0, proc.stderr)
        self.assertIn("unverified", proc.stdout)
        claims = json.loads(self.read("changes/imp/change.json"))["approvals"]
        self.assertEqual(len(claims), 1)
        self.assertEqual(claims[0]["sha256"], digest(self.read("changes/imp/plan.md")))
        self.assertEqual((claims[0]["artifact"], claims[0]["by"], claims[0]["note"]), ("plan.md", "owner", "reviewed in chat"))
        self.assertRegex(claims[0]["at"], r"^\d{4}-\d\d-\d\dT\d\d:\d\d:\d\dZ$")
        result = self.status("--change", "imp")
        self.assertEqual((result["by_id"]["imp"]["readiness"], result["by_id"]["imp"]["approval"]), ("ready", "unverified"))

    def test_replaces_stale_claim_for_same_artifact(self):
        self.packet("imp", kind="repository")
        self.approve("imp", "plan.md")
        self.write("changes/imp/plan.md", "# Plan v2\n")
        self.assertIn("stale-approval", self.codes(self.status(), "imp"))
        proc = self.run_repo("approve", "imp", "plan.md", "--by", "owner")
        self.assertEqual(proc.returncode, 0, proc.stderr)
        self.assertIn("replaced 1 earlier claim", proc.stdout)
        claims = json.loads(self.read("changes/imp/change.json"))["approvals"]
        self.assertEqual([c["sha256"] for c in claims], [digest("# Plan v2\n")])
        self.assertNotIn("stale-approval", self.codes(self.status(), "imp"))

    def test_plan_claim_binds_write_scope_and_upstream_claims_do_not(self):
        self.packet("init", kind="product-init", scope=["src/**"], artifacts={"intent.md": "# I\n", "spec.md": "# S\n", "plan.md": "# P\n"})
        for name in ("intent.md", "spec.md", "plan.md"):
            proc = self.run_repo("approve", "init", name, "--by", "owner")
            self.assertEqual(proc.returncode, 0, proc.stderr)
        claims = {c["artifact"]: c for c in json.loads(self.read("changes/init/change.json"))["approvals"]}
        self.assertNotIn("write_scope_sha256", claims["intent.md"])
        self.assertNotIn("write_scope_sha256", claims["spec.md"])
        self.assertRegex(claims["plan.md"]["write_scope_sha256"], r"^sha256:[0-9a-f]{64}$")

    def test_reapproval_rebinds_a_changed_scope_without_an_artifact_edit(self):
        self.packet("imp", kind="repository", scope=["src/**"])
        self.run_repo("approve", "imp", "plan.md", "--by", "owner")
        self.set_scope("imp", ["src/**", "docs/**"])
        self.assertIn("stale-approval", self.codes(self.status(), "imp"))
        proc = self.run_repo("approve", "imp", "plan.md", "--by", "owner")
        self.assertEqual(proc.returncode, 0, proc.stderr)
        self.assertEqual(self.status("--change", "imp")["by_id"]["imp"]["readiness"], "ready")

    def test_refuses_out_of_order(self):
        self.packet("init", kind="product-init", artifacts={"intent.md": "# I\n", "spec.md": "# S\n", "plan.md": "# P\n"})
        proc = self.run_repo("approve", "init", "plan.md", "--by", "owner")
        self.assertEqual(proc.returncode, 1)
        self.assertIn("approve intent.md before plan.md", proc.stderr)
        self.assertEqual(json.loads(self.read("changes/init/change.json"))["approvals"], [])
        for name in ("intent.md", "spec.md", "plan.md"):
            self.assertEqual(self.run_repo("approve", "init", name, "--by", "owner").returncode, 0)
        self.assertEqual(self.status()["by_id"]["init"]["stage"], "implementation")

    def test_refuses_artifact_outside_chain_or_missing(self):
        self.packet("imp", kind="repository")
        proc = self.run_repo("approve", "imp", "spec.md", "--by", "owner")
        self.assertEqual(proc.returncode, 1)
        self.assertIn("not in the chain", proc.stderr)
        self.establish_baseline()
        self.packet("beh", kind="behavior", artifacts={"plan.md": "# P\n"})
        proc = self.run_repo("approve", "beh", "spec.md", "--by", "owner")
        self.assertEqual(proc.returncode, 1)
        self.assertIn("does not exist yet", proc.stderr)

    def test_refuses_blank_approver_unknown_packet_and_merged_change(self):
        self.packet("imp", kind="repository")
        self.assertIn("--by must name", self.run_repo("approve", "imp", "plan.md", "--by", "  ").stderr)
        self.assertIn("not found", self.run_repo("approve", "nope", "plan.md", "--by", "owner").stderr)
        self.approve("imp", "plan.md")
        self.commit("merged on main")
        proc = self.run_repo("approve", "imp", "plan.md", "--by", "owner")
        self.assertEqual(proc.returncode, 1)
        self.assertIn("already merged on 'main'", proc.stderr)

    def test_notes_downstream_approvals_to_repeat(self):
        self.establish_baseline()
        self.packet("beh", kind="behavior", artifacts={"spec.md": "# Spec\nnew\n", "plan.md": "# P\n"})
        self.approve("beh", "spec.md", "plan.md")
        proc = self.run_repo("approve", "beh", "spec.md", "--by", "owner")
        self.assertEqual(proc.returncode, 0, proc.stderr)
        self.assertIn("approve again, in order: plan.md", proc.stdout)


class PlaceholderProductInit(RepoCase):
    def test_refuses_artifacts_that_keep_the_marker(self):
        self.packet("init", kind="product-init", artifacts={"intent.md": UNESTABLISHED + "# I\n", "spec.md": "# S\n", "plan.md": "# P\n"})
        proc = self.run_repo("approve", "init", "intent.md", "--by", "owner")
        self.assertEqual(proc.returncode, 1)
        self.assertIn("unestablished marker", proc.stderr)
        self.write("changes/init/intent.md", "# I\n")
        self.assertEqual(self.run_repo("approve", "init", "intent.md", "--by", "owner").returncode, 0)
        self.write("changes/init/spec.md", UNESTABLISHED + "# S\n")
        proc = self.run_repo("approve", "init", "spec.md", "--by", "owner")
        self.assertEqual(proc.returncode, 1)
        self.assertIn("unestablished marker", proc.stderr)


class CloseIsGone(RepoCase):
    def test_close_is_not_a_command(self):
        self.packet("imp", kind="repository")
        proc = self.run_repo_subprocess("close", "imp", "--evidence", "x")
        self.assertEqual(proc.returncode, 2)
        self.assertIn("invalid choice", proc.stderr)

"""Fixtures for `repo.py status`: identity, lifecycle, approval, scope, merged packets."""

import contextlib
import io
import json
import os
import subprocess
import sys
import unittest

from scripts.tests import support
from scripts.tests.support import REPO_PY, UNESTABLISHED, RepoCase, digest


class ChangeIdentity(RepoCase):
    def setUp(self):
        super().setUp()
        self.packet("alpha", kind="repository", scope=["src/**"])
        self.commit("alpha packet")

    def pr(self, body):
        (self.root.parent / "body.md").write_text(body)
        return self.status("--pr-body", str(self.root.parent / "body.md"))

    def test_pr_without_change_id_is_blocked(self):
        self.assertFailed(self.pr("## Summary\n<!-- Change-ID: alpha -->\n"), "no `Change-ID")

    def test_pr_with_several_change_ids_is_blocked(self):
        self.assertFailed(self.pr("Change-ID: alpha\nChange-ID: beta\n"), "several Change-IDs")

    def test_pr_change_id_for_missing_packet_is_blocked(self):
        self.assertFailed(self.pr("Change-ID: ghost\n"), "has no packet")

    def test_branch_disagreeing_with_explicit_id_is_blocked(self):
        self.git("switch", "-q", "-c", "change/other")
        self.assertFailed(self.status("--change", "alpha"), "disagree")

    def test_branch_inference_agrees(self):
        self.git("switch", "-q", "-c", "change/alpha")
        result = self.status()
        self.assertEqual(result["active"], "alpha")
        self.assertEqual(result["active_source"], "branch")

    def test_require_change_blocks_without_identity(self):
        self.assertFailed(self.status("--require-change"), "no active change")

    def test_several_touched_packets_are_ambiguous(self):
        self.git("switch", "-q", "-c", "work")
        self.packet("beta", kind="repository")
        self.write("changes/alpha/plan.md", "# edited\n")
        self.assertFailed(self.status(), "several packets touched")


class Lifecycle(RepoCase):
    def test_product_init_missing_spec_blocks_before_plan(self):
        self.packet("init", kind="product-init", artifacts={"intent.md": "# I\n", "plan.md": "# P\n"})
        self.approve("init", "intent.md")
        result = self.status("--change", "init")
        change = result["by_id"]["init"]
        self.assertEqual(change["stage"], "spec")
        self.assertEqual(change["readiness"], "blocked")
        self.assertIn("missing-artifact", self.codes(result, "init"))

    def test_out_of_order_approval_is_blocked(self):
        self.packet("init", kind="product-init", artifacts={"intent.md": "# I\n", "spec.md": "# S\n", "plan.md": "# P\n"})
        self.approve("init", "intent.md", "plan.md")
        result = self.status("--change", "init")
        self.assertIn("out-of-order", self.codes(result, "init"))
        self.assertFailed(result, "approved before upstream spec.md")

    def test_out_of_order_timestamps_are_blocked(self):
        self.packet("init", kind="product-init", artifacts={"intent.md": "# I\n", "spec.md": "# S\n", "plan.md": "# P\n"})
        self.approve("init", "plan.md", "spec.md", "intent.md")  # later seconds for upstream artifacts
        self.assertIn("out-of-order", self.codes(self.status("--change", "init"), "init"))

    def test_ordered_product_init_reaches_implementation(self):
        self.packet("init", kind="product-init", artifacts={"intent.md": "# I\n", "spec.md": "# S\n", "plan.md": "# P\n"})
        self.approve("init", "intent.md", "spec.md", "plan.md")
        change = self.status("--change", "init")["by_id"]["init"]
        self.assertEqual((change["stage"], change["readiness"]), ("implementation", "ready"))

    def test_implementation_kind_needs_established_baseline(self):
        self.packet("impl", baseline={"intent.md": digest("x"), "spec.md": digest("y")})
        self.approve("impl", "plan.md")
        self.assertIn("baseline-unestablished", self.codes(self.status("--change", "impl"), "impl"))

    def test_stale_upstream_digest_is_stale_and_blocked(self):
        self.establish_baseline()
        self.git("switch", "-q", "-c", "change/impl")
        self.packet("impl", scope=["src/**"])
        self.approve("impl", "plan.md")
        self.commit("impl packet")
        self.assertEqual(self.status("--change", "impl")["by_id"]["impl"]["freshness"], "current")
        self.write("spec.md", "# Spec\nchanged by someone else\n")
        result = self.status("--change", "impl")
        change = result["by_id"]["impl"]
        self.assertEqual((change["freshness"], change["readiness"]), ("stale", "blocked"))
        self.assertFailed(result, "changed since this change was authored")

    def test_missing_upstream_artifact_is_blocked(self):
        self.establish_baseline()
        self.packet("impl")
        self.approve("impl", "plan.md")
        (self.root / "spec.md").unlink()
        result = self.status("--change", "impl")
        self.assertIn("missing-upstream", self.codes(result, "impl"))
        self.assertEqual(result["by_id"]["impl"]["readiness"], "blocked")

    def test_stored_lifecycle_label_is_rejected(self):
        self.packet("alpha", kind="repository", status="accepted")
        self.assertIn("invalid-change", self.codes(self.status("--change", "alpha"), "alpha"))


class Approval(RepoCase):
    def setUp(self):
        super().setUp()
        self.packet("alpha", kind="repository", scope=["src/**"])

    def set_approvals(self, approvals):
        path = self.root / "changes/alpha/change.json"
        data = json.loads(path.read_text())
        data["approvals"] = approvals
        path.write_text(json.dumps(data))

    def test_bare_typed_approver_counts_as_none(self):
        self.set_approvals([{"artifact": "plan.md", "by": "owner"}])
        change = self.status("--change", "alpha")["by_id"]["alpha"]
        self.assertEqual((change["approval"], change["readiness"]), ("none", "blocked"))

    def test_digest_bound_claim_is_unverified_and_ready(self):
        self.approve("alpha", "plan.md")
        change = self.status("--change", "alpha")["by_id"]["alpha"]
        self.assertEqual((change["approval"], change["readiness"]), ("unverified", "ready"))

    def test_local_claim_never_becomes_verified(self):
        self.approve("alpha", "plan.md")
        path = self.root / "changes/alpha/change.json"
        data = json.loads(path.read_text())
        data["approvals"][0]["note"] = "approved on GitHub by a distinct reviewer"
        path.write_text(json.dumps(data))
        self.assertEqual(self.status("--change", "alpha")["by_id"]["alpha"]["approval"], "unverified")

    def test_claim_for_old_bytes_is_stale(self):
        self.approve("alpha", "plan.md")
        self.write("changes/alpha/plan.md", "# Plan, edited after approval\n")
        result = self.status("--change", "alpha")
        change = result["by_id"]["alpha"]
        self.assertEqual((change["freshness"], change["approval"]), ("stale", "none"))
        self.assertFailed(result, "bound to different bytes")


class Revision(RepoCase):
    def implementing(self):
        self.git("switch", "-q", "-c", "change/imp")
        self.packet("imp", kind="repository", scope=["src/**"])
        self.commit("packet")
        self.write("src/a.txt", "x\n")

    def test_a_stale_plan_does_not_report_the_work_as_premature(self):
        self.implementing()
        self.approve("imp", "plan.md")
        self.write("changes/imp/plan.md", "# Plan v2\n")
        result = self.status("--change", "imp")
        self.assertEqual(result["exit"], 1)
        self.assertIn("stale-approval", self.codes(result, "imp"))
        self.assertEqual(result["by_id"]["imp"]["readiness"], "blocked")
        self.assertNotIn("not-ready", self.codes(result, "imp"))
        self.assertNotIn("awaiting-approval", self.codes(result, "imp"))

    def test_a_never_approved_plan_still_reports_premature_work(self):
        self.implementing()
        self.assertIn("not-ready", self.codes(self.status("--change", "imp"), "imp"))

    def test_a_reapproved_spec_blocks_until_the_plan_follows(self):
        self.establish_baseline()
        self.packet("beh", kind="behavior", artifacts={"spec.md": "# Spec\nnew\n", "plan.md": "# P\n"})
        self.approve("beh", "spec.md", "plan.md")
        self.write("changes/beh/spec.md", "# Spec\nnewer\n")
        self.assertEqual(self.run_repo("approve", "beh", "spec.md", "--by", "owner").returncode, 0)
        result = self.status("--change", "beh")
        self.assertIn("out-of-order", self.codes(result, "beh"))
        self.assertTrue(any("re-approve" in f for f in result["failures"]), result["failures"])
        self.assertEqual(self.run_repo("approve", "beh", "plan.md", "--by", "owner").returncode, 0)
        self.assertNotIn("out-of-order", self.codes(self.status("--change", "beh"), "beh"))


class SmallGates(RepoCase):
    def wiring(self, comment):
        checks = [{"id": g, "group": g, "argv": ["python3", "-c", "1"], "cwd": ".", "timeout_seconds": 9, "paths": ["**"], "requires": []} for g in ("core", "web")]
        self.write("checks.json", json.dumps({"schema": 1, "baseline_branch": "main", "checks": checks}))
        self.write(".github/workflows/repository.yml", f"jobs:\n  a:\n    steps:\n      - run: python3 scripts/repo.py verify --group core\n{comment}\n")

    def test_the_marker_cannot_unestablish_an_established_baseline(self):
        self.establish_baseline()
        self.git("switch", "-q", "-c", "change/r")
        self.packet("r", kind="repository", scope=["spec.md"])
        self.approve("r", "plan.md")
        self.write("spec.md", UNESTABLISHED + "# gone\n")
        self.assertFailed(self.status("--change", "r"), "may not modify established root")

    def test_an_unestablished_baseline_can_still_be_reshaped(self):
        self.git("switch", "-q", "-c", "change/r")
        self.packet("r", kind="repository", scope=["spec.md"])
        self.approve("r", "plan.md")
        self.write("spec.md", UNESTABLISHED + "# Spec\nreshaped\n")
        self.assertNotIn("out-of-scope", self.codes(self.status("--change", "r"), "r"))

    def test_identical_repeated_change_ids_fail(self):
        self.packet("imp", kind="repository")
        self.write("../body.md", "Change-ID: imp\nChange-ID: imp\n")
        result = self.status("--branch=", "--pr-body", str(self.root.parent / "body.md"))
        self.assertFailed(result, "several Change-IDs")

    def test_malformed_config_is_reported_not_raised(self):
        for text in ("{bad", "[]"):
            with self.subTest(text=text):
                self.write("checks.json", text)
                result = self.status()
                self.assertEqual(result["exit"], 1)
                self.assertTrue(any("checks.json" in f for f in result["failures"]), result["failures"])

    def test_a_non_string_baseline_digest_is_reported_not_raised(self):
        self.establish_baseline()
        self.packet("imp", baseline={"intent.md": 7, "spec.md": 7})
        self.assertIn("stale-baseline", self.codes(self.status("--change", "imp"), "imp"))

    def test_an_unhashable_check_id_is_reported_not_raised(self):
        self.write("checks.json", json.dumps({"schema": 1, "checks": [{"id": ["x"]}]}))
        self.assertEqual(self.status()["exit"], 1)

    def test_a_commented_group_line_is_not_a_job(self):
        self.wiring("      # - run: python3 scripts/repo.py verify --group web")
        self.assertFailed(self.status(), "check group 'web' has no")

    def test_a_trailing_comment_is_not_a_job(self):
        self.wiring("      - run: echo hi # python3 scripts/repo.py verify --group web")
        self.assertFailed(self.status(), "check group 'web' has no")


class CarriedBaseline(RepoCase):
    def carried(self):
        self.establish_baseline()
        self.git("switch", "-q", "-c", "change/beh")
        self.packet("beh", kind="behavior", scope=["spec.md"], artifacts={"spec.md": "# Spec\nnew\n", "plan.md": "# P\n"})
        self.approve("beh", "spec.md", "plan.md")
        self.commit("packet")

    def test_a_moved_baseline_is_stale_after_the_copy_to_the_root(self):
        self.carried()
        self.git("switch", "-q", "main")
        self.write("spec.md", "# Spec\nother change\n")
        self.commit("main moved")
        self.git("switch", "-q", "change/beh")
        self.write("spec.md", "# Spec\nnew\n")
        self.assertIn("stale-baseline", self.codes(self.status("--change", "beh"), "beh"))

    def test_the_copy_to_the_root_is_not_stale(self):
        self.carried()
        self.write("spec.md", "# Spec\nnew\n")
        self.assertNotIn("stale-baseline", self.codes(self.status("--change", "beh"), "beh"))

    def test_a_product_init_copied_to_the_root_is_not_established_already(self):
        self.git("switch", "-q", "-c", "change/init")
        self.packet("init", kind="product-init", artifacts={"intent.md": "# I\n", "spec.md": "# S\n", "plan.md": "# P\n"})
        self.approve("init", "intent.md", "spec.md", "plan.md")
        self.write("intent.md", "# I\n")
        self.write("spec.md", "# S\n")
        self.assertNotIn("baseline-established", self.codes(self.status("--change", "init"), "init"))


class ScopeBinding(RepoCase):
    def approved(self):
        self.packet("imp", kind="repository", scope=["src/**", "docs/**"])
        self.run_repo("approve", "imp", "plan.md", "--by", "owner")

    def test_widened_scope_is_stale_and_blocked(self):
        self.approved()
        self.set_scope("imp", ["src/**", "docs/**", "**"])
        result = self.status("--change", "imp")
        change = result["by_id"]["imp"]
        self.assertIn("stale-approval", self.codes(result, "imp"))
        self.assertEqual((change["readiness"], change["freshness"]), ("blocked", "stale"))

    def test_reordered_scope_stays_ready(self):
        self.approved()
        self.set_scope("imp", ["docs/**", "src/**"])
        self.assertEqual(self.status("--change", "imp")["by_id"]["imp"]["readiness"], "ready")

    def test_plan_claim_without_scope_binding_blocks_an_open_packet(self):
        self.packet("imp", kind="repository", scope=["src/**"])
        self.approve("imp", "plan.md", bind_scope=False)
        result = self.status("--change", "imp")
        self.assertIn("unbound-scope", self.codes(result, "imp"))
        self.assertEqual(result["by_id"]["imp"]["readiness"], "blocked")

    def test_merged_packet_without_scope_binding_stays_merged(self):
        self.packet("imp", kind="repository")
        self.approve("imp", "plan.md", bind_scope=False)
        self.commit("merge imp")
        result = self.status()
        self.assertEqual(result["by_id"]["imp"]["stage"], "merged")
        self.assertNotIn("unbound-scope", self.codes(result, "imp"))
        self.assertEqual(result["exit"], 0)


class TextOutput(RepoCase):
    def merged(self, *ids):
        for cid in ids:
            self.packet(cid, kind="repository")
            self.approve(cid, "plan.md")
        self.commit("merge")

    def text(self, *args):
        return self.run_repo("status", *args)

    def test_merged_packets_fold_into_one_line(self):
        self.merged("alpha", "beta")
        out = self.text().stdout
        self.assertIn("merged: 2\n", out)
        self.assertNotIn("alpha", out)

    def test_active_change_prints_in_full_when_merged(self):
        self.merged("alpha", "beta")
        out = self.text("--change", "alpha").stdout
        self.assertIn("* alpha [repository]", out)
        self.assertIn("stage=merged", out)
        self.assertIn("merged: 1\n", out)

    def test_merged_packet_with_a_reason_prints_in_full(self):
        sys.path.insert(0, str(REPO_PY.parent))
        import repo  # noqa: E402

        change = {"id": "alpha", "kind": "repository", "stage": "merged", "freshness": "current", "approval": "none",
                  "readiness": "blocked", "artifacts": {},
                  "reasons": [{"level": "error", "code": "invalid-change", "message": "look here"}]}
        result = {"active": None, "identity_errors": [], "changes": [change], "surface": [], "changed_paths": [], "ok": True, "failures": []}
        out = io.StringIO()
        with contextlib.redirect_stdout(out):
            repo.print_status(result)
        self.assertIn("look here", out.getvalue())
        self.assertNotIn("merged:", out.getvalue())

    def test_no_merged_line_without_merged_packets(self):
        self.packet("alpha", kind="repository")
        self.assertNotIn("merged: ", self.text().stdout)

    def test_json_lists_every_packet(self):
        self.merged("alpha", "beta")
        self.assertEqual({c["id"] for c in self.status()["changes"]}, {"alpha", "beta"})


class PlaceholderArtifacts(RepoCase):
    def marked(self):
        marked = {"intent.md": UNESTABLISHED + "# Intent\n", "spec.md": UNESTABLISHED + "# Specification\n", "plan.md": "# P\n"}
        self.packet("init", kind="product-init", artifacts=marked)
        self.approve("init", "intent.md", "spec.md", "plan.md")

    def test_hand_written_claim_on_a_marked_artifact_blocks(self):
        self.marked()
        result = self.status("--change", "init")
        self.assertIn("placeholder-artifact", self.codes(result, "init"))
        self.assertEqual(result["by_id"]["init"]["readiness"], "blocked")

    def test_merged_packet_with_the_marker_is_not_judged_again(self):
        self.marked()
        self.commit("merge init")
        result = self.status()
        self.assertEqual(result["by_id"]["init"]["stage"], "merged")
        self.assertNotIn("placeholder-artifact", self.codes(result, "init"))

    def test_other_kinds_are_unaffected(self):
        self.establish_baseline()
        self.packet("arch", kind="architecture", artifacts={"spec.md": UNESTABLISHED + "# S\n", "plan.md": "# P\n"})
        self.approve("arch", "spec.md", "plan.md")
        self.assertNotIn("placeholder-artifact", self.codes(self.status("--change", "arch"), "arch"))


class WriteScope(RepoCase):
    def test_path_escape_in_scope_is_invalid(self):
        for bad in ("../outside", "/etc/passwd", "src/../../x", "C:/win", "a\\b"):
            with self.subTest(bad=bad):
                self.packet("alpha", kind="repository", scope=[bad])
                self.assertIn("scope-path-escape", self.codes(self.status("--change", "alpha"), "alpha"))

    def test_changed_file_outside_scope_is_blocked(self):
        self.git("switch", "-q", "-c", "change/alpha")
        self.packet("alpha", kind="repository", scope=["src/**"])
        self.approve("alpha", "plan.md")
        self.commit("packet")
        self.write("src/ok.txt", "in scope\n")
        self.assertTrue(self.status()["ok"])
        self.write("docs/elsewhere.txt", "out of scope\n")
        self.assertFailed(self.status(), "outside declared write_scope")

    def test_committed_diff_is_checked_against_explicit_base_and_head(self):
        base = self.git("rev-parse", "HEAD")
        self.git("switch", "-q", "-c", "change/alpha")
        self.packet("alpha", kind="repository", scope=["src/**"])
        self.approve("alpha", "plan.md")
        self.write("lib/x.txt", "x\n")
        head = self.commit("candidate")
        result = self.status("--change", "alpha", "--base", base, "--head", head)
        self.assertEqual(result["changed_paths"], ["changes/alpha/change.json", "changes/alpha/plan.md", "lib/x.txt"])
        self.assertFailed(result, "lib/x.txt")

    def test_implementation_before_approval_is_blocked(self):
        self.git("switch", "-q", "-c", "change/alpha")
        self.packet("alpha", kind="repository", scope=["src/**"])
        self.commit("packet")
        self.write("changes/alpha/plan.md", "# Plan, still drafting\n")
        self.assertTrue(self.status()["ok"], "authoring the packet alone is allowed while pending")
        self.write("src/early.txt", "too early\n")
        self.assertFailed(self.status(), "before the change is ready")

    def test_repository_change_cannot_edit_established_baseline(self):
        self.establish_baseline()
        self.git("switch", "-q", "-c", "change/alpha")
        self.packet("alpha", kind="repository", scope=["spec.md"])
        self.approve("alpha", "plan.md")
        self.commit("packet")
        self.write("spec.md", "# Spec\nsneaky\n")
        self.assertFailed(self.status(), "may not modify established root")


class MergedPackets(RepoCase):
    """A packet on the baseline branch's tip is merged: its pull request closed it."""

    def branch_change(self, scope):
        self.git("switch", "-q", "-c", "change/alpha")
        self.packet("alpha", kind="repository", scope=scope)
        self.approve("alpha", "plan.md")
        self.commit("packet")

    def test_a_packet_on_the_baseline_is_merged(self):
        self.establish_baseline()
        result = self.status()
        change = result["by_id"]["init"]
        self.assertEqual((change["stage"], change["freshness"]), ("merged", "current"))
        self.assertTrue(result["ok"], result["failures"])

    def test_a_merged_packet_ignores_later_baseline_evolution(self):
        self.establish_baseline()
        self.write("spec.md", "# Spec\nv2\n")
        self.commit("the root moves on")
        result = self.status()
        self.assertEqual(self.codes(result, "init"), {"merged"})
        self.assertTrue(result["ok"], result["failures"])

    def test_a_merge_commit_marks_the_packet_merged(self):
        self.branch_change(["src/**"])
        self.assertEqual(self.status("--change", "alpha")["by_id"]["alpha"]["stage"], "implementation")
        self.git("switch", "-q", "main")
        self.git("merge", "-q", "--no-ff", "-m", "merge alpha", "change/alpha")
        self.assertEqual(self.status()["by_id"]["alpha"]["stage"], "merged")

    def test_a_merged_packet_cannot_be_worked_on_again(self):
        self.establish_baseline()
        self.git("switch", "-q", "-c", "change/init")
        self.write("src/late.txt", "x\n")
        result = self.status("--change", "init")
        self.assertEqual(result["by_id"]["init"]["readiness"], "blocked")
        self.assertFailed(result, "before the change is ready")

    def test_editing_a_merged_packet_outside_scope_fails(self):
        self.establish_baseline()
        self.branch_change(["src/**"])
        self.write("changes/init/plan.md", "# Plan, rewritten after merge\n")
        self.assertFailed(self.status("--change", "alpha"), "outside declared write_scope")

    def test_editing_a_merged_packet_inside_scope_passes(self):
        self.establish_baseline()
        self.branch_change(["changes/init/**"])
        self.write("changes/init/plan.md", "# Plan, corrected with approval\n")
        result = self.status("--change", "alpha")
        self.assertTrue(result["ok"], result["failures"])

    def test_deleting_merged_packets_needs_scope(self):
        self.establish_baseline()
        self.branch_change(["src/**"])
        self.git("rm", "-rq", "changes/init")
        self.commit("remove init")
        self.assertFailed(self.status("--change", "alpha"), "outside declared write_scope")
        self.set_scope("alpha", ["src/**", "changes/**"])
        self.run_repo("approve", "alpha", "plan.md", "--by", "owner")
        result = self.status("--change", "alpha")
        self.assertTrue(result["ok"], result["failures"])

    def test_a_deleted_id_can_be_reused(self):
        self.establish_baseline()
        self.git("rm", "-rq", "changes/init")
        self.commit("remove the merged packet")
        self.git("switch", "-q", "-c", "change/init")
        self.packet("init", kind="repository", scope=["src/**"])
        self.approve("init", "plan.md")
        self.commit("reuse the id")
        self.assertEqual(self.status("--change", "init")["by_id"]["init"]["stage"], "implementation")
        self.git("switch", "-q", "main")
        self.git("merge", "-q", "--squash", "change/init")
        self.commit("merge the reused id")
        self.assertEqual(self.status()["by_id"]["init"]["stage"], "merged")

    def test_without_a_baseline_ref_nothing_is_merged(self):
        self.establish_baseline()
        self.write("checks.json", json.dumps({"schema": 1, "baseline_branch": "trunk", "checks": []}))
        self.git("switch", "-q", "-c", "change/alpha")
        self.packet("alpha", kind="repository", scope=["src/**"])
        result = self.status("--change", "alpha")
        self.assertNotEqual(result["by_id"]["init"]["stage"], "merged")
        self.assertIn("diff-base", self.codes(result, "alpha"))

    def test_a_shallow_clone_still_sees_merged_packets(self):
        self.establish_baseline()
        self.commit("one more")
        clone = self.root.parent / "shallow"
        subprocess.run(
            ["git", "clone", "-q", "--depth", "1", f"file://{self.root}", str(clone)],
            env={**os.environ, **support.GIT_ENV}, check=True, capture_output=True,
        )
        result = self.status(cwd=clone)
        self.assertEqual(result["by_id"]["init"]["stage"], "merged")
        self.assertTrue(result["ok"], result["failures"])


class Surfaces(RepoCase):
    def test_shadowing_claude_md_is_reported(self):
        self.write("CLAUDE.md", "# My rules\n")
        self.commit("shadow")
        self.assertFailed(self.status(), "shadows AGENTS.md")

    def test_claude_md_importing_agents_md_is_accepted(self):
        self.write(".claude/CLAUDE.md", "@../AGENTS.md\n\nExtra notes.\n")
        self.write("CLAUDE.md", "See @AGENTS.md for the rules.\n")
        self.commit("imports")
        self.assertTrue(self.status()["ok"])

    def test_local_claude_md_warns_only_locally(self):
        self.write("CLAUDE.local.md", "# private\n")
        self.write(".gitignore", "CLAUDE.local.md\n")
        self.commit("ignore")
        local = self.run_repo("status", "--json")
        warnings = [s for s in json.loads(local.stdout)["surface"] if s["code"] == "instruction-shadowed-local"]
        self.assertEqual((local.returncode, len(warnings)), (0, 1))
        self.assertEqual([s for s in self.status()["surface"] if s["code"] == "instruction-shadowed-local"], [])

    def adapters(self):
        self.write(".agents/skills/demo/SKILL.md", "---\nname: demo\n---\n")
        (self.root / ".claude/skills").mkdir(parents=True)

    def test_symlink_adapter_is_valid(self):
        self.adapters()
        os.symlink("../../.agents/skills/demo", self.root / ".claude/skills/demo")
        self.assertTrue(self.status()["ok"])

    def test_plain_file_adapter_is_broken(self):
        self.adapters()
        self.write(".claude/skills/demo", "../../.agents/skills/demo")  # what core.symlinks=false checks out
        self.assertFailed(self.status(), "core.symlinks=false")

    def test_copied_skill_adapter_is_broken(self):
        self.adapters()
        self.write(".claude/skills/demo/SKILL.md", "---\nname: demo\n---\n")
        self.assertFailed(self.status(), "is a copy")

    def settings(self, command):
        hooks = {"hooks": {"PreToolUse": [{"matcher": "Edit", "hooks": [{"type": "command", "command": command}]}]}}
        self.write(".claude/settings.json", json.dumps(hooks))

    def hook_script(self, name="guard.py", executable=True):
        self.write(f"scripts/hooks/{name}", "#!/usr/bin/env python3\n")
        if executable:
            (self.root / "scripts/hooks" / name).chmod(0o755)

    def surface_codes(self, result):
        return {s["code"] for s in result["surface"]}

    def test_wired_executable_hook_is_valid(self):
        self.hook_script()
        self.settings('python3 "$CLAUDE_PROJECT_DIR"/scripts/hooks/guard.py claude')
        self.commit("hooks")
        result = self.status()
        self.assertTrue(result["ok"], result["failures"])
        self.assertNotIn("hook-adapter-missing", self.surface_codes(result))

    def test_hook_pointing_at_missing_script_is_broken(self):
        self.settings("python3 scripts/hooks/missing.py claude")
        self.commit("hooks")
        self.assertFailed(self.status(), "scripts/hooks/missing.py, which is not tracked")

    def test_hook_script_without_exec_bit_is_still_valid(self):
        self.hook_script(executable=False)  # the adapter runs it through python3
        self.settings("python3 scripts/hooks/guard.py claude")
        self.commit("hooks")
        self.assertTrue(self.status()["ok"])

    def test_invalid_settings_json_is_broken(self):
        self.write(".claude/settings.json", "{not json")
        self.commit("hooks")
        self.assertFailed(self.status(), "not valid JSON")

    def test_unwired_hook_script_is_informational(self):
        self.hook_script()
        self.commit("hooks")
        result = self.status()
        self.assertTrue(result["ok"])
        self.assertIn("hook-adapter-missing", self.surface_codes(result))


class RegisteredChecks(RepoCase):
    def configure(self):
        check = {"id": "control-plane", "argv": ["true"], "cwd": ".", "timeout_seconds": 5, "paths": ["scripts/**"]}
        self.write("checks.json", json.dumps({"schema": 1, "checks": [check]}))

    def warnings(self):
        return [s for s in self.status()["surface"] if s["code"] == "checks-unregistered"]

    def test_template_only_tree_does_not_warn(self):
        self.configure()
        self.write("scripts/repo.py", "")
        self.write("docs/notes.md", "")
        self.commit("template")
        self.assertEqual(self.warnings(), [])

    def test_product_files_with_only_control_plane_warn(self):
        self.configure()
        self.write("src/app.py", "print('hi')\n")
        self.commit("product")
        warnings = self.warnings()
        self.assertEqual(len(warnings), 1)
        self.assertIn("'src/app.py'", warnings[0]["message"])
        self.assertTrue(self.status()["ok"])

    def test_registered_product_check_silences_warning(self):
        self.configure()
        config = json.loads(self.read("checks.json"))
        config["checks"].append({"id": "unit", "argv": ["true"], "cwd": ".", "timeout_seconds": 5, "paths": ["src/**"]})
        self.write("checks.json", json.dumps(config))
        self.write("src/app.py", "")
        self.commit("product")
        self.assertEqual(self.warnings(), [])


class CheckWiring(RepoCase):
    WORKFLOW = "jobs:\n  a:\n    steps:\n      - run: python3 scripts/repo.py verify --group core $ARGS\n"

    def configure(self, groups, workflow):
        checks = [
            {"id": f"c-{g}", "group": g, "argv": ["true"], "cwd": ".", "timeout_seconds": 5, "paths": ["**"]}
            for g in groups
        ]
        self.write("checks.json", json.dumps({"schema": 1, "checks": checks}))
        self.write(".github/workflows/repository.yml", workflow)
        self.commit("wiring")

    def test_wired_groups_pass(self):
        self.configure(["core"], self.WORKFLOW)
        self.assertTrue(self.status()["ok"])

    def test_group_without_ci_job_fails(self):
        self.configure(["core", "web"], self.WORKFLOW)
        self.assertFailed(self.status(), "check group 'web' has no")

    def test_ci_job_for_undeclared_group_fails(self):
        self.configure([], self.WORKFLOW)
        self.assertFailed(self.status(), "does not declare")

    def test_a_group_run_directly_is_wired(self):
        self.configure(["core"], "jobs:\n  a:\n    steps:\n      - run: true\n")
        self.assertTrue(self.status()["ok"])

    def test_a_group_with_one_check_not_run_is_unwired(self):
        checks = [{"id": i, "group": "core", "argv": ["python3", "-c", i], "cwd": ".", "timeout_seconds": 5, "paths": ["**"]} for i in ("1", "2")]
        self.write("checks.json", json.dumps({"schema": 1, "checks": checks}))
        self.write(".github/workflows/repository.yml", "jobs:\n  a:\n    steps:\n      - run: python3 -c 1\n")
        self.commit("half wired")
        self.assertFailed(self.status(), "check group 'core' has no")

    def test_a_commented_direct_run_is_not_wired(self):
        self.configure(["core"], "jobs:\n  a:\n    steps:\n      # - run: true\n      - run: echo hi # true\n")
        self.assertFailed(self.status(), "check group 'core' has no")

    def test_invalid_checks_json_fails_status(self):
        self.write("checks.json", json.dumps({"schema": 1, "checks": [{"id": "x", "argv": "make test"}]}))
        self.commit("bad")
        self.assertFailed(self.status(), "never a shell string")


class PullRequestGate(RepoCase):
    def setUp(self):
        super().setUp()
        self.git("switch", "-q", "-c", "change/alpha")
        self.packet("alpha", kind="repository", scope=["src/**", "changes/**"])

    def pr(self, cid="alpha"):
        body = self.root.parent / "body.md"
        body.write_text(f"Change-ID: {cid}\n")
        return self.status("--branch=", "--pr-body", str(body), "--base", "main", "--head", "HEAD")

    def ready(self):
        self.approve("alpha", "plan.md")

    def test_unapproved_packet_fails(self):
        self.write("src/a.txt", "x\n")
        self.commit("draft")
        self.assertFailed(self.pr(), "before the change is ready")

    def test_ready_packet_passes_without_any_closure_record(self):
        self.ready()
        self.write("src/a.txt", "x\n")
        self.commit("implemented")
        result = self.pr()
        self.assertTrue(result["ok"], result["failures"])
        self.assertFalse(list(self.root.glob("changes/*/closure.json")))

    def test_template_packet_edit_passes(self):
        self.write("changes/_template/plan.md", "# Plan\n")
        self.ready()
        self.commit("template")
        result = self.pr()
        self.assertTrue(result["ok"], result["failures"])

    def test_a_carried_spec_must_be_merged_into_the_root(self):
        self.git("switch", "-q", "main")
        self.git("branch", "-q", "-D", "change/alpha")
        self.git("clean", "-fdq")
        self.establish_baseline()
        self.git("switch", "-q", "-c", "change/beh")
        self.packet("beh", kind="behavior", scope=["spec.md"], artifacts={"spec.md": "# Spec\nv2\n", "plan.md": "# P\n"})
        self.approve("beh", "spec.md", "plan.md")
        self.commit("approved")
        self.assertFailed(self.pr("beh"), "carried-unmerged")
        self.write("spec.md", "# Spec\nv2\n")
        self.commit("merge the accepted spec into the root")
        result = self.pr("beh")
        self.assertTrue(result["ok"], result["failures"])


if __name__ == "__main__":
    unittest.main()

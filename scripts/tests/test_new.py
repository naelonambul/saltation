"""`repo.py new`: mechanical packet creation."""

from __future__ import annotations

import json
import os
import shutil
import subprocess
import sys
import unittest

from scripts.tests.support import GIT_ENV, REPO_PY, RepoCase, digest

SHIPPED_PLAN = (REPO_PY.parents[1] / "changes" / "_template" / "plan.md").read_bytes()


class NewCase(RepoCase):
    def new(self, *args):
        return self.run_repo("new", *args)

    def change(self, cid):
        return json.loads(self.read(f"changes/{cid}/change.json"))

    def snapshot(self):
        base = self.root / "changes"
        if not base.exists():
            return None
        return {p.relative_to(base).as_posix(): p.read_bytes() if p.is_file() else None for p in sorted(base.rglob("*"))}

    def assertRefused(self, fragment, *args):
        before = self.snapshot()
        proc = self.new(*args)
        self.assertEqual(proc.returncode, 1, proc.stdout + proc.stderr)
        self.assertIn(fragment, proc.stderr)
        self.assertEqual(proc.stdout, "")
        self.assertEqual(self.snapshot(), before)


class CreateTests(NewCase):
    def test_repository_packet(self):
        head = self.git("rev-parse", "HEAD")
        proc = self.new("tooling", "--kind", "repository", "--title", "  Tooling  ", "--scope", "scripts/", "--scope", "docs/*.md", "--scope", "scripts/")
        self.assertEqual(proc.returncode, 0, proc.stderr)
        self.assertIn("created changes/tooling/change.json", proc.stdout)
        self.assertIn("python3 scripts/repo.py status --change tooling", proc.stdout)
        self.assertEqual(
            self.change("tooling"),
            {
                "schema": 1,
                "id": "tooling",
                "kind": "repository",
                "title": "Tooling",
                "base": {"ref": "main", "commit": head},
                "baseline": {},
                "write_scope": ["scripts/", "docs/*.md"],
                "approvals": [],
            },
        )
        self.assertEqual(sorted(p.name for p in (self.root / "changes" / "tooling").iterdir()), ["change.json", "plan.md"])
        self.assertEqual((self.root / "changes/tooling/plan.md").read_bytes(), SHIPPED_PLAN)
        result = self.status("--change", "tooling")
        self.assertEqual(result["exit"], 0, result["failures"])
        ch = result["by_id"]["tooling"]
        self.assertEqual(ch["stage"], "plan")
        self.assertEqual([r["code"] for r in ch["reasons"] if r["level"] == "error"], [])

    def test_repository_skeleton_preferred_over_shipped(self):
        self.write("changes/_template/plan.md", "# Local skeleton\n")
        self.write("checks.json", json.dumps({"schema": 1, "baseline_branch": "trunk", "checks": []}))
        self.commit("local template")
        self.assertEqual(self.new("x", "--kind", "repository", "--title", "x").returncode, 0)
        self.assertEqual(self.read("changes/x/plan.md"), "# Local skeleton\n")
        self.assertEqual(self.change("x")["base"]["ref"], "trunk")

    def test_shipped_skeleton_fallback_without_changes_dir(self):
        self.assertFalse((self.root / "changes").exists())
        proc = self.new("adopt", "--kind", "repository", "--title", "Adopt")
        self.assertEqual(proc.returncode, 0, proc.stderr)
        self.assertEqual((self.root / "changes/adopt/plan.md").read_bytes(), SHIPPED_PLAN)
        self.assertEqual(sorted(p.name for p in (self.root / "changes").iterdir()), ["adopt"])

    def test_behavior_on_established_baseline(self):
        self.establish_baseline()
        proc = self.new("feat", "--kind", "behavior", "--title", "Feature", "--scope", "src/")
        self.assertEqual(proc.returncode, 0, proc.stderr)
        data = self.change("feat")
        self.assertEqual(data["baseline"], {"intent.md": digest(self.read("intent.md")), "spec.md": digest(self.read("spec.md"))})
        self.assertEqual(self.read("changes/feat/spec.md"), self.read("spec.md"))
        self.assertFalse((self.root / "changes/feat/intent.md").exists())
        result = self.status("--change", "feat")
        self.assertEqual(result["exit"], 0, result["failures"])
        self.assertEqual(result["by_id"]["feat"]["stage"], "spec")

    def test_intent_with_and_without_spec(self):
        self.establish_baseline()
        cases = (("a", (), ["change.json", "intent.md", "plan.md"]), ("b", ("--with-spec",), ["change.json", "intent.md", "plan.md", "spec.md"]))
        for cid, extra, names in cases:
            with self.subTest(cid=cid):
                self.assertEqual(self.new(cid, "--kind", "intent", "--title", cid, *extra).returncode, 0)
                self.assertEqual(sorted(p.name for p in (self.root / "changes" / cid).iterdir()), names)
                for name in set(names) - {"change.json", "plan.md"}:
                    self.assertEqual(self.read(f"changes/{cid}/{name}"), self.read(name))
                result = self.status("--change", cid)
                self.assertEqual(result["exit"], 0, result["failures"])
                self.assertEqual(result["by_id"][cid]["stage"], "intent")
                shutil.rmtree(self.root / "changes" / cid)

    def test_implementation_copies_no_artifacts(self):
        self.establish_baseline()
        self.assertEqual(self.new("impl", "--kind", "implementation", "--title", "Impl").returncode, 0)
        self.assertEqual(sorted(p.name for p in (self.root / "changes/impl").iterdir()), ["change.json", "plan.md"])
        self.assertEqual(set(self.change("impl")["baseline"]), {"intent.md", "spec.md"})

    def test_product_init_on_unestablished_baseline(self):
        proc = self.new("init", "--kind", "product-init", "--title", "Init")
        self.assertEqual(proc.returncode, 0, proc.stderr)
        self.assertIn("unestablished marker", proc.stdout)
        self.assertEqual(self.change("init")["baseline"], {})
        self.assertEqual(self.read("changes/init/intent.md"), self.read("intent.md"))
        self.assertEqual(self.read("changes/init/spec.md"), self.read("spec.md"))


class RefusalTests(NewCase):
    def test_invalid_ids(self):
        for bad in ("Bad", "x-", "é", "a_b", "_template", "a" * 65):
            with self.subTest(bad=bad):
                self.assertRefused("invalid change id", bad, "--kind", "repository", "--title", "t")

    def test_unknown_kind(self):
        self.assertRefused("kind must be one of", "x", "--kind", "feature", "--title", "t")

    def test_empty_title(self):
        self.assertRefused("title must not be empty", "x", "--kind", "repository", "--title", "   ")

    def test_existing_packet_left_unchanged(self):
        self.packet("x", kind="repository")
        self.commit("x")
        self.assertRefused("already exists", "x", "--kind", "repository", "--title", "t")

    def test_existing_file_at_packet_path(self):
        self.write("changes/x", "not a directory\n")
        self.assertRefused("already exists", "x", "--kind", "repository", "--title", "t")

    def test_escaping_scope(self):
        for bad in ("../outside", "/abs", "a/./b", "a\\b"):
            with self.subTest(bad=bad):
                self.assertRefused("--scope", "x", "--kind", "repository", "--title", "t", "--scope", "ok/", "--scope", bad)

    def test_with_spec_not_allowed(self):
        self.establish_baseline()
        for kind in ("behavior", "implementation", "repository", "product-init"):
            with self.subTest(kind=kind):
                self.assertRefused("--with-spec", "x", "--kind", kind, "--title", "t", "--with-spec")

    def test_inheriting_kind_on_unestablished_baseline(self):
        for kind in ("intent", "behavior", "implementation", "incident", "architecture"):
            with self.subTest(kind=kind):
                self.assertRefused("start with a product-init change", "x", "--kind", kind, "--title", "t")

    def test_inheriting_kind_with_missing_root(self):
        self.establish_baseline()
        (self.root / "spec.md").unlink()
        self.assertRefused("missing or unestablished", "x", "--kind", "behavior", "--title", "t")

    def test_product_init_on_established_baseline(self):
        self.establish_baseline()
        self.assertRefused("product-init needs", "x", "--kind", "product-init", "--title", "t")

    def test_no_skeleton_anywhere(self):
        lone = self.root.parent / "lone" / "scripts"
        lone.mkdir(parents=True)
        shutil.copy(REPO_PY, lone / "repo.py")
        before = self.snapshot()
        proc = subprocess.run(
            [sys.executable, str(lone / "repo.py"), "new", "x", "--kind", "repository", "--title", "t"],
            cwd=self.root, env={**os.environ, **GIT_ENV}, capture_output=True, text=True,
        )
        self.assertEqual(proc.returncode, 1, proc.stdout + proc.stderr)
        self.assertIn("no plan skeleton", proc.stderr)
        self.assertEqual(self.snapshot(), before)

    def test_product_init_with_missing_root(self):
        (self.root / "intent.md").unlink()
        self.assertRefused("product-init needs", "x", "--kind", "product-init", "--title", "t")


class CarriedRootScopeTests(NewCase):
    def test_product_init_scope_holds_both_roots(self):
        self.assertEqual(self.new("init", "--kind", "product-init", "--title", "t", "--scope", "src/**").returncode, 0)
        self.assertEqual(self.change("init")["write_scope"], ["src/**", "intent.md", "spec.md"])

    def test_with_spec_scope_holds_spec_only(self):
        self.establish_baseline()
        self.assertEqual(self.new("x", "--kind", "incident", "--title", "t", "--with-spec").returncode, 0)
        self.assertEqual(self.change("x")["write_scope"], ["spec.md"])

    def test_repository_scope_gains_nothing(self):
        self.assertEqual(self.new("x", "--kind", "repository", "--title", "t").returncode, 0)
        self.assertEqual(self.change("x")["write_scope"], [])

    def test_hand_passed_root_is_not_duplicated(self):
        self.assertEqual(self.new("init", "--kind", "product-init", "--title", "t", "--scope", "spec.md").returncode, 0)
        self.assertEqual(self.change("init")["write_scope"], ["spec.md", "intent.md"])

    def test_quick_start_reaches_a_passing_pull_request(self):
        check = {"id": "check", "group": "core", "argv": [sys.executable, "-c", "print('ok')"], "cwd": ".",
                 "timeout_seconds": 30, "paths": ["**"], "requires": []}
        self.write(".gitignore", ".evidence/\n")
        self.write("checks.json", json.dumps({"schema": 1, "baseline_branch": "main", "checks": [check]}))
        self.commit("checks")
        self.git("switch", "-q", "-c", "change/init")
        self.assertEqual(self.new("init", "--kind", "product-init", "--title", "t").returncode, 0)
        self.write("changes/init/intent.md", "# Intent\nreal\n")
        self.write("changes/init/spec.md", "# Spec\nreal\n")
        self.write("changes/init/plan.md", "# Plan\nreal\n")
        for name in ("intent.md", "spec.md", "plan.md"):
            proc = self.run_repo("approve", "init", name, "--by", "owner")
            self.assertEqual(proc.returncode, 0, proc.stderr)
        self.commit("packet")
        self.write("intent.md", self.read("changes/init/intent.md"))
        self.write("spec.md", self.read("changes/init/spec.md"))
        self.commit("baseline")
        proc = self.run_repo("verify", "--change", "init", "--full")
        self.assertEqual(proc.returncode, 0, proc.stdout + proc.stderr)
        self.write("../body.md", "Change-ID: init\n")
        result = self.status("--branch=", "--pr-body", str(self.root.parent / "body.md"), "--base", "main", "--head", "HEAD")
        self.assertEqual(result["by_id"]["init"]["readiness"], "ready")
        self.assertTrue(result["ok"], result["failures"])


if __name__ == "__main__":
    unittest.main()

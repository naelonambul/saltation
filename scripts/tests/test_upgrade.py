from __future__ import annotations

import hashlib
import json
import os
import re
import subprocess
import tempfile
import unittest
from pathlib import Path

from scripts.tests.support import GIT_ENV, REPO_PY, RepoCase, checks_json

PROJECT = REPO_PY.parents[1]
MIGRATIONS = ".agents/skills/template-upgrade/references/migrations.md"
OWNED = ".agents/skills/template-upgrade/references/owned-files.txt"
BASELINE_ERRORS = {
    "adapter-broken", "artifact-not-allowed", "baseline-established", "baseline-unestablished", "checks-invalid",
    "checks-unwired", "diff-base", "hook-broken", "instruction-shadowed", "instructions-missing", "invalid-approval",
    "invalid-change", "missing-upstream", "not-ready", "out-of-order", "out-of-scope", "placeholder-artifact",
    "scope-path-escape", "stale-approval", "stale-baseline", "unbound-scope",
}
COPY_V1 = ("scripts/repo.py", "scripts/hooks/", "scripts/tests/", "changes/README.md", ".agents/skills/sdlc-artifacts/", ".claude/skills/sdlc-artifacts")
COPY_V3 = COPY_V1 + (".agents/skills/template-upgrade/", ".claude/skills/template-upgrade")
MERGE = ("AGENTS.md", "checks.json")
OWNED_V3 = sorted([
    ".agents/skills/sdlc-artifacts/SKILL.md", ".agents/skills/template-upgrade/SKILL.md", MIGRATIONS, OWNED,
    ".claude/skills/sdlc-artifacts", ".claude/skills/template-upgrade", "changes/README.md",
    "scripts/hooks/guard.py", "scripts/repo.py", "scripts/tests/test_a.py",
])


def lists_source(copy, merge, version) -> str:
    return f"UPGRADE_COPY = {tuple(copy)!r}\nUPGRADE_MERGE = {tuple(merge)!r}\nVERSION = {version!r}\n"


def snapshot(root: Path) -> dict:
    out = {}
    for path in sorted(root.rglob("*")):
        rel = str(path.relative_to(root))
        if path.is_symlink():
            out[rel] = ("link", os.readlink(path))
        elif path.is_file():
            out[rel] = (os.access(path, os.X_OK), path.read_bytes())
    return out


class Template:
    def __init__(self, root: Path):
        self.root = root
        root.mkdir()
        self.git("init", "-q", "-b", "main")
        self.commits = {}

    def git(self, *args) -> str:
        proc = subprocess.run(["git", *args], cwd=self.root, env={**os.environ, **GIT_ENV}, capture_output=True, text=True)
        if proc.returncode != 0:
            raise AssertionError(f"git {args}: {proc.stderr}")
        return proc.stdout.strip()

    def write(self, rel: str, text: str, executable: bool = False):
        path = self.root / rel
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(text)
        path.chmod(0o755 if executable else 0o644)

    def link(self, rel: str, target: str):
        path = self.root / rel
        path.parent.mkdir(parents=True, exist_ok=True)
        path.symlink_to(target)

    def commit(self, name: str) -> str:
        self.git("add", "-A")
        self.git("commit", "-q", "--allow-empty", "-m", name)
        self.commits[name] = self.git("rev-parse", "HEAD")
        return self.commits[name]


def build_template(root: Path) -> Template:
    t = Template(root)
    t.write("changes/mig-old/change.json", "{}\n")
    t.write("scripts/repo.py", "VERSION = 'v0'\n")
    t.commit("c0")
    t.write("scripts/repo.py", lists_source(COPY_V1, MERGE, "v1"))
    t.write("scripts/tests/test_a.py", "a1\n")
    t.write("scripts/tests/gone.py", "gone\n")
    t.write("scripts/hooks/guard.py", "g1\n")
    t.write("changes/README.md", "r1\n")
    t.write(".agents/skills/sdlc-artifacts/SKILL.md", "s1\n")
    t.link(".claude/skills/sdlc-artifacts", "../../.agents/skills/sdlc-artifacts")
    t.write("AGENTS.md", "agents1\n")
    t.write("checks.json", checks_json())
    t.write("README.md", "template readme\n")
    t.commit("c1")
    t.write("changes/mig-one/change.json", "{}\n")
    t.write("scripts/tests/test_a.py", "a2\n")
    t.commit("c2")
    t.write("scripts/repo.py", lists_source(COPY_V3, MERGE, "v3"))
    t.write("changes/mig-two/change.json", "{}\n")
    t.write(MIGRATIONS, "# Template migrations\n\n## mig-old\n\nold\n\n## mig-two\n\ntwo\n\n## mig-one\n\none\n")
    t.write(OWNED, "".join(p + "\n" for p in OWNED_V3))
    t.write(".agents/skills/template-upgrade/SKILL.md", "u1\n")
    t.link(".claude/skills/template-upgrade", "../../.agents/skills/template-upgrade")
    t.write("scripts/hooks/guard.py", "g1\n", executable=True)
    (t.root / "scripts/tests/gone.py").unlink()
    t.write("AGENTS.md", "agents3\n")
    t.write("README.md", "template readme 3\n")
    t.commit("c3")
    return t


class Upgrade(RepoCase):
    def setUp(self):
        super().setUp()
        self.template = build_template(Path(self._tmp.name) / "template")
        self.c = self.template.commits
        self.install("c1")
        self.write("scripts/tests/test_local.py", "mine\n")
        self.commit("adopt c1")

    def install(self, name: str):
        commit = self.c[name]
        listing = self.template.git("ls-tree", "-r", commit, "--", "scripts", "changes/README.md", ".agents", ".claude")
        for line in listing.splitlines():
            meta, rel = line.split("\t", 1)
            mode = meta.split()[0]
            data = subprocess.run(["git", "show", f"{commit}:{rel}"], cwd=self.template.root, capture_output=True, check=True).stdout
            path = self.root / rel
            path.parent.mkdir(parents=True, exist_ok=True)
            if os.path.lexists(path):
                path.unlink()
            if mode == "120000":
                path.symlink_to(data.decode())
            else:
                path.write_bytes(data)
                path.chmod(0o755 if mode == "100755" else 0o644)

    def apply_c3(self, pin: dict):
        self.install("c3")
        self.git("rm", "-q", "--ignore-unmatch", "scripts/tests/gone.py")
        self.write_checks(template=pin)
        self.commit("apply c3")

    def upgrade(self, *args):
        return self.run_repo("upgrade", "--template", str(self.template.root), *args)

    def report(self, *args) -> str:
        proc = self.upgrade(*args)
        self.assertEqual(proc.returncode, 0, proc.stdout + proc.stderr)
        return proc.stdout

    def lines(self, out: str, name: str) -> list[str]:
        block = re.search(rf"(?m)^{name}: .*\n((?:  .*\n)*)", out)
        self.assertIsNotNone(block, out)
        return [line.strip() for line in block.group(1).splitlines()]

    def pin(self, out: str) -> dict:
        return json.loads(re.search(r'"template": (\{.*\})', out).group(1))

    def surface(self, code: str) -> list[str]:
        return [r["message"] for r in self.status()["surface"] if r["code"] == code]

    def test_verified_base_reports_every_section(self):
        out = self.report("--to", self.c["c3"], "--from", self.c["c1"])
        self.assertIn(f"base:   {self.c['c1']}  7 of 7 template-owned copy files match", out)
        self.assertIn("drift: none", out)
        self.assertEqual(self.lines(out, "copy"), [
            "added    .agents/skills/template-upgrade/SKILL.md",
            f"added    {MIGRATIONS}",
            f"added    {OWNED}",
            "added    .claude/skills/template-upgrade (symlink)",
            "changed  scripts/hooks/guard.py (executable) (mode 100644 -> 100755)",
            "changed  scripts/repo.py",
            "removed  scripts/tests/gone.py",
            "changed  scripts/tests/test_a.py",
        ])
        merge = self.lines(out, "merge")
        self.assertEqual(len(merge), 2, merge)
        self.assertIn(f"diff {self.c['c1']} {self.c['c3']} -- AGENTS.md", merge[0])
        self.assertTrue(merge[1].startswith("git merge-file -p AGENTS.md "), merge)
        self.assertEqual(self.lines(out, "local"), ["scripts/tests/test_local.py"])
        self.assertNotIn("README.md", "\n".join(self.lines(out, "copy") + merge))

    def test_migrations_are_those_added_after_the_base_in_history_order(self):
        out = self.report("--to", self.c["c3"], "--from", self.c["c1"])
        self.assertEqual([line.split()[0] for line in self.lines(out, "migrations")], ["mig-one", "mig-two"])
        out = self.report("--to", self.c["c3"], "--from", self.c["c2"])
        self.assertEqual([line.split()[0] for line in self.lines(out, "migrations")], ["mig-two"])

    def test_the_pin_is_the_default_base(self):
        self.write_checks(template={"commit": self.c["c1"], "digest": "sha256:" + "0" * 64})
        self.commit("pin")
        self.assertIn(f"base:   {self.c['c1']}", self.report("--to", self.c["c3"]))

    def test_without_a_base_it_lists_the_best_candidates_and_stops(self):
        proc = self.upgrade("--to", self.c["c3"])
        self.assertEqual(proc.returncode, 1, proc.stdout)
        self.assertIn("pass --from", proc.stdout)
        self.assertEqual(proc.stdout.splitlines()[1:], [f"  {self.c['c1']}  7 of 7 match"])
        for rel in ("scripts/tests/test_a.py", "scripts/tests/gone.py", "changes/README.md", "scripts/repo.py"):
            self.write(rel, "edited\n")
        self.commit("heavy local edits")
        proc = self.upgrade("--to", self.c["c3"])
        self.assertIn(f"  {self.c['c1']}  3 of 7 match", proc.stdout)
        self.assertNotIn(self.c["c0"], proc.stdout)

    def test_staged_drift_is_reported_but_an_already_applied_file_is_not(self):
        self.write("scripts/tests/test_a.py", "a1 local\n")
        self.git("rm", "-q", "--cached", "scripts/hooks/guard.py")
        self.git("add", "-A", "scripts/tests")
        out = self.report("--to", self.c["c3"], "--from", self.c["c1"])
        self.assertEqual(self.lines(out, "drift"), ["scripts/hooks/guard.py: missing", "scripts/tests/test_a.py: differs from the base"])
        self.install("c3")
        self.git("add", "-A")
        out = self.report("--to", self.c["c3"], "--from", self.c["c1"])
        self.assertEqual(self.lines(out, "drift"), [])
        self.assertEqual(self.lines(out, "copy"), ["removed  scripts/tests/gone.py"])

    def test_an_added_file_that_collides_is_flagged(self):
        self.write(".agents/skills/template-upgrade/SKILL.md", "mine\n")
        out = self.report("--to", self.c["c3"], "--from", self.c["c1"])
        self.assertIn("added    .agents/skills/template-upgrade/SKILL.md (collides with an existing adopter file; resolve it first)", self.lines(out, "copy"))

    def test_carried_files_get_a_recipe_or_a_removal_and_the_pin_follows(self):
        self.write("scripts/tests/test_a.py", "a1 local\n")
        self.write_checks(template={"commit": self.c["c1"], "digest": "sha256:" + "0" * 64, "carried": ["scripts/tests/test_a.py", "scripts/tests/gone.py"]})
        self.commit("carry")
        out = self.report("--to", self.c["c3"])
        self.assertEqual(self.lines(out, "drift"), [])
        carried = self.lines(out, "carried")
        self.assertEqual(carried[0], "scripts/tests/gone.py: removed upstream; delete it, or keep it as a local file outside the pin")
        self.assertTrue(carried[1].startswith("scripts/tests/test_a.py: git merge-file -p scripts/tests/test_a.py "), carried)
        self.assertEqual([line for line in self.lines(out, "copy") if "tests/" in line], [])
        pin = self.pin(out)
        self.assertEqual(pin["carried"], ["scripts/tests/test_a.py"])
        self.install("c3")
        self.write("scripts/tests/test_a.py", "a3 merged with local\n")
        self.write_checks(template=pin)
        self.commit("apply with carried")
        self.assertEqual(self.surface("template-drift") + self.surface("checks-invalid"), [])

    def test_a_carried_path_the_template_never_owned_is_refused(self):
        self.write_checks(template={"commit": self.c["c1"], "digest": "sha256:" + "0" * 64, "carried": ["scripts/tests/test_local.py"]})
        proc = self.upgrade("--to", self.c["c3"])
        self.assertEqual(proc.returncode, 1)
        self.assertIn("does not own: scripts/tests/test_local.py", proc.stderr)

    def test_a_base_that_is_not_an_ancestor_is_refused(self):
        self.template.git("switch", "-q", "-c", "side", self.c["c0"])
        self.template.write("scripts/repo.py", lists_source(COPY_V1, MERGE, "side"))
        side = self.template.commit("side")
        proc = self.upgrade("--to", self.c["c3"], "--from", side)
        self.assertEqual(proc.returncode, 1)
        self.assertIn("is not an ancestor of target", proc.stderr)

    def test_bad_inputs_exit_one(self):
        missing = self.run_repo("upgrade", "--template", str(self.root.parent / "nowhere"), "--to", "main")
        self.assertIn("is not a Git clone", missing.stderr)
        unknown = self.upgrade("--to", "no-such-ref", "--from", self.c["c1"])
        self.assertIn("'no-such-ref' is not a commit", unknown.stderr)
        old = self.upgrade("--to", self.c["c1"], "--from", self.c["c1"])
        self.assertIn("predates upgrade support", old.stderr)
        self.write_checks(template={"commit": 123, "digest": "x", "carried": [{}]})
        bad_pin = self.upgrade("--to", self.c["c3"])
        self.assertIn("checks.json template: template.commit must be a full 40-hex commit id", bad_pin.stderr)
        self.write("checks.json", "{not json")
        bad_json = self.upgrade("--to", self.c["c3"], "--from", self.c["c1"])
        self.assertIn("checks.json is not valid JSON", bad_json.stderr)
        shallow = Path(self._tmp.name) / "shallow"
        subprocess.run(["git", "clone", "-q", "--depth", "1", f"file://{self.template.root}", str(shallow)], check=True, capture_output=True)
        self.assertIn("is a shallow clone", self.run_repo("upgrade", "--template", str(shallow), "--to", "HEAD").stderr)
        for proc in (missing, unknown, old, bad_pin, bad_json):
            self.assertEqual(proc.returncode, 1, proc.stderr)

    def test_unreadable_target_lists_are_refused(self):
        self.template.write("scripts/repo.py", "UPGRADE_COPY = tuple(x for x in 'ab')\nUPGRADE_MERGE = ()\n")
        broken = self.template.commit("c4")
        proc = self.upgrade("--to", broken, "--from", self.c["c1"])
        self.assertEqual(proc.returncode, 1)
        self.assertIn("cannot read the upgrade lists", proc.stderr)

    def test_the_report_writes_nothing(self):
        self.write("scripts/tests/test_a.py", "dirty\n")
        self.write("untracked.txt", "x\n")
        before = (snapshot(self.root), snapshot(self.template.root))
        self.report("--to", self.c["c3"], "--from", self.c["c1"])
        self.upgrade("--to", self.c["c3"])
        self.assertEqual((snapshot(self.root), snapshot(self.template.root)), before)

    def test_the_printed_pin_is_what_status_accepts_and_drift_is_named(self):
        pin = self.pin(self.report("--to", self.c["c3"], "--from", self.c["c1"]))
        self.assertEqual(pin, {"commit": self.c["c3"], "digest": pin["digest"]})
        self.apply_c3(pin)
        lines = []
        for record in self.git("ls-files", "-s").splitlines():
            meta, rel = record.split("\t", 1)
            mode, blob, _ = meta.split()
            if rel in OWNED_V3:
                lines.append(f"{rel}\0{mode}\0{blob}\n")
        self.assertEqual(len(lines), len(OWNED_V3))
        self.assertEqual(pin["digest"], "sha256:" + hashlib.sha256("".join(sorted(lines)).encode()).hexdigest())
        self.assertEqual(self.surface("template-drift"), [])
        self.write("scripts/tests/test_local.py", "local edit\n")
        self.write("scripts/tests/test_other_local.py", "new local\n")
        self.git("add", "-A")
        self.assertEqual(self.surface("template-drift"), [])
        self.write("scripts/tests/test_a.py", "edited\n")
        os.chmod(self.root / "scripts/hooks/guard.py", 0o644)
        self.git("add", "-A")
        drift = self.surface("template-drift")
        self.assertEqual(len(drift), 1)
        self.assertIn(": scripts/hooks/guard.py, scripts/tests/test_a.py. Restore each file", drift[0])
        self.write_checks(template={**pin, "carried": ["scripts/tests/test_local.py"]})
        self.assertIn("must be tracked files listed in", self.surface("checks-invalid")[0])

    def test_status_without_the_manifest_is_invalid(self):
        pin = self.pin(self.report("--to", self.c["c3"], "--from", self.c["c1"]))
        self.apply_c3(pin)
        self.git("rm", "-q", OWNED)
        self.assertIn(f"{OWNED} is not tracked", self.surface("checks-invalid")[0])

    def test_a_manifest_naming_outside_paths_or_carried_itself_is_invalid(self):
        pin = self.pin(self.report("--to", self.c["c3"], "--from", self.c["c1"]))
        self.apply_c3(pin)
        self.write_checks(template={**pin, "carried": [OWNED]})
        self.assertIn(f"template.carried: {OWNED!r} is not a template-owned copy path", self.surface("checks-invalid"))
        self.write_checks(template=pin)
        self.write(OWNED, "../outside\n" + (self.root / OWNED).read_text())
        self.git("add", "-A")
        self.assertIn("lists paths outside the copy group: ../outside", self.surface("checks-invalid")[0])

    def test_drift_without_history_points_at_the_report(self):
        pin = self.pin(self.report("--to", self.c["c3"], "--from", self.c["c1"]))
        self.apply_c3(pin)
        self.write_checks(template={**pin, "digest": "sha256:" + "1" * 64})
        drift = self.surface("template-drift")
        self.assertIn(f"upgrade --template <template clone> --to {self.c['c3']}", drift[0])


def template_itself(root: Path) -> bool:
    try:
        config = json.loads((root / "checks.json").read_text())
    except (OSError, ValueError):
        return True
    return not (isinstance(config, dict) and "template" in config)


class TemplateItself(unittest.TestCase):
    def test_a_pinned_repository_is_an_adopter(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            self.assertTrue(template_itself(root))
            (root / "checks.json").write_text(json.dumps({"schema": 1, "checks": []}))
            self.assertTrue(template_itself(root))
            (root / "checks.json").write_text(json.dumps({"schema": 1, "checks": [], "template": {"commit": "0" * 40}}))
            self.assertFalse(template_itself(root))


@unittest.skipUnless(template_itself(PROJECT), "checks.json has a template pin: these are the template's own invariants, and the pin covers the adopter")
class Completeness(unittest.TestCase):
    def tracked(self) -> list:
        out = subprocess.run(["git", "ls-files", "-z"], cwd=PROJECT, capture_output=True, text=True, check=True).stdout
        return [p for p in out.split("\0") if p]

    def test_every_listed_path_exists_in_the_template(self):
        from scripts import repo
        tracked = self.tracked()
        for entry in repo.UPGRADE_COPY + repo.UPGRADE_MERGE:
            present = entry in tracked or (entry.endswith("/") and any(p.startswith(entry) for p in tracked))
            self.assertTrue(present, f"{entry} is listed for upgrades but not tracked")

    def test_the_owned_manifest_lists_exactly_the_copy_group(self):
        from scripts import repo
        expected = sorted(p for p in self.tracked() if repo.in_copy_group(p, repo.UPGRADE_COPY))
        listed = (PROJECT / OWNED).read_text().splitlines()
        self.assertEqual(listed, expected, f"regenerate {OWNED}: one tracked copy-group path per line, sorted")

    def test_every_new_status_error_has_a_migration_entry(self):
        codes = set(re.findall(r'(?:Reason\(|\.add\()\s*"error",\s*"([a-z-]+)"', REPO_PY.read_text()))
        self.assertIn("checks-invalid", codes, "the error-code scan found nothing; fix its pattern")
        text = (PROJECT / MIGRATIONS).read_text()
        missing = sorted(code for code in codes - BASELINE_ERRORS if f"`{code}`" not in text)
        self.assertEqual(missing, [], f"add a section to {MIGRATIONS} naming each new status error")

    def test_every_migration_heading_names_a_change_packet(self):
        ids = re.findall(r"(?m)^## (\S+)\s*$", (PROJECT / MIGRATIONS).read_text())
        self.assertGreater(len(ids), 0)
        for cid in ids:
            self.assertTrue((PROJECT / "changes" / cid / "change.json").is_file(), f"{MIGRATIONS}: ## {cid} names no changes/{cid}/")


if __name__ == "__main__":
    unittest.main()

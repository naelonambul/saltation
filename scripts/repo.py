from __future__ import annotations

import argparse
import ast
import datetime as _dt
import hashlib
import json
import os
import re
import shlex
import shutil
import signal
import subprocess
import sys
import time
from dataclasses import dataclass, field
from pathlib import Path, PurePosixPath

SCHEMA = 1
CHANGES = "changes"
TEMPLATE_PACKET = "_template"
CHAIN = ("intent.md", "spec.md", "plan.md")
PLAN = CHAIN[-1]
BASELINE = ("intent.md", "spec.md")
UNESTABLISHED = "<!-- sdlc:baseline-unestablished -->"
ID_RE = re.compile(r"^[a-z0-9](?:[a-z0-9-]{0,62}[a-z0-9])?$")
DIGEST_RE = re.compile(r"^sha256:[0-9a-f]{64}$")

KINDS = {
    "product-init": ({"intent.md", "spec.md", "plan.md"}, set()),
    "intent": ({"intent.md", "plan.md"}, {"spec.md"}),
    "behavior": ({"spec.md", "plan.md"}, set()),
    "implementation": ({"plan.md"}, set()),
    "incident": ({"plan.md"}, {"spec.md"}),
    "architecture": ({"plan.md"}, {"spec.md"}),
    "repository": ({"plan.md"}, set()),
}
CHANGE_KEYS = {"schema", "id", "kind", "title", "base", "baseline", "write_scope", "approvals"}
APPROVAL_KEYS = {"artifact", "sha256", "write_scope_sha256", "by", "at", "note"}


class GitError(RuntimeError):
    pass


def git(root: Path, *args: str, binary: bool = False, check: bool = True):
    proc = subprocess.run(["git", *args], cwd=root, capture_output=True)
    if check and proc.returncode != 0:
        raise GitError(f"git {' '.join(args)}: {proc.stderr.decode(errors='replace').strip()}")
    if not check and proc.returncode != 0:
        return None
    return proc.stdout if binary else proc.stdout.decode()


def git_paths(root: Path, *args: str) -> list[str]:
    return list(filter(None, git(root, *args).split("\0")))


def repo_root() -> Path:
    return Path(git(Path.cwd(), "rev-parse", "--show-toplevel").strip())


def sha256_bytes(data: bytes) -> str:
    return "sha256:" + hashlib.sha256(data).hexdigest()


def file_digest(path: Path) -> str | None:
    return sha256_bytes(path.read_bytes()) if path.is_file() else None


def scope_pattern_error(pattern) -> str | None:
    if not isinstance(pattern, str) or not pattern:
        return "must be a non-empty string"
    if pattern.startswith("/") or "\\" in pattern or re.match(r"^[A-Za-z]:", pattern):
        return "must be a relative POSIX path"
    parts = pattern.rstrip("/").split("/")
    if any(p in ("", ".", "..") for p in parts):
        return "must not contain empty, '.' or '..' segments"
    return None


def glob_regex(pattern: str) -> re.Pattern:
    out, i = "", 0
    while i < len(pattern):
        if pattern.startswith("**/", i):
            out, i = out + "(?:.*/)?", i + 3
        elif pattern.startswith("**", i):
            out, i = out + ".*", i + 2
        elif pattern[i] == "*":
            out, i = out + "[^/]*", i + 1
        elif pattern[i] == "?":
            out, i = out + "[^/]", i + 1
        else:
            out, i = out + re.escape(pattern[i]), i + 1
    if pattern.endswith("/"):
        out += ".*"
    return re.compile(out + r"\Z")


def matches_any(path: str, patterns) -> bool:
    return any(glob_regex(p).match(path) for p in patterns)


@dataclass
class Context:
    root: Path
    ci: bool = False
    baseline_branch: str = "main"
    base: str | None = None
    head: str | None = None
    _shallow: bool | None = None
    _baseline_ref: str | None | bool = False
    _merged_ids: set[str] | None = None

    @property
    def merged_ids(self) -> set[str]:
        if self._merged_ids is None:
            self._merged_ids = set()
            if self.baseline_ref is not None:
                listing = git(self.root, "ls-tree", "-r", "--name-only", self.baseline_ref, "--", f"{CHANGES}/")
                for rel in listing.splitlines():
                    parts = rel.split("/")
                    if len(parts) == 3 and parts[2] == "change.json" and parts[1] != TEMPLATE_PACKET:
                        self._merged_ids.add(parts[1])
        return self._merged_ids

    @property
    def shallow(self) -> bool:
        if self._shallow is None:
            self._shallow = git(self.root, "rev-parse", "--is-shallow-repository").strip() == "true"
        return self._shallow

    @property
    def baseline_ref(self) -> str | None:
        if self._baseline_ref is False:
            self._baseline_ref = None
            for ref in (f"refs/remotes/origin/{self.baseline_branch}", f"refs/heads/{self.baseline_branch}"):
                if git(self.root, "rev-parse", "--verify", "--quiet", ref + "^{commit}", check=False):
                    self._baseline_ref = ref
                    break
        return self._baseline_ref


def load_config(root: Path) -> dict:
    path = root / "checks.json"
    if not path.is_file():
        return {}
    try:
        config = json.loads(path.read_text())
    except ValueError as exc:
        return {"problem": f"checks.json is not valid JSON: {exc}"}
    return config if isinstance(config, dict) else {"problem": "checks.json must be a JSON object"}


def baseline_branch(root: Path) -> str:
    return load_config(root).get("baseline_branch", "main")


@dataclass
class Reason:
    level: str
    code: str
    message: str

    def as_dict(self):
        return {"level": self.level, "code": self.code, "message": self.message}


@dataclass
class ChangeStatus:
    id: str
    kind: str | None = None
    stage: str | None = None
    freshness: str = "current"
    approval: str = "none"
    readiness: str = "blocked"
    artifacts: dict = field(default_factory=dict)
    reasons: list = field(default_factory=list)
    change: dict | None = None

    def add(self, level: str, code: str, message: str):
        self.reasons.append(Reason(level, code, message))

    @property
    def errors(self):
        return [r for r in self.reasons if r.level == "error"]

    def as_dict(self):
        return {
            "id": self.id,
            "kind": self.kind,
            "stage": self.stage,
            "freshness": self.freshness,
            "approval": self.approval,
            "readiness": self.readiness,
            "artifacts": self.artifacts,
            "reasons": [r.as_dict() for r in self.reasons],
        }


def parse_time(value):
    if not isinstance(value, str):
        return None
    try:
        parsed = _dt.datetime.fromisoformat(value.replace("Z", "+00:00"))
    except ValueError:
        return None
    return parsed if parsed.tzinfo else None


def validate_change(st: ChangeStatus, data) -> bool:
    if not isinstance(data, dict):
        st.add("error", "invalid-change", "change.json must be a JSON object")
        return False
    ok = True
    extra = set(data) - CHANGE_KEYS
    if extra:
        st.add("error", "invalid-change", f"unknown change.json keys (derived state is never stored): {sorted(extra)}")
        ok = False
    if data.get("schema") != SCHEMA:
        st.add("error", "invalid-change", f"schema must be {SCHEMA}")
        ok = False
    if data.get("id") != st.id:
        st.add("error", "invalid-change", f"id {data.get('id')!r} does not match directory {st.id!r}")
        ok = False
    if data.get("kind") not in KINDS:
        st.add("error", "invalid-change", f"kind must be one of {sorted(KINDS)}")
        ok = False
    base = data.get("base")
    if not isinstance(base, dict) or not isinstance(base.get("ref"), str) or not isinstance(base.get("commit"), str):
        st.add("error", "invalid-change", "base must be {\"ref\": str, \"commit\": str}")
        ok = False
    scope = data.get("write_scope")
    if not isinstance(scope, list):
        st.add("error", "invalid-change", "write_scope must be a list of path patterns")
        ok = False
    else:
        for pattern in scope:
            problem = scope_pattern_error(pattern)
            if problem:
                st.add("error", "scope-path-escape", f"write_scope entry {pattern!r} {problem}")
                ok = False
    if not isinstance(data.get("approvals", []), list):
        st.add("error", "invalid-change", "approvals must be a list")
        ok = False
    if "baseline" in data and not isinstance(data["baseline"], dict):
        st.add("error", "invalid-change", "baseline must map artifact name to digest")
        ok = False
    return ok


def scope_digest(scope: list) -> str:
    return sha256_bytes(json.dumps(sorted(scope), separators=(",", ":")).encode())


def is_placeholder(kind: str, packet_dir: Path, artifact: str) -> bool:
    return kind == "product-init" and artifact in BASELINE and UNESTABLISHED.encode() in (packet_dir / artifact).read_bytes()


def check_approvals(st: ChangeStatus, data: dict, packet_dir: Path, chain: list) -> tuple[dict, bool, bool]:
    approved_at = {}
    stale_approval = False
    unbound_scope = False
    for claim in data.get("approvals", []):
        if not isinstance(claim, dict) or set(claim) - APPROVAL_KEYS:
            st.add("error", "invalid-approval", f"approval claims only use keys {sorted(APPROVAL_KEYS)}")
            continue
        artifact = claim.get("artifact")
        if artifact not in chain:
            st.add("error", "invalid-approval", f"approval names {artifact!r}, which is not in this change's chain")
            continue
        digest = claim.get("sha256")
        if not digest:
            st.add("pending", "bare-approval", f"approval for {artifact} is not bound to an artifact digest; it counts as none")
            continue
        when = parse_time(claim.get("at"))
        if not DIGEST_RE.match(str(digest)) or not claim.get("by") or when is None:
            st.add("error", "invalid-approval", f"approval for {artifact} needs sha256, by, and a timezone-aware ISO 8601 at")
            continue
        if digest != file_digest(packet_dir / artifact):
            stale_approval = True
            st.add("error", "stale-approval", f"approval for {artifact} is bound to different bytes; re-approve the current content")
            continue
        if artifact == PLAN:
            bound = claim.get("write_scope_sha256")
            if bound is None:
                unbound_scope = True
            elif bound != scope_digest(data.get("write_scope", [])):
                stale_approval = True
                st.add("error", "stale-approval", f"write_scope changed since {artifact} was approved; the owner re-runs repo.py approve {st.id} {artifact}")
                continue
        approved_at[artifact] = min(when, approved_at.get(artifact, when))
    return approved_at, stale_approval, unbound_scope


def check_order(st: ChangeStatus, chain: list, approved_at: dict) -> None:
    st.stage = "implementation"
    first_open = None
    for name in chain:
        if name not in approved_at:
            first_open = name
            st.stage = name[: -len(".md")]
            break
    if first_open:
        later = [n for n in chain[chain.index(first_open) + 1:] if n in approved_at]
        if later:
            st.add("error", "out-of-order", f"{', '.join(later)} approved before upstream {first_open}")
        elif not any(r.code == "stale-approval" for r in st.reasons):
            st.add("pending", "awaiting-approval", f"{first_open} awaits a digest-bound owner approval")
    times = [approved_at[n] for n in chain if n in approved_at]
    if times != sorted(times):
        st.add("error", "out-of-order", "approval timestamps do not follow intent -> spec -> plan order; re-approve the later artifacts in order")


def evaluate_change(ctx: Context, change_id: str) -> ChangeStatus:
    st = ChangeStatus(change_id)
    packet_dir = ctx.root / CHANGES / change_id
    if not ID_RE.match(change_id):
        st.add("error", "invalid-change", "change ids use lowercase letters, digits and inner hyphens")
        return st
    try:
        data = json.loads((packet_dir / "change.json").read_text())
    except FileNotFoundError:
        st.add("error", "invalid-change", "missing change.json")
        return st
    except json.JSONDecodeError as exc:
        st.add("error", "invalid-change", f"change.json is not valid JSON: {exc}")
        return st
    if not validate_change(st, data):
        return st
    st.change = data
    st.kind = data["kind"]
    if change_id in ctx.merged_ids:
        st.stage = "merged"
        st.add("pending", "merged", f"merged on {ctx.baseline_branch!r}; record follow-up work as a new change")
        return st
    required, optional = KINDS[st.kind]
    present = {name for name in CHAIN if (packet_dir / name).is_file()}
    for name in sorted(required - present):
        st.add("pending", "missing-artifact", f"{name} is required for kind {st.kind!r} and does not exist yet")
    for name in sorted(present - required - optional):
        st.add("error", "artifact-not-allowed", f"{name} is not allowed in a {st.kind!r} change")
    chain = [name for name in CHAIN if name in required or name in present]

    approved_at, stale_approval, unbound_scope = check_approvals(st, data, packet_dir, chain)

    st.artifacts = {
        name: {
            "sha256": file_digest(packet_dir / name),
            "approval": "unverified" if name in approved_at else "none",
        }
        for name in chain
    }
    st.approval = "unverified" if chain and all(n in approved_at for n in chain) else "none"

    check_order(st, chain, approved_at)

    placeholders = [n for n in approved_at if is_placeholder(st.kind, packet_dir, n)]
    if placeholders:
        st.add("error", "placeholder-artifact", f"approved {placeholders} still carry the unestablished marker")
    if unbound_scope:
        st.add("error", "unbound-scope", f"the {PLAN} approval does not bind write_scope; the owner re-runs repo.py approve {change_id} {PLAN}")

    stale_baseline = check_baseline(ctx, st, data, present)
    if stale_baseline or stale_approval:
        st.freshness = "stale"
    blocking = [r for r in st.reasons if r.level in ("error", "pending")]
    st.readiness = "ready" if not blocking and st.stage == "implementation" else "blocked"
    return st


def root_state(ctx: Context, name: str):
    path = ctx.root / name
    if not path.is_file():
        return None, None
    data = path.read_bytes()
    return sha256_bytes(data), UNESTABLISHED.encode() not in data


def baseline_state(ctx: Context, name: str):
    data = git(ctx.root, "show", f"{ctx.baseline_ref}:{name}", binary=True, check=False) if ctx.baseline_ref else None
    if data is None:
        return root_state(ctx, name)
    return sha256_bytes(data), UNESTABLISHED.encode() not in data


def check_baseline(ctx: Context, st: ChangeStatus, data: dict, present: set) -> bool:
    stale = False
    declared = data.get("baseline", {}) or {}
    if st.kind == "repository":
        if declared:
            st.add("error", "invalid-change", "repository changes inherit no product baseline; leave baseline empty")
        return False
    for name in BASELINE:
        digest, established = root_state(ctx, name)
        local = file_digest(ctx.root / CHANGES / st.id / name) if name in present else None
        if digest is None:
            st.add("error", "missing-upstream", f"root {name} does not exist")
            continue
        if local is not None:
            digest, established = baseline_state(ctx, name)
        if local is not None and digest == local:
            continue
        if st.kind == "product-init":
            if established:
                st.add("error", "baseline-established", f"root {name} is already established; use another change kind")
            continue
        if not established:
            st.add("error", "baseline-unestablished", f"root {name} is not established; start with a product-init change")
            continue
        if not declared.get(name):
            st.add("error", "missing-upstream", f"baseline.{name} must record the root digest this change was authored against")
        elif declared.get(name) != digest:
            stale = True
            st.add("error", "stale-baseline", f"root {name} changed since this change was authored ({str(declared.get(name))[:19]}... -> {digest[:19]}...)")
    return stale


def established_at(ctx: Context, commit: str, name: str) -> bool:
    data = git(ctx.root, "show", f"{commit}:{name}", binary=True, check=False)
    return data is not None and UNESTABLISHED.encode() not in data


def change_ids(root: Path) -> list[str]:
    base = root / CHANGES
    if not base.is_dir():
        return []
    return sorted(p.name for p in base.iterdir() if p.is_dir() and p.name != TEMPLATE_PACKET)


def parse_pr_change_ids(body: str) -> list[str]:
    body = re.sub(r"<!--.*?-->", "", body, flags=re.S)
    return sorted(re.findall(r"(?mi)^\s*Change-ID:\s*(\S+)\s*$", body))


def current_branch(root: Path) -> str | None:
    out = git(root, "symbolic-ref", "--quiet", "--short", "HEAD", check=False)
    return out.strip() if out else None


def resolve_base(ctx: Context, change: dict | None) -> tuple[str | None, str]:
    head = ctx.head or "HEAD"
    if ctx.base:
        mb = git(ctx.root, "merge-base", ctx.base, head, check=False)
        return (mb.strip(), f"merge-base with --base {ctx.base}") if mb else (None, f"--base {ctx.base} is unresolvable")
    if ctx.baseline_ref:
        mb = git(ctx.root, "merge-base", ctx.baseline_ref, head, check=False)
        if mb:
            return mb.strip(), f"merge-base with {ctx.baseline_ref}"
    declared = (change or {}).get("base", {}).get("commit")
    if declared and git(ctx.root, "rev-parse", "--verify", "--quiet", declared + "^{commit}", check=False):
        return declared, "change.json base.commit"
    return None, "no resolvable base"


def changed_paths(ctx: Context, base: str) -> list[str]:
    diff = ("diff", "--name-only", "--no-renames", "-z", base)
    if ctx.head:
        return sorted(set(git_paths(ctx.root, *diff, ctx.head)))
    return sorted({*git_paths(ctx.root, *diff), *git_paths(ctx.root, "ls-files", "--others", "--exclude-standard", "-z")})


@dataclass
class Active:
    id: str | None = None
    source: str | None = None
    errors: list = field(default_factory=list)


def resolve_active(ctx: Context, explicit: str | None, pr_body: str | None, branch: str | None, ids: list[str]) -> Active:
    active = Active()
    sources = {}
    if explicit:
        sources["--change"] = explicit
    if pr_body is not None:
        found = parse_pr_change_ids(pr_body)
        if not found:
            active.errors.append("pull request body has no `Change-ID: <id>` line")
        elif len(found) > 1:
            active.errors.append(f"pull request body names several Change-IDs {found}; one PR is one change")
        else:
            sources["PR Change-ID"] = found[0]
    branch = branch if branch is not None else current_branch(ctx.root)
    if branch and branch.startswith("change/"):
        sources["branch"] = branch[len("change/"):].split("/")[0]
    if len(set(sources.values())) > 1:
        active.errors.append("change identity sources disagree: " + ", ".join(f"{k}={v}" for k, v in sources.items()))
        return active
    if sources:
        active.id = next(iter(sources.values()))
        active.source = " + ".join(sources)
    elif not active.errors:
        base, _ = resolve_base(ctx, None)
        touched = set()
        if base:
            for path in changed_paths(ctx, base):
                parts = path.split("/")
                if len(parts) > 2 and parts[0] == CHANGES and parts[1] != TEMPLATE_PACKET:
                    touched.add(parts[1])
        if len(touched) == 1:
            active.id, active.source = touched.pop(), "only touched packet"
        elif len(touched) > 1:
            active.errors.append(f"several packets touched {sorted(touched)}; pass --change")
    if active.id and active.id not in ids:
        active.errors.append(f"change {active.id!r} (from {active.source}) has no packet under {CHANGES}/")
    return active


def check_scope(ctx: Context, st: ChangeStatus) -> list[str]:
    base, how = resolve_base(ctx, st.change)
    if ctx.shallow and not ctx.base:
        st.add("error", "diff-base", "shallow history cannot establish the change base; fetch full history")
        return []
    if base is None:
        st.add("error", "diff-base", f"cannot determine the change base ({how}); pass --base")
        return []
    paths = changed_paths(ctx, base)
    own = f"{CHANGES}/{st.id}/"
    scope = list((st.change or {}).get("write_scope", []))
    outside = [p for p in paths if not p.startswith(own) and not matches_any(p, scope)]
    if outside:
        st.add("error", "out-of-scope", f"changed paths outside declared write_scope ({how}): {outside}")
    if st.kind == "repository":
        touched = [p for p in paths if p in BASELINE and established_at(ctx, base, p)]
        if touched:
            st.add("error", "out-of-scope", f"repository changes may not modify established root {touched}")
    implementation = [p for p in paths if not p.startswith(own)]
    if implementation and any(r.level == "pending" for r in st.reasons):
        st.add("error", "not-ready", f"{len(implementation)} path(s) outside the packet changed before the change is ready")
    if st.errors:
        st.readiness = "blocked"
    return paths


def surface_findings(ctx: Context) -> list[Reason]:
    tracked = set(git_paths(ctx.root, "ls-files", "-z"))
    return instruction_findings(ctx, tracked) + adapter_findings(ctx.root) + hook_findings(ctx.root, tracked)


def instruction_findings(ctx: Context, tracked: set) -> list[Reason]:
    findings = []
    root = ctx.root
    if "AGENTS.md" not in tracked:
        findings.append(Reason("error", "instructions-missing", "canonical AGENTS.md is not tracked"))
    for path in sorted(tracked):
        name = PurePosixPath(path).name
        if name == "CLAUDE.local.md":
            findings.append(Reason("error", "instruction-shadowed", f"{path} is tracked; CLAUDE.local.md is personal and shadows AGENTS.md"))
        if name != "CLAUDE.md":
            continue
        scope_dir = PurePosixPath(path).parent
        if scope_dir.name == ".claude":
            scope_dir = scope_dir.parent
        text = (root / path).read_text(errors="replace") if (root / path).is_file() else ""
        imported = False
        for ref in re.findall(r"(?<!\S)@(\S*AGENTS\.md)\b", text):
            target = os.path.normpath((PurePosixPath(path).parent / ref).as_posix())
            if target in tracked and PurePosixPath(target).parent in (scope_dir, *scope_dir.parents):
                imported = True
        if not imported:
            findings.append(Reason("error", "instruction-shadowed", f"{path} shadows AGENTS.md; add an `@AGENTS.md` import or remove it"))
    if not ctx.ci:
        for dirpath, dirnames, filenames in os.walk(root):
            dirnames[:] = [d for d in dirnames if d not in (".git", "node_modules")]
            if "CLAUDE.local.md" in filenames:
                rel = Path(dirpath, "CLAUDE.local.md").relative_to(root).as_posix()
                if rel not in tracked:
                    findings.append(Reason("warn", "instruction-shadowed-local", f"{rel} disables the native AGENTS.md fallback for that scope on this machine"))
    return findings


def adapter_findings(root: Path) -> list[Reason]:
    findings = []
    skills = root / ".agents" / "skills"
    adapters = root / ".claude" / "skills"
    canonical = sorted(p.name for p in skills.iterdir() if (p / "SKILL.md").is_file()) if skills.is_dir() else []
    if adapters.is_dir() or adapters.is_symlink():
        for entry in sorted(adapters.iterdir()):
            expected = f"../../.agents/skills/{entry.name}"
            if entry.is_symlink():
                if os.readlink(entry) != expected or not (entry / "SKILL.md").is_file():
                    findings.append(Reason("error", "adapter-broken", f".claude/skills/{entry.name} must be a symlink to {expected}"))
            elif entry.is_file():
                findings.append(Reason("error", "adapter-broken", f".claude/skills/{entry.name} is a plain file, not a symlink; Git symlinks are disabled (core.symlinks=false)"))
            else:
                findings.append(Reason("error", "adapter-broken", f".claude/skills/{entry.name} is a copy, not a symlink to the canonical skill body"))
    for name in canonical:
        if not (adapters / name).is_symlink():
            findings.append(Reason("info", "adapter-missing", f".agents/skills/{name} has no .claude/skills adapter"))
    return findings


HOOK_SETTINGS = ".claude/settings.json"
HOOK_DIR = "scripts/hooks"


def hook_commands(settings) -> list[str]:
    out = []
    for event in (settings.get("hooks") or {}).values() if isinstance(settings, dict) else []:
        for matcher in event if isinstance(event, list) else []:
            for hook in (matcher.get("hooks") or []) if isinstance(matcher, dict) else []:
                if isinstance(hook, dict) and isinstance(hook.get("command"), str):
                    out.append(hook["command"])
    return out


def hook_findings(root: Path, tracked: set) -> list[Reason]:
    findings = []
    scripts = sorted(p for p in tracked if p.startswith(HOOK_DIR + "/") and p.endswith(".py"))
    if HOOK_SETTINGS not in tracked:
        for script in scripts:
            findings.append(Reason("info", "hook-adapter-missing", f"{script} is not wired by any {HOOK_SETTINGS} hook"))
        return findings
    try:
        settings = json.loads((root / HOOK_SETTINGS).read_text())
    except (OSError, json.JSONDecodeError) as exc:
        return [Reason("error", "hook-broken", f"{HOOK_SETTINGS} is not valid JSON: {exc}")]
    referenced = set()
    for command in hook_commands(settings):
        for script in re.findall(rf"{re.escape(HOOK_DIR)}/[\w./-]+", command):
            referenced.add(script)
            if script not in tracked:
                findings.append(Reason("error", "hook-broken", f"{HOOK_SETTINGS} runs {script}, which is not tracked"))
    for script in scripts:
        if script not in referenced:
            findings.append(Reason("info", "hook-adapter-missing", f"{script} is not wired by any {HOOK_SETTINGS} hook"))
    return findings


TEMPLATE_PATHS = (
    "scripts/", "changes/", ".agents/", ".claude/", ".github/", "docs/", ".evidence/",
    "AGENTS.md", "README.md", "REVIEW.md", "LICENSE", "intent.md", "spec.md", "checks.json",
    ".gitignore", ".gitattributes", "comment-exceptions.json",
)
GATE_ROLES = ("build", "test", "lint", "format", "types", "boundaries", "comments")
EXCEPTIONS = "comment-exceptions.json"
COMMENT_CATEGORIES = ("license", "directive", "public-api", "external-constraint")
EXCEPTION_KEYS = {"path", "text", "category", "reason", "evidence"}
UPGRADE_COPY = (
    "scripts/repo.py", "scripts/__init__.py", "scripts/README.md", "scripts/hooks/", "scripts/tests/",
    "changes/README.md", "changes/_template/",
    ".agents/skills/change-execution/", ".agents/skills/repository-quality/", ".agents/skills/sdlc-artifacts/",
    ".agents/skills/template-upgrade/", ".agents/skills/verification-map/",
    ".claude/skills/change-execution", ".claude/skills/repository-quality", ".claude/skills/sdlc-artifacts",
    ".claude/skills/template-upgrade", ".claude/skills/verification-map",
)
UPGRADE_MERGE = (
    "AGENTS.md", "REVIEW.md", "checks.json", ".claude/settings.json", ".github/workflows/repository.yml",
    ".github/pull_request_template.md", ".gitignore", ".gitattributes",
)
MIGRATIONS = ".agents/skills/template-upgrade/references/migrations.md"
OWNED = ".agents/skills/template-upgrade/references/owned-files.txt"
TEMPLATE_KEYS = {"commit", "digest", "carried"}
COMMIT_RE = re.compile(r"^[0-9a-f]{40}$")
MISSING = ("missing", "missing")


def product_paths(tracked: set) -> list[str]:
    return sorted(p for p in tracked if not any(p == t or (t.endswith("/") and p.startswith(t)) for t in TEMPLATE_PATHS))


CHECK_KEYS = {"id", "group", "argv", "cwd", "timeout_seconds", "paths", "requires"}


def validate_checks(config: dict) -> list[str]:
    if "problem" in config:
        return [config["problem"]]
    problems = []
    checks = config.get("checks")
    if config.get("schema") != SCHEMA or not isinstance(checks, list):
        return [f"checks.json needs \"schema\": {SCHEMA} and a \"checks\" list"]
    seen = set()
    for i, check in enumerate(checks):
        where = f"checks[{i}]"
        if not isinstance(check, dict):
            problems.append(f"{where} must be an object")
            continue
        extra = set(check) - CHECK_KEYS
        if extra:
            problems.append(f"{where} has unknown keys {sorted(extra)}")
        cid = check.get("id")
        if not isinstance(cid, str) or not ID_RE.match(cid) or cid in seen:
            problems.append(f"{where}.id must be a unique lowercase id")
        else:
            seen.add(cid)
        argv = check.get("argv")
        if not isinstance(argv, list) or not argv or not all(isinstance(a, str) and a for a in argv):
            problems.append(f"{where}.argv must be a non-empty array of strings (never a shell string)")
        cwd = check.get("cwd")
        if not isinstance(cwd, str) or (cwd != "." and scope_pattern_error(cwd)) or any(c in cwd for c in "*?"):
            problems.append(f"{where}.cwd must be '.' or a relative directory")
        timeout = check.get("timeout_seconds")
        if not isinstance(timeout, (int, float)) or isinstance(timeout, bool) or timeout <= 0:
            problems.append(f"{where}.timeout_seconds must be a positive number")
        paths = check.get("paths")
        if not isinstance(paths, list) or not paths or any(scope_pattern_error(p) for p in paths):
            problems.append(f"{where}.paths must be a non-empty list of relative path patterns")
        requires = check.get("requires", [])
        if not isinstance(requires, list) or not all(isinstance(r, str) and r for r in requires):
            problems.append(f"{where}.requires must be a list of executable names")
        if not isinstance(check.get("group", "core"), str):
            problems.append(f"{where}.group must be a string")
    return problems + gate_problems(config.get("gates"), seen) + pin_problems(config.get("template"))


def pin_problems(pin) -> list[str]:
    if pin is None:
        return []
    if not isinstance(pin, dict) or not {"commit", "digest"} <= set(pin) <= TEMPLATE_KEYS:
        return ["checks.json template must be an object with commit, digest and an optional carried list; take it from `repo.py upgrade`"]
    problems = []
    if not isinstance(pin["commit"], str) or not COMMIT_RE.match(pin["commit"]):
        problems.append("template.commit must be a full 40-hex commit id")
    if not isinstance(pin["digest"], str) or not DIGEST_RE.match(pin["digest"]):
        problems.append("template.digest must be sha256:<64 hex>")
    carried = pin.get("carried", [])
    if not isinstance(carried, list) or not all(isinstance(c, str) for c in carried):
        return problems + ["template.carried must be a list of paths"]
    for path in carried:
        if not in_copy_group(path, UPGRADE_COPY) or path == OWNED:
            problems.append(f"template.carried: {path!r} is not a template-owned copy path")
    if len(set(carried)) != len(carried):
        problems.append("template.carried lists a path twice")
    return problems


def in_copy_group(path: str, entries) -> bool:
    return any(path == e or (e.endswith("/") and path.startswith(e)) for e in entries)


def index_entries(root: Path) -> dict:
    out = {}
    for record in git(root, "ls-files", "-s", "-z").split("\0"):
        if record:
            meta, rel = record.split("\t", 1)
            mode, blob, _stage = meta.split()
            out[rel] = (mode, blob)
    return out


def tree_entries(root: Path, commit: str, paths) -> dict:
    out = {}
    for record in git(root, "ls-tree", "-r", "-z", commit, "--", *paths).split("\0"):
        if record:
            meta, rel = record.split("\t", 1)
            mode, _kind, blob = meta.split()
            out[rel] = (mode, blob)
    return out


def pin_digest(entries: dict) -> str:
    lines = "".join(f"{path}\0{mode}\0{blob}\n" for path, (mode, blob) in sorted(entries.items()))
    return sha256_bytes(lines.encode())


def owned_manifest(text: str) -> list[str]:
    return [line for line in text.splitlines() if line]


def drifted_files(ctx: Context, pin: dict, current: dict) -> list[str] | None:
    log = git(ctx.root, "log", "--format=%H", "-S", pin["digest"], "--", "checks.json", check=False)
    for commit in reversed((log or "").split()):
        text = git(ctx.root, "show", f"{commit}:checks.json", check=False)
        if text is None or pin["digest"] not in text:
            continue
        then = tree_entries(ctx.root, commit, list(current))
        if pin_digest({rel: then.get(rel, MISSING) for rel in current}) != pin["digest"]:
            return None
        return sorted(rel for rel, entry in current.items() if then.get(rel, MISSING) != entry)
    return None


def template_findings(ctx: Context, config: dict) -> list[Reason]:
    pin = config.get("template")
    if pin is None:
        return []
    manifest = git(ctx.root, "show", f":{OWNED}", binary=True, check=False)
    if manifest is None:
        return [Reason("error", "checks-invalid", f"checks.json template is set but {OWNED} is not tracked; restore it from the pinned template")]
    owned = owned_manifest(manifest.decode(errors="replace"))
    outside = [rel for rel in owned if not in_copy_group(rel, UPGRADE_COPY)]
    if outside:
        return [Reason("error", "checks-invalid", f"{OWNED} lists paths outside the copy group: {', '.join(outside[:5])}")]
    index = index_entries(ctx.root)
    carried = pin.get("carried", [])
    bad = [c for c in carried if c not in owned or c not in index]
    if bad:
        return [Reason("error", "checks-invalid", f"template.carried: {', '.join(bad)} must be tracked files listed in {OWNED}")]
    current = {rel: index.get(rel, MISSING) for rel in owned if rel not in carried}
    if pin_digest(current) == pin["digest"]:
        return []
    named = drifted_files(ctx, pin, current)
    which = (
        f": {', '.join(named)}" if named
        else f"; list them with `python3 <template clone>/scripts/repo.py upgrade --template <template clone> --to {pin['commit']}`"
    )
    return [Reason(
        "error", "template-drift",
        f"template-owned files differ from the pinned template {pin['commit'][:12]}{which}. "
        "Restore each file, or record a deliberate edit in template.carried (template-upgrade skill)",
    )]


def gate_problems(gates, registered: set) -> list[str]:
    if gates is None:
        return []
    if not isinstance(gates, dict):
        return ["checks.json gates must be an object mapping each role to a check id or {\"none\": \"<reason>\"}"]
    problems = []
    for role, value in gates.items():
        where = f"gates.{role}"
        if role not in GATE_ROLES:
            problems.append(f"{where}: unknown gate role {role!r}; the roles are {', '.join(GATE_ROLES)}")
        elif isinstance(value, str):
            if value not in registered:
                problems.append(f"{where}: {value!r} is not a registered check id")
        elif not (isinstance(value, dict) and set(value) == {"none"} and isinstance(value["none"], str) and value["none"].strip()):
            problems.append(f"{where} must be a registered check id or {{\"none\": \"<reason>\"}} with a non-empty reason and no other key")
    return problems


def route(ctx: Context, checks: list[dict], full: bool) -> tuple[dict, dict]:
    if full:
        return {c["id"]: (True, "full suite requested") for c in checks}, {"mode": "full"}
    base, how = resolve_base(ctx, None)
    if base is None:
        return {c["id"]: (True, f"full suite: {how}") for c in checks}, {"mode": "full", "reason": how}
    paths = changed_paths(ctx, base)
    unmapped = [p for p in paths if not any(matches_any(p, c["paths"]) for c in checks)]
    facts = {"mode": "routed", "base": base, "base_source": how, "changed_paths": paths, "unmapped_paths": unmapped}
    if unmapped:
        return {c["id"]: (True, f"full suite: {len(unmapped)} unmapped changed path(s)") for c in checks}, facts
    decisions = {}
    for c in checks:
        hits = [p for p in paths if matches_any(p, c["paths"])]
        decisions[c["id"]] = (True, f"{len(hits)} changed path(s) match") if hits else (False, "no changed path matches this check's paths")
    return decisions, facts


def tool_version(executable: str) -> str | None:
    try:
        proc = subprocess.run([executable, "--version"], capture_output=True, text=True, timeout=15)
    except (OSError, subprocess.TimeoutExpired):
        return None
    out = (proc.stdout or proc.stderr).strip().splitlines()
    return out[0] if out else None


def log_digest(path: Path) -> str | None:
    return file_digest(path)


def run_check(ctx: Context, check: dict, log_path: Path) -> dict:
    record = {
        "id": check["id"],
        "group": check.get("group", "core"),
        "argv": check["argv"],
        "cwd": check["cwd"],
        "timeout_seconds": check["timeout_seconds"],
        "exit_code": None,
        "duration_seconds": None,
        "log": None,
        "log_sha256": None,
        "tools": {},
    }
    cwd = (ctx.root / check["cwd"]).resolve()
    if not cwd.is_dir():
        return {**record, "status": "blocked", "reason": f"working directory {check['cwd']!r} does not exist"}
    needed = list(dict.fromkeys([*check.get("requires", []), check["argv"][0]]))
    missing = [t for t in needed if shutil.which(t) is None and not ((cwd / t).is_file() and os.access(cwd / t, os.X_OK))]
    if missing:
        return {**record, "status": "blocked", "reason": f"required executable(s) not found: {missing}"}
    record["tools"] = {t: tool_version(t) for t in check.get("requires", [])}
    log_path.parent.mkdir(parents=True, exist_ok=True)
    started = time.monotonic()
    with open(log_path, "wb") as log:
        try:
            proc = subprocess.Popen(check["argv"], cwd=cwd, stdout=log, stderr=subprocess.STDOUT, stdin=subprocess.DEVNULL, start_new_session=True)
        except OSError as exc:
            return {**record, "status": "blocked", "reason": f"could not start the check: {exc}"}
        try:
            code = proc.wait(timeout=check["timeout_seconds"])
            timed_out = False
        except subprocess.TimeoutExpired:
            timed_out = True
            try:
                os.killpg(proc.pid, signal.SIGTERM)
            except ProcessLookupError:
                pass
            try:
                code = proc.wait(timeout=15)
            except subprocess.TimeoutExpired:
                try:
                    os.killpg(proc.pid, signal.SIGKILL)
                except ProcessLookupError:
                    pass
                code = proc.wait()
    record["duration_seconds"] = round(time.monotonic() - started, 3)
    record["exit_code"] = None if timed_out else code
    record["log"] = log_path.relative_to(ctx.root).as_posix() if log_path.is_relative_to(ctx.root) else str(log_path)
    record["log_sha256"] = log_digest(log_path)
    if timed_out:
        return {**record, "status": "failed", "reason": f"timed out after {check['timeout_seconds']}s"}
    if code != 0:
        return {**record, "status": "failed", "reason": f"exit code {code}"}
    if record["log_sha256"] is None:
        return {**record, "status": "blocked", "reason": "check exited 0 but its evidence log is missing"}
    return {**record, "status": "passed", "reason": "exit code 0"}


def candidate_digest(root: Path, _cid: str | None = None) -> str:
    gitlinks = {}
    for entry in git_paths(root, "ls-files", "-s", "-z"):
        meta, rel = entry.split("\t", 1)
        mode, oid, _stage = meta.split()
        if mode == "160000":
            gitlinks[rel] = oid
    digest = hashlib.sha256()
    for rel in sorted(set(git_paths(root, "ls-files", "-co", "--exclude-standard", "-z"))):
        path = root / rel
        if rel.startswith(".evidence/"):
            continue
        if rel in gitlinks:
            mode, data = "160000", gitlinks[rel].encode()
        elif path.is_symlink():
            mode, data = "120000", os.readlink(path).encode()
        elif path.is_file():
            mode, data = ("100755" if os.access(path, os.X_OK) else "100644"), path.read_bytes()
        else:
            continue
        digest.update(f"{mode} {rel}\0{sha256_bytes(data)}\n".encode())
    return "sha256:" + digest.hexdigest()


def source_identity(ctx: Context) -> dict:
    commit = git(ctx.root, "rev-parse", "HEAD", check=False)
    dirty = git(ctx.root, "status", "--porcelain", "--untracked-files=normal", check=False)
    return {"commit": commit.strip() if commit else None, "dirty": bool(dirty and dirty.strip())}


WORKFLOW = Path(".github/workflows/repository.yml")


def gate_findings(ctx: Context, config: dict) -> list[Reason]:
    product = product_paths(set(git_paths(ctx.root, "ls-files", "-z")))
    if not product:
        return []
    example = f"{len(product)} product path(s) are tracked (for example {product[0]!r})"
    if not config:
        return [Reason("error", "checks-missing", f"{example} but checks.json is missing; restore it and decide every gate (see the repository-quality skill)")]
    gates = config.get("gates") or {}
    missing = [role for role in GATE_ROLES if role not in gates]
    if not missing:
        return []
    return [Reason(
        "error", "gates-undecided",
        f"{example} but checks.json gates leaves {', '.join(missing)} undecided; decide each role in checks.json `gates`: "
        "a registered check id or {\"none\": \"<reason>\"}; see the repository-quality skill",
    )]


def exception_findings(ctx: Context) -> list[Reason]:
    path = ctx.root / EXCEPTIONS
    if not path.is_file():
        return []
    return [Reason("error", "comment-exceptions-invalid", f"{EXCEPTIONS}: {p}") for p in exception_problems(ctx.root, path)]


def exception_problems(root: Path, path: Path) -> list[str]:
    try:
        register = json.loads(path.read_text())
    except (ValueError, UnicodeDecodeError) as exc:
        return [f"not valid JSON: {exc}"]
    if not isinstance(register, dict) or register.get("schema") != SCHEMA or not isinstance(register.get("exceptions"), list):
        return [f"needs \"schema\": {SCHEMA} and an \"exceptions\" list"]
    tracked = set(git_paths(root, "ls-files", "-z"))
    problems, seen = [], set()
    for i, entry in enumerate(register["exceptions"]):
        where = f"exceptions[{i}]"
        if not isinstance(entry, dict):
            problems.append(f"{where} must be an object")
            continue
        extra = set(entry) - EXCEPTION_KEYS
        if extra:
            problems.append(f"{where} has unknown keys {sorted(extra)}")
        fields = {k: entry.get(k) for k in ("path", "text", "category", "reason")}
        blank = [k for k, v in fields.items() if not isinstance(v, str) or not v.strip()]
        if blank:
            problems.append(f"{where} needs a non-empty {', '.join(blank)}")
            continue
        if fields["category"] not in COMMENT_CATEGORIES:
            problems.append(f"{where}.category {fields['category']!r} is not one of {', '.join(COMMENT_CATEGORIES)}; our own code gets no comment, so fix the code instead")
        evidence = entry.get("evidence")
        if fields["category"] == "external-constraint" and not (isinstance(evidence, str) and evidence.strip()):
            problems.append(f"{where}: an external-constraint needs evidence naming the outside source")
        text = fields["text"].strip()
        key = (fields["path"], text)
        if key in seen:
            problems.append(f"{where} is a duplicate of an earlier entry for {fields['path']}")
        seen.add(key)
        if fields["path"] not in tracked:
            problems.append(f"{where}.path {fields['path']!r} is not tracked")
            continue
        try:
            lines = {line.strip() for line in (root / fields["path"]).read_text().splitlines()}
        except (OSError, UnicodeDecodeError):
            lines = set()
        if text not in lines:
            problems.append(f"{where}.text is not a line of {fields['path']}; delete the stale entry")
    return problems


def config_findings(ctx: Context) -> list[Reason]:
    config = load_config(ctx.root)
    if not config:
        return gate_findings(ctx, config)
    problems = validate_checks(config)
    if problems:
        return [Reason("error", "checks-invalid", p) for p in problems]
    findings = gate_findings(ctx, config) + template_findings(ctx, config)
    workflow = ctx.root / WORKFLOW
    if not workflow.is_file():
        return findings
    declared = {c.get("group", "core") for c in config["checks"]}
    uncommented = re.sub(r"(?m)(^|\s)#.*$", "", workflow.read_text())
    through_verify = set(re.findall(r"repo\.py verify\b[^\n]*?--group[ =](\S+)", uncommented))
    steps = {m.strip() for m in re.findall(r"(?m)^\s*(?:-\s+)?run:[ \t]*(\S.*)$", uncommented)}
    direct = {
        group for group in declared
        if all(" ".join(c["argv"]) in steps for c in config["checks"] if c.get("group", "core") == group)
    }
    for group in sorted(declared - through_verify - direct):
        findings.append(Reason("error", "checks-unwired", f"check group {group!r} has no CI job in {WORKFLOW}: run `repo.py verify --group {group}`, or each of its checks' argv as a `run:` step"))
    wired = through_verify
    for group in sorted(wired - declared):
        findings.append(Reason("error", "checks-unwired", f"{WORKFLOW} runs group {group!r}, which checks.json does not declare"))
    return findings


def cmd_verify(args) -> int:
    ctx = make_context(args)
    config = load_config(ctx.root)
    problems = validate_checks(config)
    if problems:
        for p in problems:
            print(f"blocked: {p}", file=sys.stderr)
        return 1
    checks = config["checks"]
    groups = set(args.group or [])
    unknown = groups - {c.get("group", "core") for c in checks}
    if unknown:
        print(f"blocked: unknown check group(s) {sorted(unknown)}", file=sys.stderr)
        return 1
    active = resolve_active(ctx, args.change, read_pr_body(args), args.branch, change_ids(ctx.root))
    if active.errors and (args.change or args.pr_body is not None or args.require_change):
        for e in active.errors:
            print(f"blocked: {e}", file=sys.stderr)
        return 1
    decisions, routing = route(ctx, checks, args.full)
    run_id = _dt.datetime.now(_dt.timezone.utc).strftime("%Y%m%dT%H%M%S%fZ")
    out_dir = Path(args.evidence_dir or ctx.root / ".evidence") / run_id
    tree = candidate_digest(ctx.root)
    results = []
    for check in checks:
        group = check.get("group", "core")
        selected, reason = decisions[check["id"]]
        base = {"id": check["id"], "group": group, "argv": check["argv"], "cwd": check["cwd"], "timeout_seconds": check["timeout_seconds"]}
        if groups and group not in groups:
            results.append({**base, "status": "not-run", "reason": f"group {group!r} not requested"})
        elif not selected:
            results.append({**base, "status": "not-applicable", "reason": reason})
        elif args.dry_run:
            results.append({**base, "status": "not-run", "reason": f"dry run; would run ({reason})"})
        else:
            record = run_check(ctx, check, out_dir / f"{check['id']}.log")
            record["route"] = reason
            results.append(record)
    stable = candidate_digest(ctx.root) == tree
    evidence = {
        "schema": SCHEMA,
        "run_id": run_id,
        "source": {**source_identity(ctx), "tree_sha256": tree},
        "change": active.id,
        "groups": sorted(groups) or "all",
        "routing": routing,
        "complete": stable and not any(r["status"] == "not-run" for r in results),
        "checks": results,
    }
    if not args.dry_run:
        out_dir.mkdir(parents=True, exist_ok=True)
        (out_dir / "evidence.json").write_text(json.dumps(evidence, indent=2) + "\n")
    if args.json:
        json.dump(evidence, sys.stdout, indent=2)
        print()
    else:
        for r in results:
            dur = f", {r['duration_seconds']:.1f}s" if r.get("duration_seconds") is not None else ""
            print(f"{r['status']:>14}  {r['id']}  ({r['reason']}{dur})")
            if r["status"] in ("failed", "blocked") and r.get("log"):
                print(f"{'':>16}log: {r['log']}")
                tail = format_log_tail(ctx.root / r["log"])
                if tail:
                    print(f"--- tail of {r['log']} ---")
                    print(tail)
                    print(f"--- end tail of {r['log']} ---")
        not_run = [r["id"] for r in results if r["status"] == "not-run"]
        if not_run:
            print(f"INCOMPLETE: {len(not_run)} check(s) not run: {not_run}")
        if not stable:
            print("INCOMPLETE: files changed during the run")
        if not args.dry_run:
            path = out_dir / "evidence.json"
            print(f"evidence: {path.relative_to(ctx.root) if path.is_relative_to(ctx.root) else path}")
    bad = [r for r in results if r["status"] in ("failed", "blocked")]
    return 1 if bad or not stable else 0


def make_context(args) -> Context:
    root = repo_root()
    return Context(
        root=root,
        ci=args.ci,
        baseline_branch=baseline_branch(root),
        base=args.base,
        head=args.head,
    )


def read_pr_body(args) -> str | None:
    if args.pr_body is None:
        return None
    return Path(args.pr_body).read_text() if args.pr_body != "-" else sys.stdin.read()


def format_log_tail(path: Path, max_lines: int = 150, max_bytes: int = 32768) -> str:
    try:
        raw = path.read_bytes()
    except OSError:
        return ""
    truncated = len(raw) > max_bytes
    if truncated:
        raw = raw[-max_bytes:]
    text = raw.decode("utf-8", errors="replace")
    lines = text.splitlines()
    if len(lines) > max_lines:
        lines = lines[-max_lines:]
        truncated = True
    body = "\n".join(lines)
    if truncated and body:
        return f"... (truncated; showing tail of {path.name})\n{body}"
    return body


def pull_request_failures(ctx: Context, st: ChangeStatus) -> list[str]:
    failures = []
    if st.readiness != "ready":
        failures.append(f"{st.id}: pull request change is not ready (readiness={st.readiness})")
    packet_dir = ctx.root / CHANGES / st.id
    for name in BASELINE:
        local = file_digest(packet_dir / name)
        if local is not None and root_state(ctx, name)[0] != local:
            failures.append(f"{st.id}: accepted {name} is not merged into the root: cp {CHANGES}/{st.id}/{name} {name} (carried-unmerged)")
    return failures


def compute_status(ctx: Context, args) -> dict:
    ids = change_ids(ctx.root)
    active = resolve_active(ctx, args.change, read_pr_body(args), args.branch, ids)
    statuses = {cid: evaluate_change(ctx, cid) for cid in ids}
    result = {"active": None, "active_source": active.source, "identity_errors": active.errors, "changed_paths": []}
    if active.id in statuses:
        result["active"] = active.id
        result["changed_paths"] = check_scope(ctx, statuses[active.id])
    surface = surface_findings(ctx) + config_findings(ctx) + exception_findings(ctx)
    failures = list(active.errors)
    if active.id is None and not active.errors and args.pr_body is not None:
        failures.append("pull request has no Change-ID")
    if args.require_change and active.id is None and not active.errors:
        failures.append("no active change; pass --change <id>")
    for cid, st in statuses.items():
        errors = st.errors
        if cid == active.id:
            failures += [f"{cid}: {r.message}" for r in errors]
        else:
            integrity = ("invalid-change", "scope-path-escape")
            failures += [f"{cid}: {r.message}" for r in errors if r.code in integrity]
    if args.pr_body is not None and active.id in statuses:
        failures += pull_request_failures(ctx, statuses[active.id])
    failures += [r.message for r in surface if r.level == "error"]
    result["changes"] = [statuses[c].as_dict() for c in ids]
    result["surface"] = [r.as_dict() for r in surface]
    result["failures"] = failures
    result["ok"] = not failures
    return result


def folded(change: dict, active: str | None) -> bool:
    return change["id"] != active and change["stage"] == "merged" and all(r["code"] == "merged" for r in change["reasons"])


def print_status(result: dict):
    if result["active"]:
        print(f"active change: {result['active']} (from {result['active_source']})")
    else:
        print("active change: none")
    for err in result["identity_errors"]:
        print(f"  ! {err}")
    for ch in result["changes"]:
        if folded(ch, result["active"]):
            continue
        mark = "*" if ch["id"] == result["active"] else " "
        print(f"{mark} {ch['id']} [{ch['kind']}]")
        print(f"    stage={ch['stage']} freshness={ch['freshness']} approval={ch['approval']} readiness={ch['readiness']}")
        for name, art in ch["artifacts"].items():
            print(f"    {name}: {art['sha256']} approval={art['approval']}")
        for r in ch["reasons"]:
            print(f"    - {r['level']}: {r['message']}")
    merged = sum(folded(ch, result["active"]) for ch in result["changes"])
    if merged:
        print(f"merged: {merged}")
    for r in result["surface"]:
        if r["level"] != "info":
            print(f"surface {r['level']}: {r['message']}")
    if result["changed_paths"]:
        print(f"changed paths: {len(result['changed_paths'])}")
    print("status: ok" if result["ok"] else "status: FAILED")
    for f in result["failures"]:
        print(f"  x {f}")


def cmd_status(args) -> int:
    ctx = make_context(args)
    result = compute_status(ctx, args)
    if args.json:
        json.dump(result, sys.stdout, indent=2)
        print()
    else:
        print_status(result)
    return 0 if result["ok"] else 1


class Refusal(Exception):
    pass


def plan_skeleton(root: Path) -> bytes:
    for base in (root, Path(__file__).resolve().parent.parent):
        path = base / CHANGES / TEMPLATE_PACKET / "plan.md"
        if path.is_file():
            return path.read_bytes()
    raise Refusal(f"no plan skeleton: {CHANGES}/{TEMPLATE_PACKET}/plan.md not found in the repository or beside repo.py")


def new_packet_files(root: Path, args) -> dict[str, bytes]:
    cid, kind = args.id, args.kind
    if cid == TEMPLATE_PACKET or not ID_RE.match(cid):
        raise Refusal(f"invalid change id {cid!r}: use lowercase letters, digits and inner hyphens (max 64)")
    if kind not in KINDS:
        raise Refusal(f"kind must be one of {sorted(KINDS)}")
    if not args.title.strip():
        raise Refusal("title must not be empty")
    target = root / CHANGES / cid
    if target.exists() or target.is_symlink():
        raise Refusal(f"{CHANGES}/{cid} already exists")
    scope = []
    for pattern in args.scope or []:
        problem = scope_pattern_error(pattern)
        if problem:
            raise Refusal(f"--scope {pattern!r} {problem}")
        if pattern not in scope:
            scope.append(pattern)
    required, optional = KINDS[kind]
    if args.with_spec and "spec.md" not in optional:
        raise Refusal(f"--with-spec is only allowed for kinds with an optional spec.md: {sorted(k for k, (_, o) in KINDS.items() if 'spec.md' in o)}")
    roots = {}
    for name in BASELINE:
        path = root / name
        roots[name] = path.read_bytes() if path.is_file() else None
    established = {n: data is not None and UNESTABLISHED.encode() not in data for n, data in roots.items()}
    baseline = {}
    if kind == "product-init":
        if any(data is None for data in roots.values()) or any(established.values()):
            raise Refusal("product-init needs root intent.md and spec.md that still carry the unestablished marker")
    elif kind != "repository":
        if not all(established.values()):
            raise Refusal(f"{kind} changes inherit the root baseline, but root intent.md/spec.md are missing or unestablished; start with a product-init change")
        baseline = {n: sha256_bytes(data) for n, data in roots.items()}
    head = git(root, "rev-parse", "--verify", "--quiet", "HEAD^{commit}", check=False)
    if not head:
        raise Refusal("the repository has no commit to use as the change base")
    carried = sorted((required | ({"spec.md"} if args.with_spec else set())) - {"plan.md"})
    for name in carried:
        if roots.get(name) is None:
            raise Refusal(f"root {name} does not exist")
    change = {
        "schema": SCHEMA,
        "id": cid,
        "kind": kind,
        "title": args.title.strip(),
        "base": {"ref": baseline_branch(root), "commit": head.strip()},
        "baseline": baseline,
        "write_scope": scope + [name for name in carried if name not in scope],
        "approvals": [],
    }
    files = {"change.json": (json.dumps(change, indent=2) + "\n").encode(), "plan.md": plan_skeleton(root)}
    files.update({name: roots[name] for name in carried})
    return files


def cmd_new(args) -> int:
    root = repo_root()
    files = new_packet_files(root, args)
    changes = root / CHANGES
    created_changes = not changes.exists()
    staging = changes / f".new-{args.id}-{os.getpid()}"
    made = done = False
    try:
        staging.mkdir(parents=True)
        made = True
        for rel, data in files.items():
            (staging / rel).write_bytes(data)
        staging.rename(changes / args.id)
        done = True
    except OSError as exc:
        print(f"refused: cannot create {CHANGES}/{args.id}: {exc}", file=sys.stderr)
        return 1
    finally:
        if not done:
            if made:
                shutil.rmtree(staging, ignore_errors=True)
            if created_changes:
                shutil.rmtree(changes, ignore_errors=True)
    for rel in files:
        print(f"created {CHANGES}/{args.id}/{rel}")
    if args.kind == "product-init":
        print("remove the unestablished marker from the copied intent.md and spec.md while drafting them")
        print("decide the folder structure in spec.md (default: feature-based vertical slices; see the sdlc-artifacts skill) and every checks.json gate before the first product path lands")
    print(f"base.commit is {json.loads(files['change.json'])['base']['commit']} (HEAD); run new from the change's branch point")
    print(f"next: python3 scripts/repo.py status --change {args.id}")
    return 0


def check_change_id(cid: str) -> None:
    if not ID_RE.match(cid) or cid == TEMPLATE_PACKET:
        raise Refusal(f"invalid change id {cid!r}")


def packet_json(path: Path, what: str):
    try:
        return json.loads(path.read_text())
    except FileNotFoundError:
        raise Refusal(f"{what} not found: {path}")
    except json.JSONDecodeError as exc:
        raise Refusal(f"{what} is not valid JSON: {exc}")


def write_json(path: Path, data) -> None:
    path.write_text(json.dumps(data, indent=2) + "\n")


def approval_claim(root: Path, args) -> tuple[Path, dict, dict]:
    cid, artifact = args.id, args.artifact
    check_change_id(cid)
    if artifact not in CHAIN:
        raise Refusal(f"artifact must be one of {list(CHAIN)}")
    if not args.by.strip():
        raise Refusal("--by must name the human owner who approved")
    packet_dir = root / CHANGES / cid
    path = packet_dir / "change.json"
    data = packet_json(path, f"{CHANGES}/{cid}/change.json")
    ctx = Context(root=root, baseline_branch=baseline_branch(root))
    st = evaluate_change(ctx, cid)
    if any(r.code == "invalid-change" for r in st.errors):
        raise Refusal("; ".join(r.message for r in st.errors if r.code == "invalid-change"))
    if st.stage == "merged":
        raise Refusal(f"the change is already merged on {ctx.baseline_branch!r}; record follow-up work as a new change")
    if artifact not in st.artifacts:
        raise Refusal(f"{artifact} is not in the chain of a {st.kind!r} change ({', '.join(st.artifacts)})")
    if not (packet_dir / artifact).is_file():
        raise Refusal(f"{CHANGES}/{cid}/{artifact} does not exist yet; draft it first")
    if is_placeholder(st.kind, packet_dir, artifact):
        raise Refusal(f"{CHANGES}/{cid}/{artifact} still carries the unestablished marker; draft it and remove the marker first")
    chain = list(st.artifacts)
    for upstream in chain[: chain.index(artifact)]:
        if st.artifacts[upstream]["approval"] == "none":
            raise Refusal(f"approve {upstream} before {artifact}: artifacts are approved in order {' -> '.join(chain)}")
    claim = {
        "artifact": artifact,
        "sha256": file_digest(packet_dir / artifact),
        "by": args.by.strip(),
        "at": _dt.datetime.now(_dt.timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ"),
    }
    if artifact == PLAN:
        claim["write_scope_sha256"] = scope_digest(data.get("write_scope", []))
    if args.note:
        claim["note"] = args.note
    return path, data, claim


def cmd_approve(args) -> int:
    path, data, claim = approval_claim(repo_root(), args)
    kept = [c for c in data.get("approvals", []) if not (isinstance(c, dict) and c.get("artifact") == args.artifact)]
    replaced = len(data.get("approvals", [])) - len(kept)
    data["approvals"] = kept + [claim]
    write_json(path, data)
    print(f"recorded approval of {args.artifact} at {claim['sha256']} by {claim['by']} ({claim['at']})"
          + (f"; replaced {replaced} earlier claim(s) for it" if replaced else ""))
    claimed = {c.get("artifact") for c in kept if isinstance(c, dict)}
    downstream = [n for n in CHAIN[CHAIN.index(args.artifact) + 1:] if n in claimed]
    if downstream:
        print(f"approve again, in order: {', '.join(downstream)}")
    print("this is a local, unverified claim: it binds bytes, it does not prove identity")
    print(f"next: python3 scripts/repo.py status --change {args.id}")
    return 0


class Clone:
    def __init__(self, path: Path):
        if not (path / ".git").exists() or git(path, "rev-parse", "--is-inside-work-tree", check=False) is None:
            raise Refusal(f"{path} is not a Git clone of the template")
        if git(path, "rev-parse", "--is-shallow-repository").strip() == "true":
            raise Refusal(f"{path} is a shallow clone; the report needs the template's full history")
        self.path = path

    def commit(self, ref: str) -> str:
        out = git(self.path, "rev-parse", "--verify", "--quiet", ref + "^{commit}", check=False)
        if not out:
            raise Refusal(f"{ref!r} is not a commit in {self.path}")
        return out.strip()

    def lists(self, commit: str) -> tuple | None:
        source = git(self.path, "show", f"{commit}:scripts/repo.py", check=False)
        if source is None:
            return None
        found = {}
        try:
            for node in ast.parse(source).body:
                if isinstance(node, ast.Assign) and len(node.targets) == 1 and isinstance(node.targets[0], ast.Name):
                    if node.targets[0].id in ("UPGRADE_COPY", "UPGRADE_MERGE"):
                        found[node.targets[0].id] = tuple(ast.literal_eval(node.value))
        except (SyntaxError, ValueError) as exc:
            raise Refusal(f"cannot read the upgrade lists of {commit[:12]}: {exc}")
        if set(found) != {"UPGRADE_COPY", "UPGRADE_MERGE"} or not all(isinstance(e, str) for v in found.values() for e in v):
            return None
        return found["UPGRADE_COPY"], found["UPGRADE_MERGE"]

    def owned(self, commit: str, copy_entries) -> dict:
        manifest = git(self.path, "show", f"{commit}:{OWNED}", check=False)
        if manifest is not None:
            paths = owned_manifest(manifest)
            outside = [rel for rel in paths if not in_copy_group(rel, copy_entries)]
            if outside:
                raise Refusal(f"{OWNED} at {commit[:12]} lists paths outside the copy group: {', '.join(outside[:5])}")
            entries = tree_entries(self.path, commit, paths)
            return {rel: entries.get(rel, MISSING) for rel in paths}
        return {rel: e for rel, e in tree_entries(self.path, commit, copy_entries).items() if in_copy_group(rel, copy_entries)}

    def ancestor(self, older: str, newer: str) -> bool:
        return subprocess.run(["git", "merge-base", "--is-ancestor", older, newer], cwd=self.path, capture_output=True).returncode == 0


def base_candidates(clone: Clone, target: str, index: dict, copy_entries) -> list[tuple[str, int, int]]:
    scored = []
    for commit in git(clone.path, "rev-list", "--reverse", "--topo-order", target).split():
        owned = {rel: e for rel, e in tree_entries(clone.path, commit, copy_entries).items() if in_copy_group(rel, copy_entries)}
        hit = sum(1 for rel, entry in owned.items() if index.get(rel) == entry)
        scored.append((commit, hit, len(owned)))
    best = max((hit for _, hit, _ in scored), default=0)
    return [c for c in scored if c[1] == best and best > 0]


def migration_ids(text: str) -> list[str]:
    return re.findall(r"(?m)^## ([a-z0-9][a-z0-9-]*[a-z0-9])\s*$", text)


def migrations_between(clone: Clone, base: str, target: str) -> list[tuple[str, str]]:
    text = git(clone.path, "show", f"{target}:{MIGRATIONS}", check=False)
    if text is None:
        return []
    order = git(clone.path, "rev-list", "--reverse", "--topo-order", f"{base}..{target}").split()
    picked = []
    for cid in migration_ids(text):
        added = git(clone.path, "log", "--diff-filter=A", "--format=%H", target, "--", f"{CHANGES}/{cid}/change.json").split()
        if added and added[-1] in order:
            picked.append((order.index(added[-1]), cid, added[-1]))
    return [(cid, commit) for _, cid, commit in sorted(picked)]


def upgrade_pin(config: dict) -> tuple[dict | None, set]:
    if "problem" in config:
        raise Refusal(config["problem"])
    pin = config.get("template")
    problems = pin_problems(pin)
    if problems:
        raise Refusal("checks.json template: " + "; ".join(problems))
    return pin, set((pin or {}).get("carried", []))


def kind(entry) -> str:
    return {"120000": " (symlink)", "100755": " (executable)"}.get(entry[0], "") if entry else ""


def cmd_upgrade(args) -> int:
    root = repo_root()
    clone = Clone(Path(args.template).resolve())
    target = clone.commit(args.to)
    lists = clone.lists(target)
    if lists is None or git(clone.path, "show", f"{target}:{OWNED}", check=False) is None:
        raise Refusal(f"{args.to} predates upgrade support (no UPGRADE_COPY/UPGRADE_MERGE or {OWNED})")
    copy_entries, merge_entries = lists
    pin, carried = upgrade_pin(load_config(root))
    index = index_entries(root)
    claimed = args.from_ or (pin or {}).get("commit")
    if not claimed:
        found = base_candidates(clone, target, index, copy_entries)
        print("no base: pass --from <commit>, or record checks.json template. Candidates with the most matching copy files, oldest first:")
        for commit, hit, total in found[:20]:
            print(f"  {commit}  {hit} of {total} match")
        if len(found) > 20:
            print(f"  ... {len(found) - 20} more with the same count")
        return 1
    base = clone.commit(claimed)
    if not clone.ancestor(base, target):
        raise Refusal(f"base {base[:12]} is not an ancestor of target {target[:12]}")
    base_lists = clone.lists(base)
    base_entries = base_lists[0] if base_lists else copy_entries
    owned_base = clone.owned(base, base_entries)
    owned_target = clone.owned(target, copy_entries)
    stray = sorted(c for c in carried if c not in owned_base)
    if stray:
        raise Refusal(f"template.carried names files the base template does not own: {', '.join(stray)}")
    q = shlex.quote
    hit = sum(1 for rel, entry in owned_base.items() if index.get(rel) == entry)
    tags = git(clone.path, "tag", "--points-at", target).split()
    print(f"target: {target}{' (' + ', '.join(tags) + ')' if tags else ''}")
    print(f"base:   {base}  {hit} of {len(owned_base)} template-owned copy files match the adopter")
    section("drift", [
        f"{rel}: {'missing' if rel not in index else 'differs from the base'}"
        for rel, entry in sorted(owned_base.items())
        if rel not in carried and index.get(rel) != entry and index.get(rel) != owned_target.get(rel)
    ])
    recipe = "git merge-file -p {f} <(git -C {c} show {b}:{f}) <(git -C {c} show {t}:{f})"
    carried_lines = []
    for rel in sorted(carried):
        if rel not in owned_target:
            carried_lines.append(f"{rel}: removed upstream; delete it, or keep it as a local file outside the pin")
        elif owned_base[rel] != owned_target[rel]:
            carried_lines.append(f"{rel}: {recipe.format(f=q(rel), c=q(str(clone.path)), b=base, t=target)}")
    section("carried", carried_lines)
    copy_lines = []
    for rel in sorted(set(owned_base) | set(owned_target)):
        before, after = owned_base.get(rel), owned_target.get(rel)
        if rel in carried or before == after or index.get(rel) == after:
            continue
        if after is None:
            copy_lines.append(f"removed  {rel}")
        elif before is None:
            clash = rel in index or os.path.lexists(root / rel)
            copy_lines.append(f"added    {rel}{kind(after)}{' (collides with an existing adopter file; resolve it first)' if clash else ''}")
        else:
            copy_lines.append(f"changed  {rel}{kind(after)}{' (mode ' + before[0] + ' -> ' + after[0] + ')' if before[0] != after[0] else ''}")
    section("copy", copy_lines)
    merge_base = tree_entries(clone.path, base, merge_entries)
    merge_target = tree_entries(clone.path, target, merge_entries)
    merge_lines = []
    for rel in merge_entries:
        before, after = merge_base.get(rel), merge_target.get(rel)
        if before == after:
            continue
        if after is None:
            merge_lines.append(f"{rel}: removed upstream; remove the template's part of it")
        elif before is None or rel not in index:
            merge_lines.append(f"{rel}: new to this repository; start from git -C {q(str(clone.path))} show {target}:{q(rel)}")
        else:
            merge_lines.append(f"{rel}: git -C {q(str(clone.path))} diff {base} {target} -- {q(rel)}")
            merge_lines.append(f"    {recipe.format(f=q(rel), c=q(str(clone.path)), b=base, t=target)}")
    section("merge", merge_lines)
    section("local", sorted(
        rel for rel in index
        if in_copy_group(rel, copy_entries + base_entries) and rel not in owned_base and rel not in owned_target
    ))
    section("migrations", [f"{cid} ({commit[:12]}): see {MIGRATIONS} at the target" for cid, commit in migrations_between(clone, base, target)])
    kept = sorted(c for c in carried if c in owned_target)
    new_pin = {"commit": target, "digest": pin_digest({rel: e for rel, e in owned_target.items() if rel not in kept})}
    if kept:
        new_pin["carried"] = kept
    print("pin: record in checks.json once the upgrade is staged")
    print("  \"template\": " + json.dumps(new_pin))
    return 0


def section(name: str, lines: list[str]) -> None:
    print(f"{name}: {len(lines) or 'none'}")
    for line in lines:
        print(f"  {line}")


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(prog="repo.py", description="Repository control plane: `status`, `verify`, `new`, `approve` and `upgrade`.")
    sub = parser.add_subparsers(dest="command", required=True)

    def common(p):
        p.add_argument("--change", help="explicit active change id (authoritative locally)")
        p.add_argument("--pr-body", metavar="FILE", help="pull request body file ('-' for stdin); requires a Change-ID line")
        p.add_argument("--branch", help="branch name to use for convenience inference (default: current branch)")
        p.add_argument("--base", help="base commit/ref for the candidate diff (PR base SHA in CI)")
        p.add_argument("--head", help="candidate commit (default: working tree)")
        p.add_argument("--ci", action="store_true", help="CI mode: skip machine-local checks")
        p.add_argument("--require-change", action="store_true", help="fail when no active change is identified")
        p.add_argument("--json", action="store_true", help="machine-readable output")

    status = sub.add_parser("status", help="compute change lifecycle state and gates")
    common(status)
    status.set_defaults(func=cmd_status)
    verify = sub.add_parser("verify", help="run registered checks with routing and evidence")
    common(verify)
    verify.set_defaults(func=cmd_verify)
    verify.add_argument("--full", action="store_true", help="run every check regardless of routing (pushes to the baseline branch)")
    verify.add_argument("--group", action="append", help="only run checks in this group (repeatable); others are reported not-run")
    verify.add_argument("--dry-run", action="store_true", help="report routing without running checks")
    verify.add_argument("--evidence-dir", help="evidence root (default: .evidence/)")
    new = sub.add_parser("new", help="create a change packet (no branch, commit or approval)")
    new.set_defaults(func=cmd_new)
    new.add_argument("id", help="change id")
    new.add_argument("--kind", required=True, help=f"one of {', '.join(sorted(KINDS))}")
    new.add_argument("--title", required=True, help="one-line summary")
    new.add_argument("--scope", action="append", metavar="PATTERN", help="write_scope pattern (repeatable)")
    new.add_argument("--with-spec", action="store_true", help="also copy root spec.md (intent, incident, architecture)")
    approve = sub.add_parser("approve", help="record the owner's digest-bound approval of a chain artifact (run by the owner)")
    approve.set_defaults(func=cmd_approve)
    approve.add_argument("id", help="change id")
    approve.add_argument("artifact", help=f"one of {', '.join(CHAIN)}")
    approve.add_argument("--by", required=True, help="the human owner who reviewed exactly these bytes")
    approve.add_argument("--note", help="free-text note stored with the claim")
    upgrade = sub.add_parser("upgrade", help="report what upgrading to a template commit changes (read-only; run the target's repo.py)")
    upgrade.set_defaults(func=cmd_upgrade)
    upgrade.add_argument("--template", required=True, metavar="CLONE", help="path to a full clone of the template")
    upgrade.add_argument("--to", required=True, metavar="REF", help="target template commit or tag")
    upgrade.add_argument("--from", dest="from_", metavar="COMMIT", help="base template commit (default: checks.json template.commit)")
    return parser


def main(argv=None) -> int:
    args = build_parser().parse_args(argv)
    try:
        return args.func(args)
    except Refusal as exc:
        print(f"refused: {exc}", file=sys.stderr)
        return 1
    except GitError as exc:
        print(f"blocked: {exc}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    sys.exit(main())

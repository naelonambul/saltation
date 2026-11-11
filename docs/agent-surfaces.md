# Agent surface support

This page has three parts, and only the first is a promise:

1. **Repository-owned portable contract.** What the repository itself defines, for every agent.
2. **Observed harness integration.** How specific harnesses currently pick that contract up, as dated, smoke-tested observations.
3. **Optional user-local model/tier mapping.** How a user may route roles to models. An optimization only, outside the repository.

Hard repository rules must never depend on an agent having read `AGENTS.md`. They are enforced by `scripts/repo.py`, tests, and CI.

## 1. Repository-owned portable contract

- `AGENTS.md` is the canonical instruction source.
- `.agents/skills/<name>/SKILL.md` holds the only copy of each shared skill.
- `scripts/repo.py`, `checks.json`, CI and the required human pull-request review decide what may change and whether a change is accepted.
- The `change-execution` skill defines execution roles (`coordinator`, `worker`, `reviewer`), cost tiers (`high`, `standard`, `economy`), the worker and review briefs, and mechanical acceptance. It names no model and no harness configuration path.

Nothing in the repository depends on which harness or model does the work.

## 2. Observed harness integration

Whether a harness receives the contract depends on the host, its version, and user or admin settings. A surface is supported only when a smoke test on that surface passes. Everything here is an observation, not an interface the repository relies on.

### Claude Code

#### Instructions

Claude Code's built-in `agents-md` behavior (default mode `claude-md-or-agents-md`) loads `AGENTS.md` only as a fallback:

- A project `CLAUDE.md`, `.claude/CLAUDE.md`, or `CLAUDE.local.md` between the project root and the working directory **shadows** `AGENTS.md`. The fallback stays out unless that file imports it with a line such as `@AGENTS.md`.
- User or managed settings can select a mode (`claude-md`, `managed-only`) in which project `AGENTS.md` is never loaded, with no repository change.
- Explore and Plan agents, and custom agents configured with `omitClaudeMd`, receive no project instructions at all.

For these reasons the template ships **no** `CLAUDE.md`. If a project adds one, it must import `AGENTS.md` rather than restate or replace it. `CLAUDE.local.md` is a personal, untracked file, so it can silently disable the fallback on one machine. Avoid it, or import `AGENTS.md` from it.

#### Skills

Claude Code discovers project skills under `.claude/skills/`, not `.agents/skills/`. Each skill therefore has a thin adapter: a relative symlink `.claude/skills/<name> -> ../../.agents/skills/<name>`. Edit only the canonical body under `.agents/skills/`.

On checkouts where Git symlinks are disabled (for example Windows with `core.symlinks=false`), each adapter becomes a small text file that contains the link target. The skills are then silently unavailable. Enable symlinks (`git config core.symlinks true`, plus Windows Developer Mode or equivalent privilege) and re-checkout.

#### Hooks

`.claude/settings.json` registers `scripts/hooks/guard.py claude` as a `PreToolUse` hook for `Edit|Write|MultiEdit`, MCP edit tools and `Bash`. The adapter holds no policy; `repo.py status` reports `hook-broken` when it points at an untracked script. The guard's exit code 2 blocks the tool call and feeds the reason back to the agent; an `ask` decision is returned as `permissionDecision: ask`, which prompts the human. Project settings apply only when the session's working directory is inside the project, and user or managed settings can change which hooks run, so a hook is never the enforcement boundary. To check the path on the real CLI, ask `claude -p` in a scratch copy to edit the root `intent.md` and to add an approval to a packet's `change.json`, and confirm both edits are denied with the guard's message (observed 2026-10-05, CLI 2.1.289).

### Codex

Codex reads `AGENTS.md` and discovers skills in `.agents/skills/` itself. No adapter file is needed, and the template ships none. No hook integration has been observed, so the guard does not run in Codex sessions; `repo.py status` catches the same edits after the fact.

Codex also lists user-level and built-in skills alongside the repository's. Only the repository's skills are part of the contract.

The `workspace-write` sandbox keeps `.git` read-only, so Codex cannot commit in a plain clone. Either make commits outside the sandbox, or keep the Git data outside the worktree, add that directory as a writable root and set `GIT_DIR` and `GIT_WORK_TREE`. The control-plane tests clear these variables, so their fixtures stay out of that data. `repo.py verify` passes them to registered checks, because a check may need Git, so a product test that creates its own Git fixtures must clear them too.

### Smoke-test results

| Date | Surface | Setup | `AGENTS.md` delivered | Project skills discovered |
|---|---|---|---|---|
| 2026-09-23 | Claude Code CLI 2.1.280 | no `CLAUDE.md`, no `.claude/skills` adapters | yes | 0/5 |
| 2026-09-23 | Claude Code CLI 2.1.280 | no `CLAUDE.md`, adapters present | yes | 5/5 |
| 2026-09-23 | Claude Code CLI 2.1.280 | `CLAUDE.md` without `@AGENTS.md`, adapters present | **no** (shadowed) | 5/5 |
| 2026-09-23 | Claude Code CLI 2.1.280 | `CLAUDE.md` importing `@AGENTS.md`, adapters present | yes | not asked |
| 2026-09-23 | Claude desktop app (Code tab) | session opened before adapters existed | not observed | not observed |
| 2026-09-24 | Claude Code CLI 2.1.280 | no `CLAUDE.md`, adapters present | yes | 7/7 |
| 2026-09-24 | Codex CLI 0.156.1 | `codex exec --sandbox read-only`, no adapters | yes | 7/7 (plus user-level and built-in skills) |
| 2026-10-05 | Claude Code CLI 2.1.289 | no `CLAUDE.md`, adapters and `.claude/settings.json` hooks present; probes run non-interactively in a scratch copy | yes (no, with a shadowing `CLAUDE.md`) | 4/4 core skills checked; guard denied the root `intent.md` edit and the approval edit |
| 2026-10-30 | Claude Code CLI 2.1.314 | no `CLAUDE.md`, adapters and `.claude/settings.json` hooks present; probes run non-interactively in a scratch copy | yes (no, with a shadowing `CLAUDE.md`) | 4/4 core skills; guard asked on commands placed after a comment line |
| 2026-10-30 | Claude Code CLI 2.1.314 | hooks with the MCP edit matcher present; probes run non-interactively in a scratch copy | yes | 4/4 core skills; guard denied a live `Write` to root `intent.md`; MCP edit tools unit-tested only |
| 2026-10-30 | Claude Code CLI 2.1.314 | shortened `sdlc-artifacts` Close section; probes run non-interactively in a scratch copy | yes | 4/4 core skills |
| 2026-10-30 | Claude Code CLI 2.1.314 | changed `sdlc-artifacts` and `repository-quality`; probes run non-interactively in a scratch copy | yes | 4/4 core skills |
| 2026-10-30 | Codex CLI 0.156.1 | same scratch copy, `codex exec --sandbox read-only` | yes | 4/4 core skills |
| 2026-10-31 | Claude Code CLI 2.1.314 | changed `AGENTS.md` and skills; no `CLAUDE.md`, adapters present; probes run non-interactively in a scratch copy | yes (no, with a shadowing `CLAUDE.md`) | 4/4 core skills |
| 2026-10-31 | Codex CLI 0.166.0 | same scratch copy, `codex exec --skip-git-repo-check --sandbox read-only` | yes | 4/4 core skills |
| 2026-11-01 | Claude Code CLI 2.1.316 | changed `sdlc-artifacts` and `repository-quality`; no `CLAUDE.md`, adapters present; probes run non-interactively in a scratch copy | yes (no, with a shadowing `CLAUDE.md`) | 4/4 core skills |
| 2026-11-01 | Codex CLI 0.166.0 | same scratch copy, `codex exec --skip-git-repo-check --sandbox read-only` | yes | 4/4 core skills |
| 2026-11-01 | Claude Code CLI 2.1.316 | changed `sdlc-artifacts` CI failure text; no `CLAUDE.md`, adapters present; probes run non-interactively in a scratch copy | yes (no, with a shadowing `CLAUDE.md`) | 4/4 core skills |
| 2026-11-01 | Codex CLI 0.166.0 | same scratch copy, `codex exec --skip-git-repo-check --sandbox read-only` | yes | 4/4 core skills |
| 2026-11-01 | Claude Code CLI 2.1.316 | changed `AGENTS.md` and `sdlc-artifacts` identity text; no `CLAUDE.md`, adapters present; probes run non-interactively in a scratch copy | yes (no, with a shadowing `CLAUDE.md`) | 4/4 core skills |
| 2026-11-01 | Codex CLI 0.166.0 | same scratch copy, `codex exec --skip-git-repo-check --sandbox read-only` | yes | 4/4 core skills |
| 2026-11-01 | Claude Code CLI 2.1.316 | changed `AGENTS.md` and `sdlc-artifacts` pre-existing file text; no `CLAUDE.md`, adapters present; probes run non-interactively in a scratch copy | yes (no, with a shadowing `CLAUDE.md`) | 4/4 core skills |
| 2026-11-01 | Codex CLI 0.166.0 | same scratch copy, `codex exec --skip-git-repo-check --sandbox read-only` | yes | 4/4 core skills |
| 2026-11-01 | Claude Code CLI 2.1.316 | changed `sdlc-artifacts` and `repository-quality` workflow handoff text; no `CLAUDE.md`, adapters present; probes run non-interactively in a scratch copy | yes (no, with a shadowing `CLAUDE.md`) | 4/4 core skills |
| 2026-11-01 | Codex CLI 0.166.0 | same scratch copy, `codex exec --skip-git-repo-check --sandbox read-only` | yes | 4/4 core skills |

The desktop app is **not verified**. Re-run the procedure below from a desktop session before claiming support. No other agent surface has been tested. Cursor is not a target.

### Smoke-test procedure

Run it against a scratch copy of the repository, never the working checkout:

```sh
git clone . /tmp/agent-smoke && cd /tmp/agent-smoke
Q='Answer from your loaded context only; do not call any tool. Line 1: "AGENTS=yes" if your context contains a document headed "Repository Agent Instructions", else "AGENTS=no". Line 2: "SKILLS=" followed by a comma-separated list of the project skills listed as available to you.'
claude -p "$Q" --disallowedTools "Read,Bash,Grep,Glob,Edit,Write,Agent"
codex exec --sandbox read-only "$Q" < /dev/null
```

For Claude Code, keep the `Skill` tool allowed; some hosts do not list skills when it is disallowed. Repeat the Claude command with a shadowing `CLAUDE.md` to confirm the negative case. For Codex, close stdin as shown, or `codex exec` waits for more input. Check that the repository's skills are not also installed at user level, so the result shows repository discovery. Then add a row to the results table. Run the procedure again whenever a change touches `AGENTS.md`, a skill or the guard, and after a harness upgrade when an agent seems to have stopped following the repository rules.

## 3. Optional user-local model/tier mapping

Choosing which model serves `high`, `standard` or `economy` is an optimization. It is never an authority and never a correctness dependency.

- Do it outside the repository, with whatever mechanism the installed harness offers for choosing a subagent's model.
- The repository names no model and relies on no configuration path. None of the harness's agent or model configuration locations has been smoke-tested here, so none is documented as an interface.
- Without a stable per-role mapping, pick the worker model when spawning it, by hand. The `change-execution` contract is identical either way, including when the worker is the cheapest model available.

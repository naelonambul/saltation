# Repository Agent Instructions

Keep this file short. It contains repository-wide invariants that are relevant in nearly every agent session. Put detailed or conditional procedures in `.agents/skills/`.

## Authority

1. Explicit decisions from the human owner are final.
2. This `AGENTS.md` defines repository operating rules. It is guidance, not the enforcement boundary: hard rules are enforced by `scripts/repo.py`, tests, and CI, and some agents (for example subagents that omit project instructions) never read this file.
3. The root `intent.md` and `spec.md` are the durable product baseline. Work happens in change packets, `changes/<id>/`, each with its own plan; there is no root plan. See `changes/README.md`.
4. Approved upstream artifacts constrain downstream ones. Never silently resolve a contradiction by overriding the upstream artifact; surface the conflict to the human owner.
5. Repository source, tests, and configuration are authoritative for the current implemented state. Approved change packets authorize intended changes.
6. `REVIEW.md` defines review policy, not product intent.
7. `docs/` is supporting/reference/history material. It does not override the baseline or an approved change.

## SDLC gates

- Run `python3 scripts/repo.py status --change <id>`. It computes each change's stage, freshness, approval and readiness from repository facts; never store or assert those states by hand.
- Artifacts are approved in order `intent -> spec -> plan`, only by the human owner, via digest-bound claims in `change.json`. Never infer approval from file contents, Git state, or your own judgment.
- Do not change anything outside the packet until status reports `readiness=ready`, and stay inside the change's declared `write_scope`.
- If implementation requires a material departure from the approved plan, stop, revise the plan, and obtain a fresh owner approval.
- Local approval is `unverified`: it detects stale approvals, not identity.

Use the `sdlc-artifacts` skill whenever creating, revising, approving, or closing a change.

## Working rules

- Use repository-local skills from `.agents/skills/` when their trigger applies.
- When the product stack becomes concrete, use the `repository-quality` skill to discover existing quality tooling or bootstrap a minimal stack-appropriate setup when it is missing.
- Do not force-push, rewrite history, hard-reset shared work, or delete unrelated changes unless explicitly instructed.
- Do not delete, overwrite, regenerate, move, or change ignore rules for a file that existed before the current task until you have established its origin; follow "Pre-existing local files" in the `sdlc-artifacts` skill. An out-of-scope path is a status finding, not permission to remove or hide it.
- Do not weaken tests, lint rules, type checks, security checks, or configuration merely to make validation pass.
- Do not commit tool caches or generated analysis metadata.
- Prefer repository-native commands and conventions over agent preferences.
- Commit, push, open pull requests and read CI under the agent identity, using the command path `docs/host-setup.md` selects for this host. If no path works, stop and ask the owner. Never fall back to the owner's credentials.
- When delegating implementation, follow the `change-execution` skill. The delegating agent stays responsible for the change: workers act only from a closed brief within `write_scope`, never approve, commit, merge or edit the packet, and their reports are accepted only after the delegating agent re-runs verification itself.
- Keep agent-specific adapters thin. Put shared policy in this file, skills, or deterministic scripts instead of duplicating it per agent. Do not add a `CLAUDE.md` that replaces this file; see `docs/agent-surfaces.md`.

## Canonical commands

- `python3 scripts/repo.py new <id> --kind <kind> --title <summary> [--scope <pattern>]...`: creates a change packet (no branch, commit or approval).
- `python3 scripts/repo.py status --change <id>`: lifecycle, approval, identity, write-scope, and agent-surface gates.
- `python3 scripts/repo.py verify --change <id>`: runs the checks registered in `checks.json` that the diff routes to (`--full` for all), and writes evidence to `.evidence/`.
- `python3 scripts/repo.py approve <id> <artifact> --by <owner>`: records the owner's digest-bound claim for the current bytes, in chain order. The owner runs it, or asks for it explicitly; it is still an `unverified` local claim.

`scripts/hooks/guard.py` is the agent-neutral in-session guard: it denies file-tool edits to the root baseline, merged packets and approvals, and asks before destructive git. `.claude/settings.json` only wires it, so it runs in Claude Code only. A denial names the command to use instead; do not work around it through the shell, which `repo.py status` catches anyway.

Project build, test, lint, format-check, and type-check commands are registered as checks in `checks.json`, not listed here. The generic template needs only Python 3 and Git. Stack toolchains belong to the product that adopts them. Use the `repository-quality` skill to discover or bootstrap them.

## Definition of done

Before reporting implementation complete:

- Confirm the approved change plan is satisfied or explicitly revised and re-approved.
- `repo.py status --change <id>` passes and `repo.py verify --change <id> --full` reports no `failed` or `blocked` check and a complete run (no `not-run` check). `--group` runs are partial execution units: CI runs every group in its own job, and the summary requires them all.
- Report the exact validation performed, its result, and the evidence path. `blocked` is never success.
- Review the change against `REVIEW.md`.
- Confirm no generated analysis metadata, credentials, or unrelated changes are included.

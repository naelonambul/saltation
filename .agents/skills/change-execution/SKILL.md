---
name: change-execution
description: Implement an approved change packet, and decide whether and how to delegate the work to worker agents. Use when `python3 scripts/repo.py status --change <id>` reports `readiness=ready` and you are about to implement, split the plan into units, brief a worker or reviewer, accept a worker's result, or retry a failed unit.
---

# Change Execution

This skill covers how an approved change gets implemented. It is not an authority. The approved packet decides what may change. `repo.py`, CI and the human pull-request review decide whether the change is accepted. Nothing here adds a plan, an approval, a record, or a merge path.

Assume every worker is weak and interchangeable. The contract must hold when the cheapest available model does the work. Reliability comes from the brief and from mechanical acceptance, not from the model.

## 1. Preconditions

- `python3 scripts/repo.py status --change <id>` reports `readiness=ready`.
- The coordinator is the agent that owns the change. It reads the approved packet and its upstream artifacts, and stays responsible for everything a worker produces.
- Workers do not read the packet. They act only from their brief.

## 2. Roles and tiers

- Roles:
  - `coordinator`: owns the change, writes briefs, accepts results, commits;
  - `worker`: implements one unit from a brief;
  - `reviewer`: reads and reports, never edits.
- Tiers: `high`, `standard` and `economy`. `economy` means the cheapest model available.
- Any model that follows the brief can fill any role.
- This skill names no model and no harness configuration path. A tier is served by an optional user-local mapping if the harness offers one. Otherwise the coordinator or the owner picks the worker model when spawning. The contract is the same either way. Model routing saves cost; correctness never depends on it.

## 3. Route

Choose one route before starting, and record it.

| Task shape | Route |
|---|---|
| Trivial or local: the brief would be about as long as the diff (for example one plan step, one or two files) | **Single agent.** The coordinator implements. No workers. |
| Bounded normal change | Coordinator, or the approved plan, plus **one** worker |
| Large, independent workstreams with disjoint FILES and no ordering dependency | Coordinator plus a **small** number of parallel workers. Otherwise run them in sequence. |
| Risky, ambiguous or wide-reaching (triggers in section 8) | Add **one** independent reviewer. Use more only when reviewers disagree or a finding is disputed. |
| Verification failed | Retry **only the failed unit** (section 7) |

Delegation is never mandatory. When in doubt on a small change, do it yourself.

## 4. Units

- Derive units from the approved plan's Order of work. A unit is a slice of that plan, not a new plan.
- A unit may be delegated only if every field of the brief can be filled. That includes ACCEPTANCE with literal expected values, and a VERIFY command that runs in minutes.
- If a field cannot be filled, do not delegate that unit. Implement it yourself. If the plan itself is unclear, stop and get the plan revised and re-approved.
- Each unit's FILES is a subset of the change's `write_scope`. Parallel units never share a file.

## 5. Protected oracle tests

- When you supply explicit acceptance tests, regression witnesses or other oracle tests with a brief, list those exact files under PROTECTED. Write them yourself, or have them written as a separate, earlier unit.
- The worker must not weaken, delete or modify a PROTECTED file. This keeps the pass/fail oracle out of a weak worker's hands.
- The worker may create or modify ordinary implementation tests within its FILES when the approved plan calls for them.
- A worker's own tests are supporting evidence only. Acceptance rests on you re-running ACCEPTANCE and VERIFY (section 7).

## 6. Brief

- Fill in `references/worker-brief.md` for every worker. Leave no placeholder: a field you cannot fill means the unit is not ready (section 4).
- Point the worker at paths rather than pasting file contents. Quote spec lines verbatim when the worker must obey them; do not paraphrase.
- Spawn a fresh worker for every brief. Never resume a worker to add instructions: send a new, consolidated brief.

## 7. Accept, mechanically first

A worker's report is a claim, not evidence. For every unit:

1. Check that the changed paths (`git status --porcelain`) are all inside the unit's FILES, and that every PROTECTED file is byte-for-byte unchanged (`git diff --exit-code -- <protected files>`).
2. Re-run every VERIFY command yourself, and check each ACCEPTANCE item against the actual result.
3. Run `python3 scripts/repo.py status --change <id>` and `python3 scripts/repo.py verify --change <id>`.
4. Read the diff for what tests cannot see: escapes, encodings, hidden edge cases, and whether the worker's own tests actually assert the required behaviour.

On failure:

- Spawn a fresh worker with the same brief plus the concrete failure evidence.
- After two failed retries, move the unit up a tier, or implement it yourself.
- Never loosen ACCEPTANCE or PROTECTED tests to make a unit pass.

Commit accepted work yourself, under the repository's normal commit identity. Workers never commit.

## 8. Selective review

Add a read-only reviewer, using `references/review-brief.md`, only when at least one trigger applies:

- security or privacy;
- persisted data or its format;
- concurrency;
- a public interface;
- a change to verification, tests' oracles, or registered checks;
- an ambiguous spec;
- a unit that needed a retry;
- a diff clearly larger than the plan implied.

Prefer a reviewer from a different model family; otherwise use a fresh context. Sort its findings into act on, consider, noted and dismissed, with a reason for each dismissal. The review is advisory. The required human pull-request review remains the merge gate.

## 9. Context economy

- Keep only fixed-shape reports and your own check results in the coordinator's context. Never paste raw logs or whole files back.
- Send bulk reading to a worker or reviewer and take back a short answer.

## 10. Close

- Continue with the Close steps of the `sdlc-artifacts` skill, unchanged.
- In the pull request's Verification section, record the route, each unit with its tier and status, and which commands you re-ran. Nothing else is recorded in the repository.

---

Adapts ideas from pstack (MIT, Lauren Tan): the structured worker brief, verifying the real artifact rather than a report, and selective multi-model review. The wording and the governance boundary are this template's own.

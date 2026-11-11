# Implementation Plan

## Summary

Five independent reviews (Codex Astra, agy Argon, Claude, GPT Pro 6, GPT 5.6 Pro, November 2026) judged the template materially overengineered for its single-owner-plus-agents purpose. All five ranked the closure and frozen-history machinery as the largest cost. The owner chose the "small hard core plus optional profiles" direction. This is the first of the changes that implement it.

This change makes the **merged pull request the closure**. A change is done when its pull request merges. Git, the pull request and its CI run are the audit record. The change removes:

- `closure.json` and `repo.py close`;
- the `closure` state axis and the `closed` stage, and `frozen` freshness;
- first-parent anchor discovery, `frozen-integrity`, `closure-history`, `removed_frozen`, and the Git-object batch reader that serves them;
- the 45 closed template-maintenance packets under `changes/`, which become history in Git only.

It keeps, unchanged in meaning:

- digest-bound approvals in chain order, plan-to-`write_scope` binding, and stale detection;
- root baseline freshness for open changes, and the `repository`-kind rule against editing an established root;
- `write_scope` enforcement against the diff, readiness, and one `Change-ID` per pull request;
- `repo.py verify` with its tree-bound local evidence, now a local record and not a merge prerequisite;
- the required CI `summary` check as the merge gate.

New computed rule: a packet whose `change.json` exists on the baseline branch tip is **merged**. A merged packet is not checked for readiness or freshness. Any edit to it in a later pull request is an ordinary diff path, so it fails `out-of-scope` unless that change's approved `write_scope` names it.

### Later changes in this direction (not part of this change)

Each is its own packet with its own owner approval. Order may change after this one lands.

1. `ci-native-tests`: run the control-plane tests once with native `unittest`; retire `scripts/tests/run.py`, `test_run.py` and the in-process/subprocess equivalence harness; one test job plus `status` plus `summary`; replace textual workflow-shape tests with a few semantic assertions. Touches `.github/workflows/**`, so the owner push handoff applies.
2. `checks-simplify`: reduce `checks.json` to a plain command list; drop path routing, groups and CI group-wiring checks until a product needs them.
3. `optional-profiles`: move the Claude adapter (`guard.py`, `.claude/`, agent-surface checks, `docs/agent-surfaces.md`) to `extras/adapters/`, and the GitHub App identity, last-push rule, `identity` job and workflow handoff to `extras/hardened-host/`. Move `change-execution` and `verification-map` skills to extras.
4. `docs-consolidate`: one operator path in `README.md`, `AGENTS.md` and `changes/README.md`; narrow the local-file provenance and smoke-probe triggers; withdraw the host-setup target that drops human review in favour of `unverified` local claims.

## Files and components that change

- `scripts/repo.py`
  - Delete: `CLOSURE`, `CLOSURE_REFRESH`, `CLOSURE_KEYS`, `Objects`, `tree_packet`, `tree_file`, `Context.closure_commits`, `Context.objects`, `validate_closure`, `find_closure_anchor`, `removed_frozen`, `check_closure`, `check_anchored_closure`, `check_candidate_closure`, `evidence_problem`, `evidence_ref`, `check_verify_evidence`, `closure_record`, `cmd_close`, the `close` subparser, `ChangeStatus.closure` and `ChangeStatus.anchor`. Delete `packet_digest`, `worktree_packet` and `Context.packet_paths` if nothing else uses them.
  - Add: `Context.merged_ids`, the set of packet ids whose `change.json` exists on the baseline tip, read with one `git ls-tree` call. Missing baseline ref gives an empty set.
  - `evaluate_change`: a merged packet gets `stage=merged`, runs only `validate_change`, and skips approvals-staleness, placeholder, unbound-scope and baseline checks. Open packets behave as today without the closure branch. `stage` values become `intent | spec | plan | implementation | merged`; `freshness` becomes `current | stale`.
  - `compute_status`: other packets fail the gate only on `invalid-change` and `scope-path-escape`. Drop the removed-frozen failure.
  - `pull_request_failures`: replace "needs a candidate closure" with "every carried `intent.md`/`spec.md` equals the root file" (code `carried-unmerged`). Drop the "touches other packets" rule; `check_scope` already rejects other packets' paths unless `write_scope` names them.
  - `print_status` and `as_dict`: drop `closure` and `closure_anchor`; fold healthy merged packets into one `merged: N` line, as frozen packets fold today.
  - `approval_claim`: refuse to approve a merged packet (replaces the anchor refusal).
  - Module docstring: list `status`, `verify`, `new`, `approve`.
- `scripts/hooks/guard.py`: replace the frozen rule (baseline has `closure.json`) with a merged rule (baseline has `change.json`), message `merged-packet`. Delete the `closure.json` deny rule. `PACKET_FILE` and the shell packet-write rule cover `change.json` only.
- Tests:
  - `scripts/tests/test_status.py`: delete `Closure`, `ClosureAndReadiness` and the anchor/batched-read class (lines 874-1039 today), the closure cases in the pull-request class, and frozen folding cases. Add the cases under "Tests and proof".
  - `scripts/tests/test_approve_close.py`: delete the `Close`, `RevisionAfterClosure` and `CarriedRevisionAfterClosure` classes; keep and adapt approval tests; replace the frozen-refusal case with a merged-refusal case.
  - `scripts/tests/test_new.py`: the quick-start case ends at a ready change whose pull request passes, not at a candidate closure.
  - `scripts/tests/test_guard.py`: frozen and closure cases become merged-packet cases; drop the closure shell-write cases and the `repo.py close` ask case.
  - `scripts/tests/support.py`: delete the closure-writing helper; add a helper that commits a packet to the baseline branch.
  - `scripts/tests/test_support.py` (added to `write_scope` in revision 2): delete `InProcessIsolation.test_no_object_process_outlives_the_call` and its `object_children` helper. Its subject, the `git cat-file --batch` process that `approve` and `close` opened, no longer exists. The rest of the file is unchanged.
- `changes/`: `git rm` the 45 packet directories that carry `closure.json`. Keep `changes/README.md`, `changes/_template/` and this packet. Rewrite `changes/README.md`: packet contents without `closure.json`, the reduced state model, a Lifecycle that ends at "pull request merges", and a short "After merge" section that replaces Closure (merged packets stay; edits need scope; undo is a new change that may be a plain revert).
- `README.md`: workflow diagram without `closure.json`; delete Quick start step 2 (packet removal before protection); renumber.
- `AGENTS.md`: drop the `close` command; Definition of done requires `status` passing and a complete `verify --full` without referring to `close`; guard sentence says merged packets instead of frozen packets and closures.
- `.agents/skills/sdlc-artifacts/SKILL.md`: Close section becomes "Finish": verify, push, open the pull request; drop `git add closure.json`; "When CI is red" drops "and `close`".
- `.agents/skills/change-execution/references/worker-brief.md`: drop `closure.json` from the forbidden list (packets stay forbidden).
- `scripts/README.md`, `scripts/hooks/README.md`: match the code.
- `docs/host-setup.md`: drop the closure-anchor reasons for squash-only and no force pushes (keep both settings, give the remaining reason); the workflow handoff's last agent push is "the next commit", not "the closure commit". Nothing else in host setup changes here.
- `.gitattributes`: comment mentions approval claims only.

Not changed here: `.github/workflows/repository.yml` keeps `fetch-depth: 0`, which the diff base still needs; its comment about closure anchors goes stale and is fixed in `ci-native-tests`, to avoid a workflow handoff in this change.

## Order of work

Each step ends with the command that proves it.

1. Tag the last pre-change commit locally for reference: `git tag pre-core-lifecycle bccaed3`. It is not pushed. Proof: `git rev-parse pre-core-lifecycle`.
2. `scripts/repo.py`: add `merged_ids` and the merged stage; remove the closure path from `evaluate_change`, `compute_status`, `pull_request_failures`, printing and `approve`. Proof: `python3 scripts/repo.py status --change core-lifecycle` runs and still lists the 45 old packets as merged.
3. `scripts/repo.py`: delete the closure, anchor, Git-object and `close` code that step 2 left unused. Proof: `python3 -m pyflakes scripts/repo.py` if available, otherwise `python3 -c "import ast,sys; ast.parse(open('scripts/repo.py').read())"` and `grep -n -i "closure\|anchor\|frozen" scripts/repo.py` returns nothing.
4. Tests for `repo.py`: update `support.py`, `test_status.py`, `test_approve_close.py`, `test_new.py`. Proof: `python3 -m unittest scripts.tests.test_status scripts.tests.test_approve_close scripts.tests.test_new`.
5. `scripts/hooks/guard.py` and `test_guard.py`. Proof: `python3 -m unittest scripts.tests.test_guard`.
6. Delete the 45 closed packets: `git rm -rq` every `changes/<id>/` that has `closure.json`. Proof: `ls changes` shows only `README.md`, `_template`, `core-lifecycle`; `python3 scripts/repo.py status --change core-lifecycle` passes.
7. Documentation: `changes/README.md`, `README.md`, `AGENTS.md`, both skill files, `scripts/README.md`, `scripts/hooks/README.md`, `docs/host-setup.md`, `.gitattributes`. Proof: `git grep -n -i -E "closure|frozen|anchor|repo.py close" -- ':!changes/core-lifecycle/'` returns only intended hits (none expected outside the stale workflow comment).
8. Full run. Proof: `python3 scripts/repo.py verify --change core-lifecycle --full` reports every check `passed`, and `python3 scripts/repo.py status --change core-lifecycle` passes.
9. The `sdlc-artifacts` skill asks for the `docs/agent-surfaces.md` probes when `AGENTS.md` or a skill changes. The owner waived them for this change; the pull request description says so. Proof: none; `docs/agent-surfaces.md` is unchanged.

## Tests and proof

New or rewritten `repo.py status` cases:

- A packet committed on the baseline branch reports `stage=merged`, and later root `spec.md` changes do not make it stale or failing.
- An open change that edits a merged packet outside its `write_scope` fails `out-of-scope`; the same edit inside `write_scope` passes scope.
- A pull request whose ready change carries `spec.md` fails `carried-unmerged` until root `spec.md` equals the packet copy, then passes, with no `closure.json` anywhere.
- A pull request for a ready `implementation` change with in-scope diff passes.
- Deleting merged packets inside `write_scope` passes; outside it fails `out-of-scope`.
- `repo.py close` is no longer a command (argparse error, exit 2).
- `approve` refuses a merged packet.
- `status` without a resolvable baseline ref treats no packet as merged and still reports `diff-base` for the active change.
- Existing approval-order, stale-approval, scope-binding, placeholder, baseline-freshness, identity and `verify` tests keep passing unchanged.

Guard cases: file-tool edit to a merged packet denied with `merged-packet`; edit to an open packet allowed; `approvals` edits still denied; shell writes to `change.json` still ask.

Whole-change proof: `python3 scripts/repo.py verify --change core-lifecycle --full` (complete, all `passed`) and CI `summary` green on the pull request. CI runs the head's `repo.py`, so this pull request needs no `closure.json`.

## Risks and mitigations

- **Loss of guarantee: merged packets are no longer byte-checked against their merge commit.** A later pull request can edit or delete one if its approved `write_scope` allows it. Mitigation: the owner approves `write_scope`; the diff shows the edit; the guard still denies file-tool edits to merged packets in Claude Code. Accepted by choosing this direction.
- **Loss of guarantee: no enforced full local run before a pull request qualifies.** CI runs the full registered checks on the candidate and the owner reviews it. `verify` evidence stays available as a local record.
- **Wide `changes/**` scope** in this change, needed to delete 45 packets. Mitigation: step 6 deletes only directories that contain `closure.json`; the review checks the deleted list against `git ls-tree` on `bccaed3`.
- **Downstream repositories that already adopted the template** keep their `closure.json` files. After upgrading `repo.py`, those files are ordinary packet files and nothing reads them; no failure. Note it in the pull request.
- **Large test deletion** could hide a lost behaviour. Mitigation: only tests whose subject is closure, anchors, frozen packets or `close` are deleted; every other test file and class stays and must pass; the new cases above cover the replacement rules.
- **Riskiest step: step 2**, because merged detection changes which packets are judged. Mitigation: before step 6 deletes them, `status` must report all 45 old packets as `merged` with no failures, which exercises the rule on real data.

## Rollback or recovery

After this change merges, undoing it is a new change whose plan restores the files from the squash commit's parent; because merged packets are no longer frozen, a plain `git revert` of the squash commit is an acceptable implementation of that change. The deleted packets are recoverable from Git history and the local `pre-core-lifecycle` tag.

## Decisions

The owner settled these before approval:

1. The 45 closed packets are deleted in this change (step 6).
2. The `pre-core-lifecycle` tag stays local; it is not pushed.
3. The agent-surface probe is waived for this change (step 9), because `optional-profiles` moves that procedure.
4. The unapproved `harness-main-spec` packet is dropped; its branch is deleted unmerged.

## Revisions

- **Revision 2.** Implementation found one test outside the original `write_scope` whose subject this change removes: `scripts/tests/test_support.py`, `test_no_object_process_outlives_the_call`. Every other test passed (238 tests, 2 subtest failures, all in that one test). This revision adds `scripts/tests/test_support.py` to `write_scope` and the Files section; nothing else in the plan changes.

## Open questions

None.

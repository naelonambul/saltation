# Changes

Each unit of work is a **change packet**, `changes/<id>/`. The root `intent.md` and `spec.md` are the durable product baseline. There is no root plan: an active plan lives only in its packet.

`python3 scripts/repo.py status` is the index. It computes every packet's state from repository facts and authored claims. Nothing it reports is stored as a writable label.

## Packet contents

| File | Purpose |
|---|---|
| `change.json` | Authored claims only: `id`, `kind`, `base`, the `baseline` digests it was authored against, `write_scope`, and `approvals`. |
| `intent.md` | Full proposed next root `intent.md`. Only for kinds that change intent. |
| `spec.md` | Full proposed next root `spec.md`. Only for kinds that change behavior or contracts. |
| `plan.md` | How the change is implemented and proven. |

Create a packet with `python3 scripts/repo.py new <id> --kind <kind> --title <summary> [--scope <pattern>]...`, run from the change's branch point, because it records `HEAD` as `base.commit`. It creates no branch, commit or approval; `status` remains the judge of the result. Copying `changes/_template/` by hand is the fallback. For a change-local `intent.md` or `spec.md`, copy the current root file and edit the copy. Once approved, it replaces the root file whole in the change's own pull request, which makes "merged into the baseline" checkable byte for byte.

## Kinds

| Kind | Required | Optional | Inherits root baseline |
|---|---|---|---|
| `product-init` | intent, spec, plan | none | no: establishes it |
| `intent` | intent, plan | spec | yes |
| `behavior` | spec, plan | none | yes |
| `implementation` | plan | none | yes |
| `incident` | plan | spec | yes; the plan starts with a reproduction that fails on the base (see the `sdlc-artifacts` skill, Incidents) |
| `architecture` | plan | spec | yes |
| `repository` | plan | none | no: process and tooling only; may not modify an established root intent or spec |

Root `intent.md` and `spec.md` start with the `<!-- sdlc:baseline-unestablished -->` marker. The first product change is a `product-init` change, and it removes the marker when its accepted copies are merged. Until then, kinds that inherit the baseline are blocked. A root `intent.md` and `spec.md` without the marker are already established, and `product-init` is not run again.

## Computed state

```text
stage:      intent | spec | plan | implementation | merged
freshness:  current | stale
approval:   none | unverified | verified
readiness:  blocked | ready
```

- **Order.** Approvals follow `intent -> spec -> plan`, both in chain position and in timestamp order. A downstream approval without its upstream approval blocks.
- **Freshness.** `stale` means a recorded `baseline` digest no longer matches the root file, or an approval is bound to bytes that have since changed. A carried artifact is compared with the baseline branch's file, because the change replaces its own branch's copy. A stale change is blocked until it is refreshed and re-approved.
- **Readiness.** `ready` means every chain artifact carries a current approval and nothing blocks. While a change is not ready, only its own packet may change. Any other changed path blocks.
- **Merged.** A packet whose `change.json` is on the baseline branch's tip is `merged`: its pull request closed it. It is not judged again for approval, freshness or readiness, and later root changes do not make it stale. A later change that edits or deletes it needs that path in its own approved `write_scope`.

## Approval claims

```json
{"artifact": "plan.md", "sha256": "sha256:<digest printed by status>", "by": "<owner>", "at": "2026-09-23T10:00:00Z"}
```

A `plan.md` claim also carries `"write_scope_sha256"`, so widening the scope after approval makes the claim stale. Pattern order does not matter. `repo.py approve` writes it; re-running `approve` rebinds a changed scope without an artifact edit. A `plan.md` claim without it blocks an open packet with `unbound-scope`. Merged packets are not affected.

Only the human owner adds a claim, after reviewing exactly those bytes. `python3 scripts/repo.py approve <id> <artifact> --by <owner>` writes it for the current bytes and refuses out-of-order approvals; `repo.py status` prints each artifact's current digest for a claim written by hand. In Claude Code, the in-session guard (`scripts/hooks/guard.py`) denies file-tool edits to `approvals` and asks before an agent runs `approve`; elsewhere `status` detects a malformed or stale claim after the fact.

A digest-bound claim yields `approval=unverified`. The digest detects staleness and accidental mismatch. **It is not identity proof**: anyone, an agent included, can compute it. A claim without `sha256` counts as `none`. In solo/manual mode `unverified` is enough for readiness.

`verified` is reserved for a provider path that proves a distinct human approval under merge enforcement the agent identity cannot bypass. The template has no such path, so it never reports `verified`.

An agent-side hook or tool policy that refuses agent edits to `approvals` adds friction and an audit trail. It is still not authentication, because an agent that can edit the policy can bypass it.

## Identity and write scope

- **Local.** `--change <id>` is authoritative. A branch named `change/<id>` or `change/<id>/...`, or a diff that touches exactly one packet, is a convenience inference only. Disagreement or ambiguity blocks.
- **Pull request.** The body carries `Change-ID: <id>`. CI passes it explicitly, and a missing, repeated or unknown ID blocks. One PR is one change: `status` in this mode fails unless the named packet is ready, its diff stays in scope, and every change-local `intent.md`/`spec.md` it carries equals the root file (`carried-unmerged`). Another packet's paths count as out of scope unless `write_scope` names them.
- **Scope.** Every changed path between the base (the PR base, or the merge-base with the baseline branch) and the candidate must fall inside `changes/<id>/` or match a `write_scope` pattern (`*`, `?`, `**`, trailing `/`). Patterns are relative POSIX paths with no `.` or `..` segments. `repo.py new` adds each root artifact the packet carries, so its approved copy can replace the root.

## Lifecycle

The ordered path for every change, with what `python3 scripts/repo.py status --change <id>` reports:

| Step | Action | Expected status |
|---|---|---|
| 1 | Branch from the updated baseline; create the packet; draft artifacts in chain order | `stage=<first unapproved artifact>`, `readiness=blocked` |
| 2 | The owner approves each artifact in order with `repo.py approve`; the agent commits and pushes the claim | after the last: `stage=implementation readiness=ready` |
| 3 | Implement inside `write_scope`; run `repo.py verify --change <id>` while iterating | unchanged |
| 4 | Replace the root `intent.md`/`spec.md` with each accepted change-local copy; run `repo.py verify --change <id> --full` | unchanged |
| 5 | The agent commits, pushes, and opens one pull request with `Change-ID: <id>`; record the verify result in it | the CI `status` job applies the pull-request gate |
| 6 | CI `summary` passes on the head; the owner reviews the pull request and squash-merges it | — |
| 7 | After merge | `stage=merged` |

- **Evidence.** `verify` writes `.evidence/<run>/evidence.json`, bound to HEAD and a digest of the files it ran on (paths, modes and content, leaving out `.evidence/`). It is the author's local record. CI runs the checks again on the candidate, and the required `summary` check is the merge gate. Any commit after a `verify` needs a new one before you report the result.
- **Fixes before merge.** If CI or review needs another commit, edit, run `verify --change <id> --full` again, and push. A change to an artifact or `write_scope` needs the owner's approval again, in chain order.
- **After merge.** The merged pull request is the record: Git keeps the packet, the commit and its review. Record follow-up work as a new change. To undo a merged change, open a new change whose plan restores the files; a plain `git revert` of the squash commit is an acceptable way to implement it. If the undone change altered the baseline, use a kind that carries the restored `spec.md` or `intent.md` for approval.

`status` needs history back to the change's base to compute its diff, so CI fetches full history.

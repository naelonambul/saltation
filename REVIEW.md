# Review Policy

Review the implementation against the human owner's explicit decisions, `AGENTS.md`, the root `intent.md` and `spec.md` baseline, and the approved change packet (`changes/<id>/`) named by the pull request's `Change-ID`.

## Review passes

1. **Correctness and regressions**
   - Find logic errors, broken edge cases, unintended behavior changes, and incomplete implementation.
2. **Artifact compliance**
   - Confirm the change solves the approved intent, satisfies the approved spec, and follows the approved plan.
   - Flag any undocumented material plan deviation.
3. **Security, privacy, and safety**
   - Find exposed secrets, unsafe input handling, authorization gaps, privacy leaks, insecure defaults, and dangerous operational behavior relevant to the change.
4. **Verification quality**
   - Confirm tests and other proof actually demonstrate the required behavior, and that `repo.py verify` evidence exists for the reviewed revision.
   - Flag skipped, weakened, deleted, or misleading checks.
5. **Maintainability**
   - Report complexity, duplication, or architectural damage only when it creates a concrete future cost or defect risk.

## Finding levels

- **Important**: could break required behavior, cause a regression, violate approved artifacts or repository policy, create a security/privacy problem, or invalidate verification.
- **Nit**: non-blocking style or local cleanup. Report at most five nits; summarize additional minor issues instead of flooding the review.

Do not spend review budget repeating formatting, lint, or other findings that deterministic repository checks already enforce unless the deterministic check itself is missing or broken.

## Evidence

For each substantive finding:

- identify the affected file or behavior;
- explain why it conflicts with the approved artifacts or repository behavior;
- state the likely impact;
- propose the smallest reasonable correction when clear.

Do not invent findings to fill a quota. A clean review may report no substantive findings.

## Separation of duties

The agent that authored a change may self-check it, and should, but its own review is never the human gate. The owner's gates are the digest-bound `repo.py approve` claims on the artifacts and the owner's review and merge of the pull request. A claim in `change.json` is `unverified` process metadata: it binds bytes, it does not prove who approved. Prefer a fresh-context or different-model review for the final independent pass when practical.

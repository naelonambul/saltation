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
   - Flag measured performance results of Oracle Database (throughput, latency, wait times, plan statistics such as A-Time or buffers) in committed files, pull request bodies or comments. They stay in the ignored `.evidence/` directory; public text states the outcome only (OTN license benchmark-disclosure clause).
4. **Verification quality**
   - Confirm tests and other proof actually demonstrate the required behavior, and that `repo.py verify` evidence exists for the reviewed revision.
   - Flag skipped, weakened, deleted, or misleading checks.
5. **Maintainability**
   - Report complexity, duplication, or architectural damage only when it creates a concrete future cost or defect risk.

## Finding levels

- **Important**: could break required behavior, cause a regression, violate approved artifacts or repository policy, create a security/privacy problem, or invalidate verification.
  Always Important, whatever the diff's size:
  - a `comment-exceptions.json` entry whose reason explains, defends or postpones our own code (a workaround, "for now", "do not remove", a complexity note, history, a TODO), or whose category does not fit. The fix is to change the code and remove the entry, never to reword the reason;
  - a `checks.json` gate set to `none` whose reason does not hold for the product, such as "later" or "not set up yet".
- **Nit**: non-blocking style or local cleanup. Report at most five nits; summarize additional minor issues instead of flooding the review.

Do not spend review budget repeating formatting, lint, or other findings that deterministic repository checks already enforce unless the deterministic check itself is missing or broken.

## Evidence

For each substantive finding:

- identify the affected file or behavior;
- explain why it conflicts with the approved artifacts or repository behavior;
- state the likely impact;
- propose the smallest reasonable correction when clear.

Do not invent findings to fill a quota. A clean review may report no substantive findings.

### Performance and resource claims

For a change claiming improved performance, throughput, or resource use, the approved plan identifies the baseline revision, workload, environment, metrics, and acceptance criteria. Measure the baseline and candidate under the same workload and environment. Compare the results against the approved criteria and record errors and any output consistency checks in the final review and PR.

Keep raw benchmark results in `.evidence/benchmarks/<change-id>/<run-id>/`. Include the baseline and candidate measurements in the final review and PR, with links to artifacts reviewers can access when available, subject to the Oracle Database disclosure rule above.

## Final review record

After verifying the implementation commit, write `changes/<id>/review.md`. Record the reviewed commit, verification evidence references, findings and their disposition, and the review conclusion. If the implementation changes, verify and review the updated revision and refresh the record. After committing the review record, verify the complete PR head and report that run separately in the PR.

## Separation of duties

The agent that authored a change may self-check it, and should, but its own review is never the human gate. The owner's gates are the digest-bound `repo.py approve` claims on the artifacts and the owner's review and merge of the pull request. A claim in `change.json` is `unverified` process metadata: it binds bytes, it does not prove who approved. Prefer a fresh-context or different-model review for the final independent pass when practical.

# Implementation Plan

## Summary

The Oracle Technology Network License Agreement, under which this project's Oracle Database 19c is used, forbids disclosing "results of any Program benchmark tests without Oracle's prior consent". This repository is public. The owner decided on 2026-11-16 that measured Oracle performance numbers stay private: they are kept only in the ignored `.evidence/` directory, and committed files, pull request bodies and comments state outcomes without numbers.

This change removes the measured results from the merged `oracledb-driver` plan and adds the rule to `REVIEW.md` so that later reviews catch it. Git history keeps the old text; the owner chose not to rewrite history.

The pull request #13 body was already rewritten without numbers (2026-11-16), outside this change, because PR text is not repository content.

## Files and components that change

- `changes/oracledb-driver/plan.md`, step 5, "Paced comparison" paragraph only: replace the measured values (the closed-loop p95 increase, the four paced p95 pairs with their rates, and the per-request Oracle counter values) with a qualitative statement of the same finding. The thresholds and method (70% of baseline throughput, the 20% limits, the 10 ms late-start rule) are rules, not results, and stay.
- `REVIEW.md`, pass 3 "Security, privacy, and safety": one bullet stating the rule.

Nothing else changes. Requirement thresholds in `spec.md` (N-1 to N-3) are targets, not measured results, and stay.

## Order of work

1. Edit the paragraph to read, in substance: "On the OCI host the new driver's closed-loop p95 was higher at concurrency 8 and above while its throughput was higher, but at an equal fixed rate (one run each) it was lower. Oracle counters over the measured window were identical per request on both drivers, and `log file sync` averaged lower on the new driver. So latency is compared at a fixed rate:" Keep the following bullets unchanged.
2. Add to `REVIEW.md` pass 3: "Flag measured performance results of Oracle Database (throughput, latency, wait times, plan statistics such as A-Time or buffers) in committed files, pull request bodies or comments. They stay in the ignored `.evidence/` directory; public text states the outcome only (OTN license benchmark-disclosure clause)."
3. Proof: `git diff origin/main` touches only the two files; `git grep -nE '[0-9]+(\.[0-9]+)? ?ms|[0-9]+/s' -- changes/oracledb-driver/plan.md` shows only the method lines (the 200 ms default acquire timeout, warm-up and measured seconds, the 10 ms late-start rule, `BENCH_RATE=<requests/s>`); `python3 scripts/repo.py status --change redact-bench-numbers` ok; `python3 scripts/repo.py verify --change redact-bench-numbers --full` with no failed, blocked or not-run check.

## Tests and proof

As in step 3. No code changes; existing checks must stay green.

## Risks and mitigations

- **Editing a merged packet's plan breaks its approval digest.** Merged packets are not judged again for approval (`changes/README.md`, Computed state), so status is unaffected. The edit is in this change's approved `write_scope`, as the README requires.
- **The numbers stay in Git history and in GitHub's edit history of PR #13.** Accepted by the owner (option 1). The owner can delete the PR body's earlier revisions from its "edited" menu.

## Rollback or recovery

Revert the squash commit.

## Open questions

None.

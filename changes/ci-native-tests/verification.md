# Verification

Local record for `ci-native-tests`. CI on the pull request runs the checks again and decides.

- `python3 scripts/repo.py verify --change ci-native-tests --full` at `7d2a33d` (clean tree): complete, `control-plane` passed. `run.py` reported `Ran 213 tests in 34.074s`, `OK`.
- `python3 scripts/repo.py status --change ci-native-tests`: `readiness=ready`, `status: ok`, no `checks-unwired` finding with `verify-core` removed.
- `ruff check --select F scripts/`: clean.
- Before this change: the parallel runner took 9 s and the sequential native run 30 s (in-process) or 41 s (subprocess), on a 10-core Mac.

This file replaces the plan's "Verification section" note in Order of work step 6. Writing the result into `plan.md` would have made the owner's approval stale.

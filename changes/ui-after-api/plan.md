# Implementation Plan

## Summary

The owner decided that UI work starts only after the API server is developed. The spec's Delivery table bundled the admin screens with the admin interface (stage 4) and named no stage for the mobile screens of F-38. This change rewrites the table: stages 2 to 4 are API server work only, generator scenarios stay stage 5, a new stage 6 builds all remaining screens (mobile and admin), and operations becomes stage 7.

Only the Delivery section of `spec.md` changes: one sentence and the table. No requirement, interface, acceptance criterion or code changes.

Decision: the UI stage comes after the generator scenarios, as the last feature stage, because the generator needs no screen and the operations stage (tunnel exposure, the full run, acceptance criterion 8) needs the screens.

## Files and components that change

<!-- Must be covered by change.json write_scope. -->

| Path | Change |
|---|---|
| `spec.md` | replaced whole by this packet's approved `spec.md` (Delivery section only differs) |

## Order of work

1. Replace the root `spec.md` with the approved packet copy.
   - Proof: `python3 scripts/repo.py status --change ui-after-api` passes, and `git diff main -- spec.md` shows only the Delivery section.
2. Run `python3 scripts/repo.py verify --change ui-after-api --full`, push, and open the pull request with `Change-ID: ui-after-api`.

## Tests and proof

The full verify and the CI `summary` check. There is no behaviour to test.

## Risks and mitigations

- **Open change `ledger-products` (stage 2 plan, not yet approved) was drafted against the current spec.** Merging this makes its recorded baseline stale; it is refreshed onto the new `main` before its approval. Its plan already carries no UI work.

## Rollback or recovery

Revert the squash commit.

## Open questions

- None.

# Implementation Plan

## Summary

Replace the generic `# SDLC Project Template` root `README.md` with a comprehensive project and engineering deep-dive README for **Saltation**, and add a full Korean counterpart in `README.ko.md` linked via a top-level language switcher (`English | 한국어`).

The new documentation introduces the *Dune*-inspired geological etymology of *Saltation* (sand grains leaping across desert dunes and revealing unseen movement beneath the surface), includes Shields.io technology-stack and architecture badges, color-styled Mermaid architecture and transaction diagrams, an engineering deep-dive covering the Rust bank server (`oracledb` session pool, plain SQL, single-entry ledger, transactional outbox, simulated clock and day rollover), the 3 banking channels (KFTC Open Banking v3.3.6, ATM, card network), the synthetic generator (personas, fraud scenarios A–G, lookalikes, isolated ground truth), repository layout, quick start, verification commands, delivery roadmap, and the AI-Native SDLC control plane.

## Files and components that change

<!-- Must be covered by change.json write_scope. -->

| Path | Change |
|---|---|
| `README.md` | Replace the template README with the English Saltation project showcase and engineering deep-dive |
| `README.ko.md` | Create the full Korean translation matching `README.md` section-for-section |

## Order of work

1. Write the English project documentation in `README.md` and the Korean counterpart in `README.ko.md` with bidirectional language links, technology badges, *Dune*-inspired etymology, color-styled Mermaid diagrams, engineering deep-dive sections, scenario matrix, directory structure, quick start, verification gates, delivery roadmap, and SDLC overview.
   - Proof: `python3 scripts/repo.py status --change readme-overhaul` passes with all modified files inside `write_scope`.
2. Run the full verification suite and confirm all checks pass.
   - Proof: `python3 scripts/repo.py verify --change readme-overhaul --full` completes with zero `failed`, `blocked`, or `not-run` checks.

## Tests and proof

- `python3 scripts/repo.py status --change readme-overhaul`
- `python3 scripts/repo.py verify --change readme-overhaul --full`

## Risks and mitigations

- **Control-plane tests referencing `README.md`**: Checked `scripts/tests/` (`test_guard.py` and `test_verify.py`); those tests operate on temporary fixture directories rather than the repository's root `README.md`. Full verification will confirm `control-plane` passes.
- **OTN benchmark restriction**: Neither `README.md` nor `README.ko.md` discloses measured Oracle benchmark numbers; performance targets cite only the specification's N-1 to N-9 requirements and qualitative plan/wait-event findings.

## Rollback or recovery

Revert the squash commit or restore `README.md` from `main` and remove `README.ko.md`.

## Open questions

- None.

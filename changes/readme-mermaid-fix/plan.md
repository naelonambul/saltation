# Implementation Plan

## Summary

Fix the Mermaid `sequenceDiagram` parse error in `README.md` under `1. Single-Entry KRW Ledger & Lock Ordering`.

In Mermaid's `sequenceDiagram` grammar, a semicolon (`;`) terminates a statement, so `; COMMIT` at the end of lines 161, 164, and 170 causes the parser to treat `COMMIT` as a new participant statement expecting an arrow token. Replacing `; COMMIT` with `and COMMIT` resolves the parse error and matches the Korean `README.ko.md` diagram (`... 후 COMMIT`).

## Files and components that change

<!-- Must be covered by change.json write_scope. -->

| Path | Change |
|---|---|
| `README.md` | Replace `; COMMIT` with `and COMMIT` in the three `sequenceDiagram` message labels (lines 161, 164, 170) |

## Order of work

1. Update the three message labels in the `sequenceDiagram` block of `README.md` to replace `; COMMIT` with `and COMMIT`.
   - Proof: `grep -n '; COMMIT' README.md` returns no matches and `python3 scripts/repo.py status --change readme-mermaid-fix` passes.
2. Run verification for the change.
   - Proof: `python3 scripts/repo.py verify --change readme-mermaid-fix --full` (or routed checks) passes.

## Tests and proof

- `python3 scripts/repo.py status --change readme-mermaid-fix`
- `python3 scripts/repo.py verify --change readme-mermaid-fix`

## Risks and mitigations

- None; documentation-only fix to a Mermaid diagram block.

## Rollback or recovery

Revert the squash commit.

## Open questions

- None.

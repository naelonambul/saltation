# Implementation Plan

## Summary

Remove the `<sub>` etymology caption beneath the hero banner in `README.md` and `README.ko.md`, and replace the `# Saltation` heading with `# sal·ta·tion (/sælˈteɪʃən/)`.

## Files and components that change

<!-- Must be covered by change.json write_scope. -->

- `README.md`: Remove the `<sub>...</sub>` block and `<br /><br />` spacers beneath the hero banner image, and change `# Saltation` to `# sal·ta·tion (/sælˈteɪʃən/)`.
- `README.ko.md`: Mirror the same change—remove the `<sub>...</sub>` block and `<br /><br />` spacers beneath the hero banner image, and change `# Saltation` to `# sal·ta·tion (/sælˈteɪʃən/)`.

## Order of work

1. Edit `README.md` and `README.ko.md` to remove the `<sub>` caption block under the banner image and replace `# Saltation` with `# sal·ta·tion (/sælˈteɪʃən/)`.
   - Proof: `head -n 12 README.md README.ko.md` confirms the banner is followed directly by `# sal·ta·tion (/sælˈteɪʃən/)` with no `<sub>` caption remaining.
2. Run repository status and routed verification (docs-only change touching only `README.md`, `README.ko.md`, and the active packet).
   - Proof: `python3 scripts/repo.py status --change readme-title` reports `status: ok` and `python3 scripts/repo.py verify --change readme-title` passes all routed checks (`control-plane`).

## Tests and proof

- `head -n 12 README.md README.ko.md`
- `python3 scripts/repo.py status --change readme-title`
- `python3 scripts/repo.py verify --change readme-title`

## Risks and mitigations

- **Markdown heading rendering**: `# sal·ta·tion (/sælˈteɪʃən/)` uses standard Unicode IPA characters supported by GitHub Markdown inside `<div align="center">`.
- **Unintended edits outside scope**: `write_scope` is strictly limited to `README.md` and `README.ko.md`.

## Rollback or recovery

Revert the PR commit (`git revert <sha>`) to restore the previous `README.md` and `README.ko.md` headers.

## Open questions

None.

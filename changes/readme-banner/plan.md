# Implementation Plan

## Summary

Add the owner-provided wide desert dune hero banner image (`docs/assets/saltation-banner.jpg`, 62 KB) to the very top of `README.md` and `README.ko.md` (above `# Saltation`), and move the existing `sal·ta·tion` etymology quote directly beneath the banner image as a small centered caption (`<sub>`), removing the duplicate blockquote section below the badges.

## Files and components that change

<!-- Must be covered by change.json write_scope. -->

- `docs/assets/saltation-banner.jpg`: New hero banner image (62 KB JPEG, wide cinematic desert dunes with thumper and rising sand).
- `README.md`: Place `<img src="docs/assets/saltation-banner.jpg" ... width="100%" />` at the top of the centered header block, move the `sal·ta·tion` quote directly under the image inside `<sub>`, and remove the old duplicate `> **sal·ta·tion** ...` blockquote block between the badges and Table of Contents.
- `README.ko.md`: Mirror the same hero banner and small `<sub>` caption placement in Korean, and remove the old duplicate blockquote block.

## Order of work

1. Copy the updated desert banner image into `docs/assets/saltation-banner.jpg`.
   - Proof: `ls -lh docs/assets/saltation-banner.jpg` confirms the file exists and is under 300 KB.
2. Update `README.md` and `README.ko.md` to render the hero banner above `# Saltation`, place the `sal·ta·tion` quote directly beneath the image in `<sub>`, and remove the old blockquote below the badges.
   - Proof: `grep -n "saltation-banner.jpg" README.md README.ko.md` confirms both READMEs reference the relative asset path, and `git diff README.md README.ko.md` confirms no duplicate etymology quote remains.
3. Run repository status and full verification.
   - Proof: `python3 scripts/repo.py status --change readme-banner` reports `status: ok` and `CARGO_BUILD_JOBS=2 python3 scripts/repo.py verify --change readme-banner --full` passes all registered checks.

## Tests and proof

- `ls -lh docs/assets/saltation-banner.jpg`
- `grep -n "saltation-banner.jpg" README.md README.ko.md`
- `python3 scripts/repo.py status --change readme-banner`
- `CARGO_BUILD_JOBS=2 python3 scripts/repo.py verify --change readme-banner --full`

## Risks and mitigations

- **Broken relative image links on GitHub**: Both `README.md` and `README.ko.md` sit at repository root and reference `docs/assets/saltation-banner.jpg` via a clean relative POSIX path.
- **Excessive vertical height on initial load**: The image is a wide ~2.4:1 aspect ratio at 62 KB, so full-width rendering keeps `# Saltation` and the subtitle visible without heavy scroll push-down or slow page load.
- **Unintended edits outside scope**: `write_scope` is strictly limited to `README.md`, `README.ko.md`, and `docs/assets/**`.

## Rollback or recovery

Revert the PR commit (`git revert <sha>`) to remove `docs/assets/saltation-banner.jpg` and restore the previous `README.md` and `README.ko.md` headers.

## Open questions

None.

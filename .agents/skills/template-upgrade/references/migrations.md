# Template migrations

One section per template change that needs action beyond copying files. `repo.py upgrade` lists the sections whose packet merged between the base and the target, in merge order. A template change that adds a `status` error must add a section here; a template test enforces it. The oldest supported base is `51ca588`.

## scripts-comment-free

PR #55.

- **What fails.** `control-plane` runs `scripts/tests/test_comments.py`, which fails on every comment and docstring in tracked `scripts/**.py`, including the repository's own Python under `scripts/` (for example `scripts/dev/`).
- **Action.** Remove those comments, or move product Python out of `scripts/`. A comment that carries a real constraint becomes a name, an assertion message, a check or a test first.

## codebase-rules-gate

PR #56.

- **What fails.** Once any product path is tracked, `status` fails with `gates-undecided` until `checks.json` `gates` decides all seven roles (`build`, `test`, `lint`, `format`, `types`, `boundaries`, `comments`), or with `checks-missing` when there is no `checks.json`. A `comment-exceptions.json`, if present, is checked and fails with `comment-exceptions-invalid`.
- **Action.** Decide every gate in the upgrade packet: a registered check id, or `{"none": "<reason>"}` only where the role truly does not apply. For `comments` in a repository that already has comments, register a check that fails only on new comments (the `repository-quality` skill), not a `none` gate. See "Decide the gates" and "Comments" in that skill.

## template-upgrade

This change.

- **What changes.** `checks.json` may carry `"template": {"commit", "digest", "carried"}`. When it does, `status` fails with `template-drift` if a copy file differs from the pin.
- **Action.** Record the `pin` value the report prints, at the end of this upgrade.

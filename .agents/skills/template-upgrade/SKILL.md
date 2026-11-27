---
name: template-upgrade
description: Upgrade a repository built on this template to a later template commit. Use when the owner asks to take template changes, when `repo.py status` reports `template-drift`, or when recording or checking the `template` pin in `checks.json`.
---

# Template Upgrade

`repo.py upgrade` computes what an upgrade changes, and `repo.py status` enforces the pin afterwards. This skill covers what the tool cannot decide.

The template's files fall into three groups. `repo.py` defines the first two (`UPGRADE_COPY`, `UPGRADE_MERGE`):

- **copy**: the control plane, its tests, the guard, `changes/README.md`, `changes/_template/` and the core skills. `references/owned-files.txt` lists every file the template owns there. Never edited locally, except paths listed in `template.carried`. `status` fails with `template-drift`, naming the files, when an owned file differs from the pin. The repository's own files inside those directories are `local`: never pinned and never touched.
- **merge**: `AGENTS.md`, `REVIEW.md`, `checks.json`, `.claude/settings.json`, the workflow, the PR template, `.gitignore`, `.gitattributes`. Merged by hand, keeping the product's own lines.
- **never**: the template's `README.md`, `LICENSE`, `docs/`, `intent.md`, `spec.md` and its `changes/<id>/` packets.

The oldest supported base is `51ca5887988429289bb33f03fa39faf5b67c2c28`.

## 1. Report

Clone the template next to the repository, never inside it, and run the **target's** `repo.py` from the repository root, so a repository whose own copy predates the command can still run it:

```bash
git clone <template url> ../sdlc-template
python3 ../sdlc-template/scripts/repo.py upgrade --template ../sdlc-template --to <tag or commit> [--from <base>]
```

- The base is `checks.json` `template.commit`. Without a pin, take it from the repository's own record of what it adopted. Without one, run without `--from`: the command prints the commits with the most matching copy files, oldest first, and stops. Take the oldest, and confirm it with the owner unless it matches fully. A base that is too new silently skips migrations.
- The report compares the Git index, so stage or commit local work first. It needs a full clone, not a shallow one.
- Read every section: `drift`, `carried`, `copy`, `merge`, `local`, `migrations`, `pin`. A file already at the target is not listed again. The command writes nothing.

## 2. Dry run

Apply the upgrade on a scratch clone of the repository, then run `repo.py status` and `verify --full` there. Approval and scope findings are expected, because no packet exists yet. Every other failure is upgrade work for the plan.

## 3. Packet

One `repository` change per upgrade, for example `template-upgrade-<short commit>`.

- `write_scope`: the copy and carried files the report lists, the merge files it lists, `checks.json` always (pin and gates), and every path a migration entry names.
- The plan records the base and target commits, each migration and how it is resolved, and each drifted or carried file and what happens to it.

## 4. Drift

A drifted copy file is never silently overwritten. Choose one, with the owner:

1. Upstream the rule to the template, then upgrade to it.
2. Move it out of the copy group: a registered check or a wrapper elsewhere.
3. List the path in `template.carried`. The report then prints a three-way merge for it on every upgrade, so the edit stays visible. On the first upgrade there is no digest yet: write `"template": {"commit": "<base>", "digest": "sha256:" followed by 64 zeros, "carried": [...]}`, run the report, and record the pin it prints.

## 5. Apply

Per file, from the report:

- `added` and `changed`: create the parent directory, then `git -C <clone> show <to>:<path> > <path>`. For an entry marked `(executable)`, also `chmod +x`. For one marked `(symlink)`, remove the path and run `ln -s "$(git -C <clone> show <to>:<path>)" <path>` instead. An entry marked `collides` overwrites a file of the repository's own: move that file out first.
- `removed`: `git rm` exactly the listed files. Never delete a whole directory; `local` files stay.
- `merge` and `carried`: run the printed `git merge-file` recipe (bash or zsh), resolve conflicts, and keep the product's own lines. A carried file `removed upstream` is deleted, or kept as a local file, which the pin then ignores.
- `git add` everything. `status` reads the index.

## 6. Migrations

Do each entry the report lists, in order. They live in `references/migrations.md` at the target.

## 7. Workflow

If `.github/workflows/repository.yml` changed, the agent identity cannot push it. Follow the repository's workflow handoff (`docs/host-setup.md` in the template); without one, the owner pushes the workflow commit first.

## 8. Close

- Write the report's `pin` value to `checks.json` as `"template"`.
  Once it is recorded, the template's own completeness tests in `scripts/tests/test_upgrade.py` are skipped; the pin covers what they check.
- `repo.py status --change <id>` is ok and `repo.py verify --change <id> --full` is complete.
- One pull request, as for any change.

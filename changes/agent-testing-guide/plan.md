# Headless testing guide for agents

## Summary

Give an agent with a fresh clone one entry point to verify the banking API and synthetic generator using the existing test harnesses. Identify what the checks prove, what requires the Oracle image, and what remains outside the scope of headless tests.

## Scope

- Add `docs/agent-testing.md` as the canonical guide.
- Link to it from the verification sections of `README.md` and `README.ko.md`.
- Keep the existing application, test harnesses, contracts, acceptance criteria, and CI definitions.

## Order of work and proof

1. Move the integrated reviewer draft, `reviewed-guide.md`, to `docs/agent-testing.md` and add the README links. Check links, Bash snippets, target names, and expected evidence against `checks.json`, `scripts/dev/it-oracle.sh`, and the existing contracts.
2. Follow the guide in a fresh disposable integration project. Verify the bank through the documented scenario harness, record an exit status and Oracle/MongoDB evidence, and confirm cleanup retains local logs. Report any image or host prerequisites that prevent the check.
3. Run `python3 scripts/repo.py status --change agent-testing-guide` and run `python3 scripts/repo.py verify --change agent-testing-guide --full` with a fresh disposable project exported as in the guide. Record the approved scope, exact commands and outputs in the PR.

## Risks and recovery

Use a unique Compose project and its dedicated state directory per run. Keep the shared integration project and existing volumes out of the examples. Keep credentials, database dumps, and measured Oracle numbers out of shared evidence. A guide rollback reverts only the guide and its links.

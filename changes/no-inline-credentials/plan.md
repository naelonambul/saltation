# Implementation Plan

## Summary

GitGuardian flagged "Generic Password" in two files merged with `product-init`. No real secret leaked, but both patterns should not be in the repository:

- `infra/oracle/smoke.sh` hard-codes throwaway passwords (`SmokeSys_1`, `SmokeApp_1`) for its temporary database.
- `infra/compose/compose.yml` builds MongoDB connection URIs with the user and password inline (`mongodb://bank:${MONGO_BANK_PASSWORD}@mongo...`), which secret scanners read as credentials.

This change removes both patterns without changing behaviour. It follows the approved spec's security constraint "Secrets come from an untracked `.env`; no secret is committed" and changes no requirement.

## Files and components that change

<!-- Must be covered by change.json write_scope. -->

| Path | Change |
|---|---|
| `infra/oracle/smoke.sh` | generate random passwords per run with Python's `secrets`; use them for the container and the schema login check |
| `infra/compose/compose.yml` | the `bank` and `generator` services receive `MONGO_HOST`, `MONGO_USER`, `MONGO_PASSWORD` and `MONGO_DB` instead of a URI with credentials |
| `bank/src/main.rs` | `mongo_uri()`: use `MONGO_URI` when set (tests), otherwise build the URI from the four variables, percent-encoding user and password |
| `generator/saltation_gen/config/__init__.py` | `_truth_uri()`: the same rule for the generator (`MONGO_TRUTH_URI` or the four variables) |

## Order of work

1. Apply the four edits.
   - Proof: `scripts/dev/ci-rust.sh fmt`, `scripts/dev/ci-rust.sh clippy`, `scripts/dev/ci-py.sh lint` and `scripts/dev/ci-py.sh test` pass.
2. Show the bank still delivers events to MongoDB with the split variables.
   - Proof: `scripts/dev/it-oracle.sh observability` passes, the bank container logs no "MONGO_URI is not set" warning, and `bank_logs.events` grows.
3. Show the smoke test still works with generated passwords.
   - Proof: `infra/oracle/smoke.sh` prints the N-7 settings and "schema login ok".
4. Show no inline credential remains.
   - Proof: `grep -nE "SmokeSys|SmokeApp|mongodb://" infra/oracle/smoke.sh infra/compose/compose.yml` prints nothing.
5. Run `python3 scripts/repo.py verify --change no-inline-credentials --full` untouched, then open the pull request with `Change-ID: no-inline-credentials`.

## Tests and proof

The proofs above; the CI `summary` check, including `oracle` on the self-hosted runner. The GitGuardian check on the pull request should report no new findings.

## Risks and mitigations

- A password containing URI-reserved characters would break a hand-built URI. Mitigation: user and password are percent-encoded in both the bank and the generator.
- `.env` files created before this change keep working: the variable names in `.env` are unchanged (`MONGO_BANK_PASSWORD`, `MONGO_GENERATOR_PASSWORD`).

## Rollback or recovery

Revert the squash commit; behaviour returns to inline URIs.

## Open questions

- None. The GitGuardian incidents on already-pushed commits stay in history; the owner marks them as false positives in the GitGuardian dashboard.

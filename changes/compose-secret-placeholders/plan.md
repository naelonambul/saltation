# Implementation Plan

## Summary

GitGuardian still reports "Generic Password" on `infra/compose/compose.yml` line 72 after `no-inline-credentials`:

```yaml
MONGO_PASSWORD: ${MONGO_BANK_PASSWORD:?set in .env}
```

The key ends in `PASSWORD` and the value carries the literal message text `set in .env`, which the detector reads as a password. Nine secret entries use the same form (lines 15, 16, 41, 42, 43, 68, 72, 127, 194). This change drops the message text from every secret placeholder: `${VAR:?set in .env}` becomes `${VAR:?}`. Compose still refuses to start when the variable is unset or empty; only the error text changes to Compose's default. Non-secret placeholders keep their messages.

No behaviour or requirement changes.

## Files and components that change

<!-- Must be covered by change.json write_scope. -->

| Path | Change |
|---|---|
| `infra/compose/compose.yml` | the nine secret placeholders use `${VAR:?}` |

## Order of work

1. Replace the nine placeholders.
   - Proof: `grep -c ':?set in .env' infra/compose/compose.yml` prints 0 for the secret entries (`grep -nE 'PASSWORD|PWD' infra/compose/compose.yml | grep -c 'set in'` prints 0).
2. Show the required-variable check still works.
   - Proof: with `.env.example`, `docker compose -f infra/compose/compose.yml --env-file infra/compose/.env.example --profile local --profile tools config -q` succeeds; with an env file lacking `MONGO_BANK_PASSWORD`, the same command fails naming that variable.
3. Run `python3 scripts/repo.py verify --change compose-secret-placeholders --full` untouched, then open the pull request with `Change-ID: compose-secret-placeholders`.

## Tests and proof

The proofs above; the CI `summary` check; the GitGuardian check on the pull request reports no findings.

## Risks and mitigations

- If GitGuardian still flags the key alone, the next step is the owner marking the entry as a false positive; the value is a variable reference, not a secret.

## Rollback or recovery

Revert the squash commit.

## Open questions

- None.

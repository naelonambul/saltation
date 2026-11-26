# Plan — qualify-n8-and-loopback-ports

## Implementation steps

1. **Update root baseline `spec.md`**
   - Update N-8 to **Container stability**: require zero OOM-kills, zero restarts, and sustained health status across all Saltation containers without requiring legacy external co-tenant containers.

2. **Update host verifier and tests (`loadtest/`)**
   - In `loadtest/saltation_loadtest/verify_host.py`, set `CO_TENANTS = frozenset()`.
   - In `loadtest/tests/test_verify_host.py`, update inventory count assertions (`8` core Saltation containers by default) and adjust tests that previously relied on default `jenkins`, `npm`, or `postgresql-postgresql-1` containers so they exercise Saltation core container restart/replacement/missing/inactive checks directly.
   - In `loadtest/README.md`, remove the outdated mention of required co-tenant containers.

3. **Expose loopback generator and metrics ports (`infra/compose/compose.yml`, `scripts/dev/it-oracle.sh`)**
   - In `infra/compose/compose.yml`, publish `${BANK_GENERATOR_PUBLISH:-127.0.0.1:8081}:8081` and `${BANK_METRICS_PUBLISH:-127.0.0.1:9100}:9100` alongside `${BANK_PUBLISH:-127.0.0.1:8080}:8080` on the `bank` service.
   - In `scripts/dev/it-oracle.sh`, add `BANK_GENERATOR_PUBLISH=127.0.0.1:` and `BANK_METRICS_PUBLISH=127.0.0.1:` to the generated `.env` (and ensure existing prepared CI `.env` files also get them if missing) so CI stacks never collide with ports `8081`/`9100` on shared hosts.

## Verification

- Run `uv run --project loadtest pytest` and `uv run --project loadtest ruff check` / `ruff format --check`.
- Run `python3 scripts/dev/check_compose_budget.py static`.
- Run `python3 scripts/repo.py status --change qualify-n8-and-loopback-ports` and `python3 scripts/repo.py verify --change qualify-n8-and-loopback-ports --full`.
- Review diff against `REVIEW.md` and open a PR under the agent identity (`agent-git` / `agent-gh`).

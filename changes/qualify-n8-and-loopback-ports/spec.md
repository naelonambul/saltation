# Specification — qualify-n8-and-loopback-ports

## Delta against root `spec.md`

### N-8 (Container stability)

Replace the current N-8 requirement in root `spec.md`:

> - N-8. **Container stability:** during the backfill, the 24-hour live run and load tests, no Saltation container is OOM-killed or restarted, and all Saltation containers keep their health status.

External co-tenant containers (`sonarqube-sonarqube-1`, `postgresql-postgresql-1`, `qdrant-qdrant-1`, `npm`, `jenkins`, `cloudflared-oci-vpc`) are no longer required on the primary OCI host or verified by the qualification harness.

### Qualification loopback bindings

Publish `bank`'s private generator listener (`:8081`) and metrics listener (`:9100`) on IPv4 loopback in `infra/compose/compose.yml` using overridable `BANK_GENERATOR_PUBLISH` (default `127.0.0.1:8081`) and `BANK_METRICS_PUBLISH` (default `127.0.0.1:9100`) variables, matching `BANK_PUBLISH` and `ORACLE_PUBLISH`.

- Listeners remain loopback-only (`127.0.0.1`), never attached to public or tunnel ingress.
- `scripts/dev/it-oracle.sh` sets `BANK_GENERATOR_PUBLISH=127.0.0.1:` and `BANK_METRICS_PUBLISH=127.0.0.1:` so CI integration stacks bind ephemeral loopback ports without colliding with a running dev stack on shared hosts.

## Verification

- `loadtest/saltation_loadtest/verify_host.py` sets `CO_TENANTS = frozenset()` so `HostVerifier` requires only `CORE_SERVICES` (plus any active `OPTIONAL_SERVICES`) and does not fail or mark runs `INCOMPLETE` when legacy co-tenant containers are absent or stopped.
- `loadtest/tests/test_verify_host.py` verifies complete inspection of the 8 core Saltation containers, detects Saltation OOM/restart/health/replacement/missing-core violations, and still covers optional `CO_TENANTS` behavior when configured in tests.
- `loadtest/README.md` removes the obsolete co-tenant prerequisite sentence.
- `python3 scripts/repo.py verify --change qualify-n8-and-loopback-ports --full` passes with zero failed or blocked checks.

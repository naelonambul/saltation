# Implementation Plan

## Summary

Apply the OCI-validated Candidate A static container CPU/memory allocation (`4.00 vCPU / 13.96 GB`, within Spec N-7 `<= 4 vCPU, <= 14 GB`) and storage guardrails recommended in `docs/stage8-oci-sizing-report.md` Section 6 item 3:

1. **Candidate A static limits in `infra/compose/compose.yml`:**
   - `oracle`: `1.50` → `1.05 vCPU` (`6g` unchanged)
   - `mongo`: `0.40` → `0.45 vCPU` (`3g` unchanged)
   - `bank`: `0.75` → `1.30 vCPU` (`1g` unchanged), `SALTATION_ARGON2_CONCURRENCY` default `2` → `3` (OWASP `19456/2/1` unchanged)
   - `web`: `0.10` → `0.05 vCPU` (`128m` unchanged)
   - `generator`: `0.50` → `0.65 vCPU` (`768m` unchanged)
   - `loadtest`: `0.75` → `0.65 vCPU` (`768m` unchanged)
   - Observability (`alloy 0.10`, `loki 0.10`, `prometheus 0.10`, `grafana 0.05`) + external connector reservation (`0.15 vCPU / 128m`) unchanged.
   - Resulting totals: `server` = `3.35 vCPU / 12.25 GiB`; `generator` and `loadtest` phases = `4.00 vCPU / 13.00 GiB (13,958,643,712 B <= 14,000,000,000 B)`.
2. **Storage guardrails:**
   - `infra/compose/compose.yml` (`prometheus`): add `--storage.tsdb.retention.size=2GB` alongside `--storage.tsdb.retention.time=15d`.
   - `infra/oracle/setup/01_saltation.sql`: cap `saltation_data01.dbf` at `MAXSIZE 30G` (instead of `MAXSIZE UNLIMITED`, safely below Oracle 19c's 8 KiB smallfile ceiling of `32,767 MiB`).

## Files and components that change

- `infra/compose/compose.yml`
- `infra/oracle/setup/01_saltation.sql`

## Order of work

1. Update CPU limits, `SALTATION_ARGON2_CONCURRENCY` default (`3`), and Prometheus `--storage.tsdb.retention.size=2GB` in `infra/compose/compose.yml`.
2. Update `saltation_data01.dbf` `MAXSIZE UNLIMITED` → `MAXSIZE 30G` in `infra/oracle/setup/01_saltation.sql`.
3. Verify locally with `python3 scripts/repo.py status --change compose-candidate-a-guardrails` and `python3 scripts/repo.py verify --change compose-candidate-a-guardrails --group core` + `--group python` (`ops-observability`, `ops-compose-budget-tests`), and validate `compose-config`, `compose-budget`, and `oracle-integration` in GitHub Actions CI on OCI.

## Tests and proof

- `python3 scripts/repo.py status --change compose-candidate-a-guardrails` reports `readiness=ready`, `status: ok`.
- `python3 scripts/repo.py verify --change compose-candidate-a-guardrails --group core` and `--group python` pass (`control-plane`, `ops-observability`, `ops-compose-budget-tests`).
- GitHub Actions `repository/oracle` job on OCI runs `compose-config`, `compose-budget` (verifying `<= 4.00 vCPU` and `<= 14 GB` across all three phases), and full `oracle-integration` with the new limits.

## Risks and mitigations

- **Risk:** `oracle` setup script change (`01_saltation.sql`) triggers a fresh Oracle setup key in `it-oracle.sh` on OCI CI, and specifying `32G` on an 8 KiB smallfile tablespace can raise `ORA-03206` (max smallfile size is `4,194,303 * 8 KiB ≈ 32,767.99 MiB`).
  - **Mitigation:** Cap at `MAXSIZE 30G`, safely below the 8 KiB smallfile limit while bounding disk growth on the 150 GB OCI volume.
- **Risk:** `loadtest` CPU reduction (`0.75` → `0.65`) could affect budget or load generation.
  - **Mitigation:** Candidate A (`0.65 vCPU` for `loadtest`) already passed exploratory N-1/N-2/N-3 probes through 200 TPS (`docs/stage8-oci-sizing-report.md` Section 3).

## Rollback or recovery

Revert the commit; existing Oracle volumes keep their current datafile settings until recreated.

## Open questions

None.

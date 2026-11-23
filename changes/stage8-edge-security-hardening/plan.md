# Harden Stage 8 edge security negative tests and sync spec N-8/F-34

## Summary

Close the final items from the repository reviews and host rename:

1. **Edge security negative test suite (`scripts/ops/security_negative_test.py`):**
   - Omit `device_id` from the spoofed login probe body so a leaked `X-Sim-Device` header on the public listener is not masked by `req.device_id.or_else(|| ctx.device_id)` (`bank/src/features/auth/mod.rs`).
   - Project `device_id` from `bank_logs.events` and require persisted `device_id is None` as well as non-empty trusted `ip` and `region` (rejecting empty/missing fields and spoofed values).
   - Add an unauthenticated `/admin/v1/customers` negative check expecting `401` or `403`.
   - Report `--skip-mongo` runs as `"status": "partial"` with `"passed": false` (exit code `2`) instead of claiming full `"passed": true`.
2. **Cloudflare Access documentation (`infra/cloudflare/README.md`):** Document Cloudflare Access protection for `/admin/v1/**` per `spec.md` § Security.
3. **Baseline spec & host verifier sync (`spec.md`, `loadtest/saltation_loadtest/verify_host.py`):**
   - Update N-8 co-tenant container name from `cloudflared-oci-podobank-api` to `cloudflared-oci-vpc` across `spec.md` and `verify_host.py`.
   - Record the already owner-approved F-34 `term_open` payer-only pre-labelling rule (`changes/generator-bc-proof/plan.md`) in `spec.md` (F-34 and Acceptance Criterion #5).

## Files and components that change

| Paths | Work |
| --- | --- |
| `spec.md` | Adopt approved change-local `spec.md` updating N-8 (`cloudflared-oci-vpc`) and F-34 / AC #5 (`term_open` pre-label rule). |
| `scripts/ops/security_negative_test.py` | Omit body `device_id` in login probe, project `device_id`, validate `device_id is None` and non-empty `ip`/`region`, add unauthenticated `/admin/v1/customers` check (`401`/`403`), and mark `--skip-mongo` output as partial (`passed: false`, exit `2`). |
| `scripts/ops/test_security_negative.py` | Add unit tests for `device_id` leak detection, empty `ip`/`region` rejection, `/admin/v1/**` denial evaluation, and `--skip-mongo` partial verdict. |
| `infra/cloudflare/README.md` | Document Cloudflare Access policy for `/admin/v1/**` and updated negative test coverage. |
| `loadtest/saltation_loadtest/verify_host.py`, `loadtest/tests/test_verify_host.py` | Rename default co-tenant `cloudflared-oci-podobank-api` to `cloudflared-oci-vpc`. |

## Order of work

1. **Harden `scripts/ops/security_negative_test.py` and tests.** Remove body `device_id` from the spoofed login probe, project `device_id` from MongoDB, enforce `device_id is None` and non-empty `ip`/`region` in `evaluate_spoofed_event`, add `evaluate_admin_unauthenticated` (`401`/`403`) on `GET /admin/v1/customers`, and emit `"status": "partial", "passed": false` (exit `2`) when `--skip-mongo` is used. Update `scripts/ops/test_security_negative.py`. Proof: `PYTHONDONTWRITEBYTECODE=1 python3 -m unittest scripts/ops/test_security_negative.py -v`.
2. **Document `/admin/v1/**` Cloudflare Access policy.** Update `infra/cloudflare/README.md`. Proof: review against `spec.md` § Security.
3. **Sync `cloudflared-oci-vpc` and root `spec.md`.** Update `CO_TENANTS` in `loadtest/saltation_loadtest/verify_host.py` and `loadtest/tests/test_verify_host.py`, and copy the approved change-local `spec.md` to root `spec.md`. Proof: `pytest loadtest/tests`, `ruff check loadtest`, and `python3 scripts/repo.py status --change stage8-edge-security-hardening`.

## Tests and proof

- `PYTHONDONTWRITEBYTECODE=1 python3 -m unittest scripts/ops/test_security_negative.py -v`
- `pytest loadtest/tests` and `ruff check loadtest`
- `python3 scripts/repo.py status --change stage8-edge-security-hardening`
- CI `control-plane`, `python`, `status`, `identity`, `summary` checks.

## Risks and mitigations

- **Existing OCI container name:** `cloudflared-oci-vpc` is already live on the primary OCI host; updating `spec.md` and `verify_host.py` removes drift before the next Stage 8 run.
- **`--skip-mongo` exit code change:** Returning exit `2` for partial runs prevents CI or operators from mistaking an HTTP-only probe for a complete Stage 8 security pass.

## Rollback or recovery

Revert the scoped files if needed; no database schema or runtime API behavior is modified.

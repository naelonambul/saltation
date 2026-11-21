# Add Cloudflare Workers VPC edge config and Stage 8 security negative test suite

## Summary

Deliver the Cloudflare edge exposure configuration and automated security negative test suite required by Stage 8 (`spec.md` Security & Acceptance Criterion #12). Version-control the Cloudflare Workers VPC proxy scripts (`sal` for `web`, `sal-api` for the bank public listener forwarding `CF-Connecting-IP` and `CF-IPCountry`) and deployment documentation under `infra/cloudflare/`, and provide a replayable Python negative-test runner (`scripts/ops/security_negative_test.py`) with unit tests (`scripts/ops/test_security_negative.py`) proving:

1. Public health and CORS preflight succeed through the tunnel (`sal` and `sal-api`).
2. Restricted routes (`/atm/v1/session`, `/card/v1/authorize`, `/internal/v1/clock`, `/v2.0/transfer/deposit/fin_num`, `/metrics`) return 404 through the tunnel.
3. Simulated context headers (`X-Sim-Ip`, `X-Sim-Region`, `X-Sim-Device`) are ignored on the public listener, recording `listener: "public"`, `human: true`, and the real Cloudflare IP/country in `bank_logs.events`.
4. MongoDB RBAC isolation holds (`bank` denied on `truth`, `generator` denied on `bank_logs`).

## Files and components that change

| Path | Change |
| --- | --- |
| `infra/cloudflare/README.md`, `infra/cloudflare/workers/sal.js`, `infra/cloudflare/workers/sal-api.js` | Document the Cloudflare Tunnel + Workers VPC topology (`50080`/`50081`) and commit the exact Worker proxy scripts (no tokens, account IDs, or secrets). |
| `scripts/ops/security_negative_test.py` | Replayable stdlib Python CLI that runs all Stage 8 negative tests (tunnel route isolation, CORS, `X-Sim-*` header spoofing check against `bank_logs.events`, and MongoDB RBAC isolation via `docker exec -e` env credentials) and exits non-zero on any failure. |
| `scripts/ops/test_security_negative.py` | Unit tests verifying route-check evaluation, spoofed-header event verification, RBAC assertion parsing, and Worker script invariants without requiring live network or Docker access. |
| `checks.json` | Register `ops-security-negative` under the `python` group routed to `infra/cloudflare/**`, `scripts/ops/security_negative_test.py`, and `scripts/ops/test_security_negative.py`. |

## Order of work

1. **Add `infra/cloudflare/` Worker scripts and edge documentation** (`infra/cloudflare/README.md`, `infra/cloudflare/workers/sal.js`, `infra/cloudflare/workers/sal-api.js`).
   - Keep all secrets, tunnel tokens, and account identifiers out of committed files.
   - Ensure `sal-api.js` forwards `CF-Connecting-IP` and `CF-IPCountry` and uses `redirect: "manual"`.
   - Proof: `python3 -m unittest scripts/ops/test_security_negative.py -v`.

2. **Implement automated Stage 8 security negative test suite and CI check** (`scripts/ops/security_negative_test.py`, `scripts/ops/test_security_negative.py`, `checks.json`).
   - Pass MongoDB credentials exclusively via `docker exec -e` environment variables (never CLI flags).
   - Register `ops-security-negative` in `checks.json` under the `python` group.
   - Proof: `python3 scripts/repo.py status --change stage8-edge-security`, `python3 scripts/repo.py verify --change stage8-edge-security --group core`, and `python3 scripts/repo.py verify --change stage8-edge-security --group python`.

3. **Run live end-to-end negative tests against `https://sal.i234.workers.dev` and `https://sal-api.i234.workers.dev` on OCI**
   - Execute `python3 scripts/ops/security_negative_test.py --web-url https://sal.i234.workers.dev --api-url https://sal-api.i234.workers.dev` on OCI and confirm 100% pass.

## Risks and mitigations

- **Accidental secret or token exposure in Worker config or scripts.** Store no account IDs, tunnel tokens, or passwords in `infra/cloudflare/` or `scripts/ops/`; read MongoDB credentials from `infra/compose/.env` via environment variables only.
- **Public listener accidentally accepting spoofed headers.** Verify the actual persisted `bank_logs.events` document for the probe's `request_id` rather than trusting HTTP status alone.

## Rollback or recovery

Remove `infra/cloudflare/`, `scripts/ops/security_negative_test.py`, `scripts/ops/test_security_negative.py`, and the `checks.json` entry. No runtime bank or database state is altered.

## Open questions

None.

# Plan — document-loopback-trust-topology

## Context

GPT-5.6 Pro audit finding `F-003` and GPT-6 Astra Pro §9: root `spec.md:232-233` still states `"No Saltation port is published on the host"`, which contradicts the deliberate IPv4 loopback publications in `infra/compose/compose.yml` (`oracle:1521`, `mongo:27017`, `bank:8080/8081/9100`, `web:8088`, `grafana:3000`) and leaves the host-local trust boundary of the `:8081` generator listener undocumented. `spec.md:237` also still names the obsolete `oracle` crate instead of the pure-Rust `oracledb` crate (`spec.md:369,385`).

## Implementation steps

1. **Carry approved `spec.md` into root baseline (`spec.md`)**
   - Copy approved `changes/document-loopback-trust-topology/spec.md` to root `spec.md` so the durable baseline documents:
     - Edge/deployed public ingress restrictions (Cloudflare TLS + Access, no non-loopback or tunnel exposure of sensitive listeners);
     - Host-local IPv4 loopback (`127.0.0.1`) port publications and the explicit host-local trust boundary for `bank:8081` (simulated headers + `/internal/**`, seeding, inbound, ATM, card routes);
     - Local and CI ephemeral loopback profiles (`*_PUBLISH=127.0.0.1:`);
     - Pure-Rust `oracledb` crate alignment in the data-access section.

2. **Add static loopback-only publication guard (`scripts/ops/test_security_negative.py`)**
   - Add a deterministic unit test (comment-free, per `scripts-comment-free`) that parses `infra/compose/compose.yml` `ports:` entries and asserts every default host publication binds strictly to `127.0.0.1` (never `0.0.0.0`, `::`, or an unbound host port), protecting the documented loopback-only invariant against future Compose regressions.

## Verification

- Run `python3 -m unittest scripts/ops/test_security_negative.py scripts/tests/test_comments.py -v`.
- Run `python3 scripts/repo.py status --change document-loopback-trust-topology --base redact-oracle-backed-benchmark-results`.
- Commit locally on branch `document-loopback-trust-topology` (no push while OCI 365-day backfill is active).

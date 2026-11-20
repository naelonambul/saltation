# Implementation Plan

## Summary

Split the frontend static `nginx` server (`:8088`) out of `saltation-bank` into a dedicated `saltation-web` VM (Ubuntu 24.04, 2 vCPU / 2 GB RAM, `groups = ["web"]`, `lan = true`) under `infra/lab/`, making the Docker-free VMware lab topology a 5-VM layout (14 vCPU / 20 GB RAM total) that mirrors the Docker Compose service separation 1:1.

## Files and components that change

<!-- Must be covered by change.json write_scope. -->

| Path | Change |
|---|---|
| `infra/lab/vms.tfvars.example` | Remove `"web"` from `saltation-bank`, add `saltation-web` (2 vCPU, 2 GB RAM, `ubuntu-24.04`, `lan = true`) |
| `infra/lab/ansible/site.yml` | Add `web` to common baseline; keep `saltation-bank` Rust-only; move Node 22, pnpm Mobile SPA build (`VITE_API_BASE` pointing at `saltation-bank:8080`), and `nginx` `:8088` deployment into a dedicated `hosts: web` play |
| `infra/lab/ansible/wire-and-smoke.yml` | Gather facts across all 5 VMs (`oracle:mongo:bank:web:generator`), wire `SALTATION_CORS_ORIGINS` on `saltation-bank` to `saltation-web:8088`, and verify `saltation-web:8088/healthz` over LAN |
| `infra/lab/README.md` | Update topology table (5 VMs, 14 vCPU / 20 GB RAM), step headings, and `web_lan_ip` override documentation |

## Order of work

1. Update `infra/lab/vms.tfvars.example` and `infra/lab/README.md` for the 5-VM layout.
   - Proof: `grep -q '"saltation-web"' infra/lab/vms.tfvars.example`.
2. Split `saltation-web` into its own play in `infra/lab/ansible/site.yml` and wire `SALTATION_CORS_ORIGINS` and `:8088/healthz` verification in `infra/lab/ansible/wire-and-smoke.yml`.
   - Proof: YAML parsing check and `ansible-playbook --syntax-check` pass for `site.yml`, `oracle.yml`, and `wire-and-smoke.yml`.
3. Run repository gates and verification:
   - `python3 scripts/repo.py status --change vmware-lab-web-vm`
   - `python3 scripts/repo.py verify --change vmware-lab-web-vm --full`

## Tests and proof

- YAML & `ansible-playbook --syntax-check` on `infra/lab/ansible/*.yml`
- `python3 scripts/repo.py status --change vmware-lab-web-vm`
- `python3 scripts/repo.py verify --change vmware-lab-web-vm --full`

## Risks and mitigations

- **Cross-origin wiring**: `saltation-web` builds with `VITE_API_BASE` pointing to `saltation-bank`'s `lan0` IP (`:8080`), and `wire-and-smoke.yml` configures `SALTATION_CORS_ORIGINS` on `saltation-bank` to allow `http://<web-lan0>:8088` (both overridable via `-e`).
- **No secrets or binaries**: No credentials, Oracle binaries, or Oracle benchmark numbers are touched or added.

## Rollback or recovery

Revert the squash commit; `saltation-bank` returns to co-hosting `nginx` on the 4-VM layout.

## Open questions

- None.

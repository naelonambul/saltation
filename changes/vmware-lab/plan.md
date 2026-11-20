# Implementation Plan

## Summary

Add the Docker-free, native `x86_64` 5-VM lab layout under `infra/lab/` (reserved in `spec.md` § Top-level modules and ownership) to deploy and validate Saltation on a Windows + WSL2 + VMware Workstation host using [`wsl-vmware-lab`](https://github.com/naelonambul/wsl-vmware-lab) (Terraform + Ansible), matching the Docker Compose service separation 1:1.

With `oraclelinux-8` (official Oracle Linux 8.10 KVM image + offline `mods.iso`) supported natively in `wsl-vmware-lab` (PR #61), all five `x86_64` VMs are provisioned and managed uniformly by Terraform (`vms.tfvars.example`) and configured by Ansible (`infra/lab/ansible/`), with no separate Oracle VM bootstrap script:

1. `saltation-oracle` (4 vCPU, 8 GB RAM, `image = "oraclelinux-8"`, `lan = true`): Oracle Database 19c EE installed natively via `oracle-database-preinstall-19c` and the owner-downloaded `oracle-database-ee-19c-1.0-1.x86_64.rpm`, creating `ORCLCDB`/`ORCLPDB1`, 3x 1 GB redo logs, 4 GB-capped undo, 2 GB PGA limit, `SALTATION_DATA`, 4 GB-capped `OUTBOX_TS`, and the `SALTATION` schema owner.
2. `saltation-mongo` (2 vCPU, 4 GB RAM, `image = "ubuntu-24.04"`, `lan = true`): MongoDB 8.0 native (`--auth`, WiredTiger 1.5 GB) with isolated `bank` (`bank_logs`) and `generator` (`truth`) users.
3. `saltation-bank` (4 vCPU, 4 GB RAM, `image = "ubuntu-24.04"`, `lan = true`): Rust `1.99.1` native `x86_64` release build (`/usr/local/bin/bank`) managed by `systemd` (`saltation-bank.service`: `:8080`, `:8081`, `:9100`).
4. `saltation-web` (2 vCPU, 2 GB RAM, `image = "ubuntu-24.04"`, `lan = true`): Node 22 + pnpm native build of the Mobile Web SPA served by `nginx` on `:8088` (static files only, `VITE_API_BASE` pointing at `saltation-bank:8080`).
5. `saltation-gen` (2 vCPU, 2 GB RAM, `image = "ubuntu-24.04"`, `lan = true`): Python 3.13 virtual environment via `uv 0.8.17` (`/opt/venv/bin/saltation-gen`).

## Files and components that change

<!-- Must be covered by change.json write_scope. -->

| Path | Change |
|---|---|
| `infra/lab/README.md` | Document prerequisites, unified 5-VM Terraform + Ansible workflow (`./lab setup`, `./lab up`, `./lab configure`), secrets handling, Oracle 19c RPM transfer, and E2E verification |
| `infra/lab/vms.tfvars.example` | Unified 5-VM Terraform definitions (`saltation-oracle` with `image = "oraclelinux-8"`, plus `saltation-mongo`, `saltation-bank`, `saltation-web`, `saltation-gen`) for `wsl-vmware-lab` |
| `infra/lab/secrets.yml.example` | Placeholder template for local lab credentials (real `secrets.yml` stays untracked outside git) |
| `infra/lab/ansible/site.yml` | Native Ansible playbook provisioning `saltation-mongo`, `saltation-bank` (Rust only), `saltation-web` (Node 22 + Mobile SPA + `nginx` `:8088`), and `saltation-gen` without Docker |
| `infra/lab/ansible/oracle.yml` | Native Ansible playbook targeting the `oracle` inventory group to install `oracle-database-preinstall-19c` and `oracle-database-ee-19c-1.0-1.x86_64.rpm`, configure `ORCLCDB`/`ORCLPDB1`, apply `oracle-init.sql`, set the `SALTATION` password, and verify `:1521/ORCLPDB1` TCP login |
| `infra/lab/ansible/oracle-init.sql` | Idempotent SQL script applying Saltation's N-7/F-36a storage caps (3x 1 GB redo, 4 GB undo, 2 GB PGA, `SALTATION_DATA`, 4 GB `OUTBOX_TS`, `SALTATION` user) |
| `infra/lab/ansible/wire-and-smoke.yml` | Ansible playbook wiring `saltation-bank.service` (`systemd`) to `saltation-oracle`, `saltation-mongo`, and `saltation-web` (`SALTATION_CORS_ORIGINS`, auto-discovering `lan0` IPs from inventory facts, overridable via `-e`), checking `:8080/healthz`, `:9100/metrics`, and `saltation-web:8088/healthz`, running a 20-customer seed + live transfer smoke test from `saltation-gen`, and asserting `bank_logs.events` and `truth.labels` counts |

## Order of work

1. Update `infra/lab/vms.tfvars.example` and `infra/lab/README.md` for the unified 5-VM Terraform (`oraclelinux-8` + 4x `ubuntu-24.04`, with `saltation-web` separated from `saltation-bank`) workflow.
   - Proof: `grep -q '"saltation-web"' infra/lab/vms.tfvars.example` and `grep -q 'image      = "oraclelinux-8"' infra/lab/vms.tfvars.example`.
2. Split `saltation-web` into its own play in `infra/lab/ansible/site.yml` and update `infra/lab/ansible/wire-and-smoke.yml` to gather facts across all 5 groups (`oracle`, `mongo`, `bank`, `web`, `generator`), wire `SALTATION_CORS_ORIGINS` to `saltation-web`, and verify `saltation-web:8088/healthz`.
   - Proof: YAML parsing check and `ansible-playbook --syntax-check` on `playdata` pass for all three playbooks.
3. Run repository gates and verification:
   - `python3 scripts/repo.py status --change vmware-lab`
   - `python3 scripts/repo.py verify --change vmware-lab --full`

## Tests and proof

- YAML & `ansible-playbook --syntax-check` validation on `infra/lab/ansible/*.yml`
- `python3 scripts/repo.py status --change vmware-lab`
- `python3 scripts/repo.py verify --change vmware-lab --full`

## Risks and mitigations

- **Secret leakage**: `secrets.yml.example` uses placeholder values only; all Ansible credential tasks use `no_log: true`, and `README.md` places the real `secrets.yml` outside git (`chmod 600`).
- **Oracle binary / OTN license**: Neither `oracle-database-ee-19c-1.0-1.x86_64.rpm` nor any measured Oracle benchmark numbers are committed; the owner provides the RPM path locally.
- **Oracle disk size**: Default `OL8U10` root LV is ~32 GB (`45G` disk recommended if running full backfill); documented in `infra/lab/README.md`.

## Rollback or recovery

Revert the squash commit; no existing application code, Docker Compose configuration, or baseline specification is modified.

## Open questions

- None.

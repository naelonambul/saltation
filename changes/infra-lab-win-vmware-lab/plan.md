# Implementation Plan

## Summary

Align `infra/lab/` documentation and Terraform variables template with `win-vmware-lab` (Windows PowerShell 5.1 + `vmrest` + `lab-controller` + single `VMnet0` Bridged LAN), matching the merged `rust-lakehouse-fds-baseline` (`e13929c`) and the verified 5-VM native x86-64 deployment on the verified Windows lab host. Keep existing Ansible playbooks (`site.yml`, `oracle.yml`, `wire-and-smoke.yml`, `oracle-init.sql`) and legacy `vms.tfvars.example` untouched.

## Files and components that change

- `infra/lab/vms.tfvars.example.json`: add a `win-vmware-lab`-compatible JSON template for the 5 native x86-64 Saltation VMs (`saltation-oracle` on `oraclelinux-8` with `disk_gb: 45`; `saltation-mongo`, `saltation-bank`, `saltation-web`, `saltation-gen` on `ubuntu-24.04` with `disk_gb: 30`), with placeholder `VMnet0` static IPs, gateway, and DNS.
- `infra/lab/README.md`: update setup, provisioning, secret staging (`lab-controller` `~/.config/saltation-lab/secrets.yml`), Oracle 19c RPM transfer, `.\lab.cmd configure` execution, and graceful shutdown (`gen/web/bank` -> `mongo/oracle`) instructions from `wsl-vmware-lab` to `win-vmware-lab`, while keeping native x86 calibration guidance intact and OTN-compliant.

## Order of work

1. Add `infra/lab/vms.tfvars.example.json`; prove valid JSON with `python3 -m json.tool infra/lab/vms.tfvars.example.json >/dev/null`.
2. Update `infra/lab/README.md` to document the `win-vmware-lab` workflow accurately against `infra/lab/ansible/{site.yml,oracle.yml,wire-and-smoke.yml}`; prove with `python3 scripts/repo.py status --change infra-lab-win-vmware-lab`.
3. Run `python3 scripts/repo.py verify --change infra-lab-win-vmware-lab` and run an independent read-only review using Codex `gpt-6.2-astra` (`medium`) against `REVIEW.md` and `main`, recording findings in `changes/infra-lab-win-vmware-lab/review.md`.

## Expected byproducts and cleanup

- `.evidence/<run>/` (gitignored, repo-owned): verification evidence from `scripts/repo.py verify`. Retain for `review.md`.

## Tests and proof

- `python3 -m json.tool infra/lab/vms.tfvars.example.json >/dev/null`
- `python3 scripts/repo.py status --change infra-lab-win-vmware-lab`
- `python3 scripts/repo.py verify --change infra-lab-win-vmware-lab`
- Independent read-only review via Codex `gpt-6.2-astra` (`medium`).

## Risks and mitigations

- Stale or inaccurate `win-vmware-lab` CLI flags or secret placement paths could mislead future operators: cross-check every command against the verified Windows lab host run and `win-vmware-lab` skill contract.
- Committing real hostnames, LAN IPs, or secrets: use RFC 5737 / clearly marked placeholder IPs (`192.0.2.x` or `192.168.0.x` example placeholders matching `win-vmware-lab` docs) and zero real credentials.

## Rollback or recovery

Revert the squash commit on `main`. No runtime code, playbook, or CI configuration changes.

## Open questions

None.

# Implementation Plan

## Summary

Add the Docker-free, native `x86_64` multi-VM lab layout under `infra/lab/` (reserved in `spec.md` § Top-level modules and ownership) to deploy and validate Saltation on a Windows + WSL2 + VMware Workstation host using `wsl-vmware-lab` (Terraform + Ansible).

The lab separates Saltation into four `x86_64` VMs over bridged LAN (`lan = true` on `VMnet0`) plus `VMnet1` (management) and `VMnet8` (NAT):

1. `saltation-oracle` (4 vCPU, 8 GB RAM, 45 GB disk, official **Oracle Linux 8.10** KVM image + separate `mods.iso` offline driver bundle): Oracle Database 19c EE installed natively via `oracle-database-preinstall-19c` and the owner-downloaded `oracle-database-ee-19c-1.0-1.x86_64.rpm`, creating `ORCLCDB`/`ORCLPDB1`, 3x 1 GB redo logs, 4 GB-capped undo, 2 GB PGA limit, `SALTATION_DATA`, 4 GB-capped `OUTBOX_TS`, and the `SALTATION` schema owner.
2. `saltation-mongo` (2 vCPU, 4 GB RAM, Ubuntu 24.04): MongoDB 8.0 native (`--auth`, WiredTiger 1.5 GB) with isolated `bank` (`bank_logs`) and `generator` (`truth`) users.
3. `saltation-bank` (4 vCPU, 4 GB RAM, Ubuntu 24.04): Rust `1.99.1` native `x86_64` release build (`/usr/local/bin/bank`) managed by `systemd` (`saltation-bank.service`), plus Node 22 + pnpm native build of the Mobile Web SPA served by `nginx` on `:8088`.
4. `saltation-gen` (2 vCPU, 2 GB RAM, Ubuntu 24.04): Python 3.13 virtual environment via `uv 0.8.17` (`/opt/venv/bin/saltation-gen`).

### Official Oracle Linux 8.10 KVM + `mods.iso` Design

Oracle's official `OL8U10_x86_64-kvm` cloud image ships only `kernel-uek-core`, which omits VMware NIC drivers (`vmxnet3`, `e1000`, `e1000e`). To keep the official image untouched without requiring `libguestfs`/`virt-customize` on the host, the Oracle VM uses two CD-ROMs:
- `ide1:0` (`seed.iso`, label `cidata`): standard cloud-init `user-data` / `meta-data`.
- `ide1:1` (`mods.iso`, label `OL8MODS`): built once per host from the matching official `kernel-uek-modules-<kver>.rpm` from `yum.oracle.com/repo/OracleLinux/OL8/UEKR7/x86_64/`.

On first boot, cloud-init mounts `OL8MODS`, installs `kernel-uek-modules`, runs `depmod -a && modprobe vmxnet3`, and brings up all three `vmxnet3` NICs via NetworkManager DHCP. After first boot, `mods.iso` can be detached and `vmxnet3` persists across reboots.

## Files and components that change

<!-- Must be covered by change.json write_scope. -->

| Path | Change |
|---|---|
| `infra/lab/README.md` | Document prerequisites, 4-VM topology, official OL8 + `mods.iso` bootstrap, secrets handling, Ansible deployment, and E2E verification |
| `infra/lab/vms.tfvars.example` | Terraform VM definitions for the Ubuntu VMs (`saltation-mongo`, `saltation-bank`, `saltation-gen`) used with `wsl-vmware-lab` |
| `infra/lab/secrets.yml.example` | Placeholder template for local lab credentials (real `secrets.yml` stays untracked outside git) |
| `infra/lab/ol8/build-oracle-vm.sh` | Reproducible WSL helper script that downloads the official OL8U10 KVM image + matching `kernel-uek-modules` RPM, builds `mods.iso` (`OL8MODS`) and `seed.iso` (`cidata`), resizes the disk to 45 GB, writes `saltation-oracle.vmx` (`efi`, `sata0`, `vmxnet3`, `ide1:0` + `ide1:1`), and boots the VM |
| `infra/lab/ansible/site.yml` | Main Ansible playbook provisioning `saltation-mongo`, `saltation-bank` (Rust + Web/nginx), and `saltation-gen` natively without Docker |
| `infra/lab/ansible/oracle.yml` | Ansible playbook installing `oracle-database-preinstall-19c` and the owner-provided `oracle-database-ee-19c-1.0-1.x86_64.rpm`, configuring `ORCLCDB`/`ORCLPDB1`, applying `oracle-init.sql`, setting `SALTATION` password, and verifying `:1521/ORCLPDB1` TCP login |
| `infra/lab/ansible/oracle-init.sql` | Idempotent SQL script applying Saltation's N-7/F-36a storage caps (3x 1 GB redo, 4 GB undo, 2 GB PGA, `SALTATION_DATA`, 4 GB `OUTBOX_TS`, `SALTATION` user) |
| `infra/lab/ansible/wire-and-smoke.yml` | Ansible playbook configuring `saltation-bank.service` (`systemd`), checking `:8080/healthz` and `:9100/metrics`, running a 20-customer seed + live transfer smoke test from `saltation-gen`, and asserting matching `bank_logs.events` and `truth.labels` counts on `saltation-mongo` |

## Order of work

1. Add `infra/lab/README.md`, `infra/lab/vms.tfvars.example`, `infra/lab/secrets.yml.example`, and `infra/lab/ol8/build-oracle-vm.sh` (official OL8 KVM + `mods.iso` bootstrap).
   - Proof: `bash -n infra/lab/ol8/build-oracle-vm.sh` passes.
2. Add the native Ansible playbooks and SQL under `infra/lab/ansible/` (`site.yml`, `oracle.yml`, `oracle-init.sql`, `wire-and-smoke.yml`) using variables for VM IPs and external `secrets.yml` (no hardcoded host IPs or real secrets).
   - Proof: Python YAML syntax validation (`python3 -c 'import yaml, glob; [yaml.safe_load(open(f)) for f in glob.glob("infra/lab/ansible/*.yml")]'`) passes and `ansible-playbook --syntax-check` on `playdata` passes.
3. Run repository gates and verification:
   - `python3 scripts/repo.py status --change vmware-lab`
   - `python3 scripts/repo.py verify --change vmware-lab --full`

## Tests and proof

- Shell syntax check: `bash -n infra/lab/ol8/build-oracle-vm.sh`
- YAML & Ansible syntax validation on `infra/lab/ansible/*.yml`
- `python3 scripts/repo.py status --change vmware-lab`
- `python3 scripts/repo.py verify --change vmware-lab --full` (includes `control-plane`, `compose-config`, and all registered checks; `infra/lab/**` routes to `infra/**`)

## Risks and mitigations

- **Secret leakage**: `secrets.yml.example` uses placeholder values only; all Ansible credential tasks use `no_log: true`, and `README.md` instructs placing the real `secrets.yml` outside the git checkout (`chmod 600`).
- **Oracle binary / OTN license**: Neither `oracle-database-ee-19c-1.0-1.x86_64.rpm` nor any measured Oracle benchmark numbers are committed; the owner provides the RPM path locally.
- **Kernel version drift on `yum.oracle.com`**: `build-oracle-vm.sh` pins the tested OL8U10 build (`b287`) and matching `kernel-uek-modules-5.15.0-320.202.8.5.el8uek.x86_64.rpm` as defaults, overridable via environment variables.

## Rollback or recovery

Revert the squash commit; no existing application code, Docker Compose configuration, or baseline specification is modified.

## Open questions

- None.

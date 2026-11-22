# Saltation VMware Lab (`x86_64` Native, Docker-Free)

Multi-VM `x86_64` deployment of Saltation on a Windows + WSL2 + VMware Workstation host using [`wsl-vmware-lab`](https://github.com/naelonambul/wsl-vmware-lab) (Terraform + Ansible), running every component natively without Docker.

## Topology (5 VMs, 14 vCPU / 20 GB RAM)

| VM | Image (`x86_64`) | vCPU / RAM | Ansible Groups | Services |
|---|---|---:|---|---|
| `saltation-oracle` | `oraclelinux-8` (official OL8.10 KVM + `mods.iso`) | 4 vCPU / 8 GB | `db`, `oracle` | Oracle Database 19c EE (`:1521/ORCLPDB1`), `SALTATION_DATA`, 4 GB-capped `OUTBOX_TS` |
| `saltation-mongo` | `ubuntu-24.04` | 2 vCPU / 4 GB | `db`, `mongo` | MongoDB 8.0 (`:27017`, WiredTiger 1.5 GB, `bank_logs` + `truth` user isolation) |
| `saltation-bank` | `ubuntu-24.04` | 4 vCPU / 4 GB | `app`, `bank` | Rust 1.99.1 `bank` (`systemd`: `:8080`, `:8081`, `:9100`) |
| `saltation-web` | `ubuntu-24.04` | 2 vCPU / 2 GB | `web` | `nginx` Mobile SPA static server (`:8088`, pointing at `saltation-bank:8080`) |
| `saltation-gen` | `ubuntu-24.04` | 2 vCPU / 2 GB | `gen`, `generator` | Python 3.13 (`uv 0.8.17`) `/opt/venv/bin/saltation-gen` |

Every VM attaches three NICs:
- `VMnet8` (NAT): outbound internet access for package installation.
- `VMnet1` (Host-Only): WSL management and Ansible SSH.
- `VMnet0` (Bridged, `lan = true`): LAN addresses (`lan0`, e.g. `192.168.0.x`) for inter-VM traffic and browser access from other hosts on the same LAN.

> **VMware Bridge Note:** Ensure `VMnet0` is bridged to the active physical Ethernet adapter (not virtual adapters such as Hyper-V `vEthernet` or VPN adapters) in VMware Virtual Network Editor or via `wsl-vmware-lab`'s `windows\setup.cmd Install`.

## Prerequisites

1. A Windows + WSL2 + VMware Workstation host with [`wsl-vmware-lab`](https://github.com/naelonambul/wsl-vmware-lab) updated to include `oraclelinux-8` support (PR #61+).
2. The owner-downloaded Oracle Database 19c EE x86-64 RPM (`oracle-database-ee-19c-1.0-1.x86_64.rpm`) obtained under the Oracle Technology Network (OTN) developer license. Never commit this file.
3. A local, untracked secrets file outside any git checkout (`chmod 600`):
   ```sh
   mkdir -p ~/.config/saltation-lab
   cp infra/lab/secrets.yml.example ~/.config/saltation-lab/secrets.yml
   chmod 600 ~/.config/saltation-lab/secrets.yml
   # Edit ~/.config/saltation-lab/secrets.yml and replace every placeholder.
   ```

## Step 1: Provision All 5 VMs via Terraform (`wsl-vmware-lab`)

From your `wsl-vmware-lab` checkout in WSL:

```sh
cp /path/to/saltation/infra/lab/vms.tfvars.example ./vms.tfvars
./lab network
bash lib/preflight.sh
./lab setup          # prepares both ubuntu-24.04 and oraclelinux-8 (+ mods.iso) bases
./lab up --dry-run
./lab up
./lab up --all
./lab status
ANSIBLE_TIMEOUT=60 ./lab configure ansible/check-lab.yml
```

> **Note on `saltation-oracle` disk space:** The default `oraclelinux-8` image ships a ~32 GB root logical volume (enough for Oracle 19c EE + smoke tests). For a full 365-day backfill, expand the VM disk to 45+ GB before running backfill.

## Step 2: Deploy MongoDB, Bank (Rust), Web (nginx), and Generator

```sh
ANSIBLE_TIMEOUT=60 ./lab configure /path/to/saltation/infra/lab/ansible/site.yml \
  -e "saltation_secrets_file=$HOME/.config/saltation-lab/secrets.yml"
```

## Step 3: Transfer Oracle 19c RPM & Install Natively on `saltation-oracle`

Copy the owner-downloaded RPM to `saltation-oracle:/tmp/` (using the `lan0` IP reported by `check-lab.yml`), then run `oracle.yml`:

```sh
ORACLE_LAN_IP=<saltation-oracle-lan0-ip>
scp -i ~/.config/wsl-vmware-lab/lab_ed25519 /path/to/oracle-database-ee-19c-1.0-1.x86_64.rpm "lab@${ORACLE_LAN_IP}:/tmp/"

ANSIBLE_TIMEOUT=60 ./lab configure /path/to/saltation/infra/lab/ansible/oracle.yml \
  -e "saltation_secrets_file=$HOME/.config/saltation-lab/secrets.yml"
```

## Step 4: Wire `saltation-bank` (`systemd`) & Run 5-VM E2E Smoke Test

```sh
ANSIBLE_TIMEOUT=60 ./lab configure /path/to/saltation/infra/lab/ansible/wire-and-smoke.yml \
  -e "saltation_secrets_file=$HOME/.config/saltation-lab/secrets.yml"
```

`wire-and-smoke.yml` discovers each VM's `lan0` IP from Ansible facts (override with `-e oracle_lan_ip=... -e mongo_lan_ip=... -e bank_lan_ip=... -e web_lan_ip=...` if needed), configures `SALTATION_CORS_ORIGINS` for `saltation-web`, starts `saltation-bank.service` (running all Oracle SQL migrations automatically), verifies `:8080/healthz`, `:9100/metrics`, and `saltation-web:8088/healthz`, seeds 20 customers and 5 counterparties from `saltation-gen`, switches the simulated clock to `live`, drives live transfers, and verifies delivered `bank_logs.events` and `truth.labels` counts on `saltation-mongo`.

## Native x86 CPU calibration

Start from the WSL control host and the existing lab directory. Run `./lab network` and `./lab status` and inspect both power and bridge state. Terraform controls VMware Workstation on Windows through vmrest. Ansible connects to each guest using its pinned host key. Check for separately registered dependencies such as an existing Oracle guest before planning additions. A load-only run needs bank, Oracle, MongoDB and a private driver, with the other guests kept outside the comparison unless they are explicitly part of the workload.

Record guest vCPU and memory from vmrest, guest `cpu.max` and `cpu.stat` for the workload's cgroup, service and application health and the date/revision or binary digest. Record WSL memory and CPU availability separately from Windows physical memory. Measure Windows available memory and page-in/page-out rates before and during the run, then account for each powered VMware guest and the WSL driver. Move the driver to a separate host when its scheduling delays interfere with measurements.

Use the transfer harness in [loadtest](../../loadtest). Establish a baseline with secret-free environment metadata, then vary one CPU allocation or runtime service quota. Keep the VM and driver placement fixed and repeat each candidate with the same offered load. Reject under-delivery and compare server measurements, application errors, N-5 evidence, and host/guest health. Keep raw results in ignored `.evidence/`. Use these runs for x86 sizing and record OCI acceptance separately.

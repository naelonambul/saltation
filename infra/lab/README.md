# Saltation VMware Lab (`x86_64` Native, Docker-Free)

Multi-VM `x86_64` deployment of Saltation on a Windows host running VMware Workstation Pro using [`win-vmware-lab`](https://github.com/naelonambul/win-vmware-lab) (Windows PowerShell 5.1 + Terraform `elsudano/vmworkstation` via `vmrest` + dedicated Linux Ansible controller VM `lab-controller` on Bridged LAN `VMnet0`), running every component natively without Docker.

## Topology (5 VMs, 14 vCPU / 20 GB RAM)

| VM | Image (`x86_64`) | vCPU / RAM / Disk | Ansible Groups | Services |
|---|---|---:|---|---|
| `saltation-oracle` | `oraclelinux-8` | 4 vCPU / 8 GB / 45 GB | `db`, `oracle` | Oracle Database 19c EE (`:1521/ORCLPDB1`), `SALTATION_DATA`, 4 GB-capped `OUTBOX_TS` |
| `saltation-mongo` | `ubuntu-24.04` | 2 vCPU / 4 GB / 30 GB | `db`, `mongo` | MongoDB 8.0 (`:27017`, WiredTiger 1.5 GB, `bank_logs` + `truth` user isolation) |
| `saltation-bank` | `ubuntu-24.04` | 4 vCPU / 4 GB / 30 GB | `app`, `bank` | Rust 1.99.1 `bank` (`systemd`: `:8080`, `:8081`, `:9100`) |
| `saltation-web` | `ubuntu-24.04` | 2 vCPU / 2 GB / 30 GB | `web` | `nginx` Mobile SPA static server (`:8088`, pointing at `saltation-bank:8080`) |
| `saltation-gen` | `ubuntu-24.04` | 2 vCPU / 2 GB / 30 GB | `gen`, `generator` | Python 3.13 (`uv 0.8.17`) `/opt/venv/bin/saltation-gen` |

Every VM attaches a Bridged NIC on `VMnet0` with a non-overlapping static LAN IP (`ip`, `gateway`, `dns` in `vms.tfvars.json`). The Ansible playbooks automatically fall back to `ansible_host` when no separate `lan0` NIC is present.

> **VMware Bridge Note:** Always run `.\lab.cmd network` before `setup` or `up` and confirm `vmnet0_bridge.resolved` is `true` on the active physical NIC. Never reset VMware networks.

## Prerequisites

1. A Windows `x86_64` host prepared with [`win-vmware-lab`](https://github.com/naelonambul/win-vmware-lab):
   - Preflight passes: `.\lab.cmd network`, `.\windows\setup.cmd Status -Json`, and `.\lab.cmd status` (exit `0`).
   - Base VMs prepared for both `ubuntu-24.04` and `oraclelinux-8`, plus `lab-controller` running (`.\lab.cmd setup --image ubuntu-24.04` and `.\lab.cmd setup --image oraclelinux-8`).
2. The owner-downloaded Oracle Database 19c EE x86-64 RPM (`oracle-database-ee-19c-1.0-1.x86_64.rpm`) obtained under the Oracle Technology Network (OTN) developer license. Never commit this file.
3. A local, untracked secrets file inside **`lab-controller`** (`~/.config/saltation-lab/secrets.yml`, `chmod 600`), matching the keys in [secrets.yml.example](secrets.yml.example). Because `.\lab.cmd configure` runs `ansible-playbook` inside `lab-controller` as user `lab`, `lookup('env', 'HOME')/.config/saltation-lab/secrets.yml` resolves on `lab-controller`.

> **Template note:** `win-vmware-lab` uses JSON (`vms.tfvars.example.json`). The older HCL `vms.tfvars.example` is retained for reference only.

## Step 1: Provision and Boot All 5 VMs (`win-vmware-lab`)

From your `win-vmware-lab` checkout in Windows PowerShell 5.1:

```powershell
Copy-Item <path-to-saltation>\infra\lab\vms.tfvars.example.json .\vms.tfvars.json
# Edit .\vms.tfvars.json and replace the RFC 5737 placeholder IPs (192.0.2.x,
# preserving each guest's /24 CIDR prefix), gateway (192.0.2.1), and DNS
# resolver (192.0.2.53) with unused static addresses and settings for your
# Bridged VMnet0 LAN.

.\lab.cmd network
.\windows\setup.cmd Status -Json
.\lab.cmd status

.\lab.cmd up --dry-run
.\lab.cmd up                                                # create clones OFF
.\lab.cmd up saltation-oracle saltation-mongo               # boot DB tier first
.\lab.cmd up saltation-bank saltation-web saltation-gen     # boot app/web/gen tier
.\lab.cmd check --peer <ora-ip>:22,<mongo-ip>:22,<bank-ip>:22,<web-ip>:22,<gen-ip>:22 --wait 180
.\lab.cmd check
```

> **Note on `saltation-oracle` disk space:** `vms.tfvars.example.json` allocates `45 GB` (`disk_gb: 45`) to `saltation-oracle` for Oracle 19c EE + smoke and backfill headroom.

## Step 2: Deploy MongoDB, Bank (Rust), Web (nginx), and Generator

```powershell
.\lab.cmd configure <path-to-saltation>\infra\lab\ansible\site.yml
```

## Step 3: Transfer Oracle 19c RPM and Install Natively on `saltation-oracle`

Copy the owner-downloaded RPM to `saltation-oracle:/tmp/` using `win-vmware-lab`'s pinned Ed25519 key and `known_hosts` (never disable host key checking), then run `oracle.yml`:

```powershell
# Default private_dir; substitute the `private_dir` path configured in lab.json if customized.
$pdir = "$env:USERPROFILE\.config\win-vmware-lab"
scp -B -i "$pdir\id_ed25519" `
  -o BatchMode=yes -o StrictHostKeyChecking=yes -o GlobalKnownHostsFile=none `
  -o IdentitiesOnly=yes -o HostKeyAlgorithms=ssh-ed25519 -o HostKeyAlias=saltation-oracle `
  -o "UserKnownHostsFile=$pdir\known_hosts" `
  <path-to>\oracle-database-ee-19c-1.0-1.x86_64.rpm "lab@<oracle-ip>:/tmp/oracle-database-ee-19c-1.0-1.x86_64.rpm"

.\lab.cmd configure <path-to-saltation>\infra\lab\ansible\oracle.yml
```

## Step 4: Wire `saltation-bank` (`systemd`) and Run 5-VM E2E Smoke Test

```powershell
.\lab.cmd configure <path-to-saltation>\infra\lab\ansible\wire-and-smoke.yml
```

`wire-and-smoke.yml` discovers each VM's Bridged LAN IP from Ansible facts (falling back to `ansible_host`, overridable via `-e oracle_lan_ip=... -e mongo_lan_ip=... -e bank_lan_ip=... -e web_lan_ip=...`), configures `SALTATION_CORS_ORIGINS` for `saltation-web`, starts `saltation-bank.service` (applying all Oracle SQL migrations automatically), verifies `:8080/healthz`, `:9100/metrics`, and `saltation-web:8088/healthz`, seeds 20 customers and 5 counterparties from `saltation-gen`, switches the simulated clock to `live`, drives live transfers, and verifies delivered `bank_logs.events` and `truth.labels` counts on `saltation-mongo`.

## Step 5: Graceful Shutdown and Cleanup

Stop application/driver VMs before stateful database VMs. Never use `--hard` on database nodes without explicit confirmation, and always preview `destroy` with `--dry-run`:

```powershell
.\lab.cmd down saltation-gen saltation-web saltation-bank
.\lab.cmd down saltation-mongo saltation-oracle
# To remove workload VMs later (preserves bases and lab-controller):
# .\lab.cmd destroy --confirm=saltation-bank,saltation-gen,saltation-mongo,saltation-oracle,saltation-web --dry-run
```

## Native x86 CPU and Stack Calibration

Start from the Windows `win-vmware-lab` host directory. Run `.\lab.cmd network`, `.\windows\setup.cmd Status -Json`, and `.\lab.cmd status` to inspect power, bridge state, and `vmrest`. Terraform manages VMware Workstation through loopback `vmrest`, and `lab-controller` connects to each guest with pinned Ed25519 host keys. Keep unrelated VMs powered off during calibration runs.

Record guest vCPU and memory from `vms.tfvars.json`/`vmrest`, guest `cpu.max` and `cpu.stat` for workload cgroups, service health, and the Git revision or binary digest. Measure Windows available memory and paging before and during the run, accounting for each powered VMware guest and `lab-controller`.

Use the transfer harness in [loadtest](../../loadtest) and matched synthetic workloads when calibrating x86 allocations or comparing downstream Rust-native (`Iggy`, `RustFS`, `Lakekeeper`, `Sail`) vs. classic (`Kafka`, `Hadoop/HDFS`, `Spark`) CPU and memory usage. Establish a baseline with secret-free environment metadata, then vary one CPU or memory allocation at a time. Keep VM and driver placement fixed, repeat each candidate under the same offered load, reject under-delivery, and compare application errors, N-5 ledger correctness evidence, and host/guest health. Keep raw captures in ignored `.evidence/`, exclude Oracle Database measured performance figures from tracked files and PRs per `REVIEW.md`, and record OCI acceptance separately.

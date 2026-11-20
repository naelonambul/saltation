# Saltation VMware Lab (`x86_64` Native, Docker-Free)

Multi-VM `x86_64` deployment of Saltation on a Windows + WSL2 + VMware Workstation host using [`wsl-vmware-lab`](https://github.com/naelonambul/wsl-vmware-lab) (Terraform + Ansible), running every component natively without Docker.

## Topology (4 VMs, 12 vCPU / 18 GB RAM)

| VM | OS (`x86_64`) | vCPU / RAM / Disk | Ansible Groups | Services |
|---|---|---|---|---|
| `saltation-oracle` | Oracle Linux 8.10 (official KVM + `mods.iso`) | 4 vCPU / 8 GB / 45 GB | `oracle` | Oracle Database 19c EE (`:1521/ORCLPDB1`), `SALTATION_DATA`, 4 GB-capped `OUTBOX_TS` |
| `saltation-mongo` | Ubuntu 24.04 | 2 vCPU / 4 GB | `db`, `mongo` | MongoDB 8.0 (`:27017`, WiredTiger 1.5 GB, `bank_logs` + `truth` user isolation) |
| `saltation-bank` | Ubuntu 24.04 | 4 vCPU / 4 GB | `app`, `bank`, `web` | Rust 1.99.1 `bank` (`systemd`: `:8080`, `:8081`, `:9100`) + `nginx` Mobile SPA (`:8088`) |
| `saltation-gen` | Ubuntu 24.04 | 2 vCPU / 2 GB | `gen`, `generator` | Python 3.13 (`uv 0.8.17`) `/opt/venv/bin/saltation-gen` |

Every VM attaches three NICs:
- `VMnet8` (NAT): outbound internet access for package installation.
- `VMnet1` (Host-Only): WSL management and Ansible SSH.
- `VMnet0` (Bridged, `lan = true`): LAN addresses (`192.168.0.x`) for inter-VM traffic and browser access from other hosts on the same LAN.

> **VMware Bridge Note:** Ensure `VMnet0` is bridged to the active physical Ethernet adapter (not virtual adapters such as Hyper-V `vEthernet` or VPN adapters) in VMware Virtual Network Editor or via `wsl-vmware-lab`'s `windows\setup.cmd Install`.

## Prerequisites

1. A Windows + WSL2 + VMware Workstation host configured with `wsl-vmware-lab` (`./lab network`, `bash lib/preflight.sh`, `./lab setup`, `./lab status` all passing).
2. The owner-downloaded Oracle Database 19c EE x86-64 RPM (`oracle-database-ee-19c-1.0-1.x86_64.rpm`) obtained under the Oracle Technology Network (OTN) developer license. Never commit this file.
3. A local, untracked secrets file outside any git checkout (`chmod 600`):
   ```sh
   mkdir -p ~/.config/saltation-lab
   cp infra/lab/secrets.yml.example ~/.config/saltation-lab/secrets.yml
   chmod 600 ~/.config/saltation-lab/secrets.yml
   # Edit ~/.config/saltation-lab/secrets.yml and replace every placeholder.
   ```

## Step 1: Provision the 3 Ubuntu 24.04 VMs (`mongo`, `bank`, `gen`)

From your `wsl-vmware-lab` checkout in WSL:

```sh
cp /path/to/saltation/infra/lab/vms.tfvars.example ./vms.tfvars
./lab up --dry-run
./lab up
./lab up --all
./lab status
ANSIBLE_TIMEOUT=60 ./lab configure ansible/check-lab.yml
```

Then deploy MongoDB 8.0, build `bank` (Rust x86_64 native) + Mobile SPA (`nginx`), and install `saltation-gen` (Python 3.13):

```sh
ANSIBLE_TIMEOUT=60 ./lab configure /path/to/saltation/infra/lab/ansible/site.yml \
  -e "saltation_secrets_file=$HOME/.config/saltation-lab/secrets.yml"
```

## Step 2: Boot `saltation-oracle` (Official OL8.10 KVM + `mods.iso`)

Oracle's official `OL8U10_x86_64-kvm` cloud image includes only `kernel-uek-core` and omits VMware NIC modules (`vmxnet3`, `e1000`, `e1000e`). `infra/lab/ol8/build-oracle-vm.sh` keeps the official image unmodified and attaches two CD-ROMs:
- `ide1:0` (`seed.iso`, label `cidata`): cloud-init `user-data` and `meta-data`.
- `ide1:1` (`mods.iso`, label `OL8MODS`): built once from the matching official `kernel-uek-modules-<kver>.rpm` from `yum.oracle.com`.

On first boot, cloud-init installs `kernel-uek-modules`, runs `depmod -a && modprobe vmxnet3`, and brings up all three NICs via DHCP. After first boot, `mods.iso` can be detached and `vmxnet3` persists across reboots.

```sh
bash /path/to/saltation/infra/lab/ol8/build-oracle-vm.sh
```

Wait ~30 seconds, inspect the VM's LAN IP from `serial.log`, then install Oracle 19c EE natively:

```sh
ORACLE_LAN_IP=<discovered-lan-ip>
scp -i ~/.config/wsl-vmware-lab/lab_ed25519 /path/to/oracle-database-ee-19c-1.0-1.x86_64.rpm "lab@${ORACLE_LAN_IP}:/tmp/"

ANSIBLE_TIMEOUT=60 ansible-playbook \
  -i "${ORACLE_LAN_IP}," \
  -u lab \
  --private-key ~/.config/wsl-vmware-lab/lab_ed25519 \
  -e "oracle_lan_ip=${ORACLE_LAN_IP}" \
  -e "saltation_secrets_file=$HOME/.config/saltation-lab/secrets.yml" \
  /path/to/saltation/infra/lab/ansible/oracle.yml
```

## Step 3: Wire `saltation-bank` (`systemd`) & Run 4-VM E2E Smoke Test

```sh
ANSIBLE_TIMEOUT=60 ./lab configure /path/to/saltation/infra/lab/ansible/wire-and-smoke.yml \
  -e "saltation_secrets_file=$HOME/.config/saltation-lab/secrets.yml" \
  -e "oracle_lan_ip=${ORACLE_LAN_IP}" \
  -e "mongo_lan_ip=<saltation-mongo-lan-ip>" \
  -e "bank_lan_ip=<saltation-bank-lan-ip>"
```

This starts `saltation-bank.service` (which runs all Oracle SQL migrations automatically), verifies `:8080/healthz` and `:9100/metrics`, seeds 20 customers and 5 counterparties from `saltation-gen`, switches the simulated clock to `live`, runs live transfers, and asserts that both `bank_logs.events` and `truth.labels` receive delivered records on `saltation-mongo`.

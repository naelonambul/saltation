# Define OCI ARM and x86 CPU budgets

## Summary

Implement the approved [N-7 specification](spec.md#non-functional-requirements). Make the OCI Ampere A1 Compose quotas auditable and keep their concurrent sum within 4 vCPU and 14 GB. Calibrate x86 CPU allocations on the native Saltation VMware guests. Use `ssh playdata` to reach Ubuntu WSL2, with VMware Workstation on the Windows host and vmrest as the control API. WSL is the orchestration environment and an optional load-driver placement. OCI is unavailable during this work. Validate the budget locally and retain primary-host deployment and N-8 as follow-up.

## Evidence and resource accounting

- Current Compose totals are 4.25 vCPU for the server stack, 4.75 with the generator, 5.25 with the load tool and 5.75 with both. Add any separately managed Saltation connector. The two-tools selection declares 14.125 GiB before an external connector.
- The owner confirmed the OCI host is 4 vCPU and 24 GB. On Ampere A1 one OCPU equals one vCPU. On OCI x86 Compute the documented conversion is two vCPUs per OCPU. The local x86 lab uses guest logical CPU and host contention evidence. The OCI cap consumes the whole nominal CPU capacity when all quotas are used. Count co-tenant N-8 results independently. [Oracle Compute Shapes](https://docs.oracle.com/en-us/iaas/Content/Compute/References/computeshapes.htm)
- The owner identified the `playdata` Windows host as HP ProBook 450 G10 821J8PA with i5-1340P and 32 GB RAM. On 2026-11-22, SSH reported an x86-64 WSL2 kernel, four online logical CPUs, 4,012,600 KiB RAM, 1 GiB swap and Python 3.12.3. `vmrest` on WSL localhost returned HTTP 401 to an unauthenticated GET of `/api/vms`, confirming reachability. The authenticated `./lab status` in `/mnt/c/wsl-vmware-lab` then returned HTTP 200 from vmrest. It reported `saltation-bank`, `saltation-gen` and `saltation-mongo` powered off. This checkout did not report Oracle or web guests. Its Windows preflight also reported no IPv4 default route and a bridge-selection warning. Verify the actual private route before load testing. The WSL totals describe the management environment. Account for VMware guests and host memory separately.
- The repository example allocates 4/2/4/2/2 vCPU and 8/4/4/2/2 GB to the five VMware guests. Measure actual Windows-usable RAM, Windows/background usage, VMware guest configured/observed memory and overhead, WSL actual demand and cap, and any other concurrent workload. Use a pre-run Windows baseline and incremental demand for stopped components so already-running allocations are counted once. Verify peak available memory and paging under load before adopting a budget.

## Files and components that change

| Paths | Work |
| --- | --- |
| `infra/compose/compose.yml` | Reallocate container quotas within N-7 and keep on-demand workloads usable. |
| `scripts/ops/*compose_budget*.py`, `checks.json` | Static phase check and runtime budget audit, registered in the existing Oracle check group. |
| `README.md`, `docs/dev-host.md`, `infra/lab/README.md` | OCI and x86 resource accounting, supported launch phases, evidence commands and x86 calibration procedure. |
| `spec.md` | Adopt the approved packet specification byte for byte at integration. |

## Order of work

1. **Check the declared budget.** Reproduce the baseline totals with `docker compose -f infra/compose/compose.yml --env-file infra/compose/.env.example --profile local --profile tools config --format json`. Add `scripts/ops/check_compose_budget.py` and `scripts/ops/test_compose_budget.py`. Check each supported active set (server, generator phase, load phase), replicas, one-offs and the Saltation edge connector when present. Compare decimal CPU quotas and memory in bytes, enforcing 14,000,000,000 bytes as the conservative 14 GB ceiling. Reject missing limits, duplicate active jobs and over-budget or unidentified members. Test exact-boundary and missing/over-limit inputs. Proof: `python3 -m unittest scripts/ops/test_compose_budget.py -v`.
2. **Set and audit quotas.** Tune `infra/compose/compose.yml` to fit the approved cap while retaining database memory requirements. Preserve Compose run entry points for development. Document guarded shared-host workflows in `README.md` and `docs/dev-host.md`, including checks before launch, detecting a concurrently running one-off and verifying actual limits through Docker inspection and cgroup quota/cpuset data. Include externally managed Saltation runtime containers in the inventory and redact secrets from reports. Wire the static check into `checks.json` under the Oracle group. Prove the runtime checker and startup against an isolated ARM64 Compose deployment. Proof: `python3 scripts/repo.py verify --change stage8-cpu-budget --group oracle` and the documented runtime audit for each supported phase.
3. **Inventory the x86 lab through its existing control path.** Document in `infra/lab/README.md` the path from `ssh playdata` to the existing `wsl-vmware-lab` checkout, `./lab network`, and `./lab status` (VM inventory and power via vmrest, checked together with per-VM warnings). The VMware provider runs in WSL and talks to Windows vmrest. Ansible uses SSH to manage guests. Verify the VM hostname, guest resource limits and connection to the bank/metrics listeners before starting tests. Test the Windows, WSL and VMware memory headroom described above. Carry out quota/allocation changes through the lab workflow with the relevant guest shut down as needed. Proof: `./lab status` in the lab checkout, guest CPU/memory and effective-quota readouts, and Windows host baseline/peak measurements.
4. **Calibrate x86 on that inventory.** Add the comparison procedure to `infra/lab/README.md`. Begin at the measured baseline, vary one CPU allocation at a time, repeat comparable N-1/N-2/N-3 shaped workloads at the best candidates, and record N-5 and local host health. Capture offered load/cadence, driver lag, server metrics, quotas, throttling, Windows paging, and guest health. Use the load verdict packet's checks for independent evidence. Keep driver placement fixed for comparisons. If running the driver from WSL, supply its Python 3.13 runtime and prove it can sustain offered load without becoming the bottleneck. Allow an isolated driver if WSL contention obscures results. Store Oracle measurements privately under ignored `.evidence/`. Proof: the lab run manifest, repeated load reports and a measured x86 budget recommendation, then `python3 scripts/repo.py verify --change stage8-cpu-budget --full`.

## Risks and mitigations

The configured 4 vCPU on OCI leaves no guaranteed CPU reserve. Runtime health and restart evidence are required when OCI becomes available. On x86, WSL and VMware compete for Windows physical resources, and a VMware guest or WSL cgroup quota alone cannot show cross-host contention. Confirm paging and available memory throughout the comparison. Record driver saturation as invalid evidence and rerun with a capable driver. Keep the tested x86 budget separate from OCI primary-host acceptance.

## Rollback or recovery

Retain the last known healthy VM configuration and restore it through the lab's normal graceful shutdown and quota-change procedure if an x86 candidate fails. Stop an invalid test and preserve its run evidence. Reverting a Compose budget below compliance restarts the budget work before it can be used for acceptance.

## Open questions

Locate the Oracle dependency and any separately managed web guest. Confirm the full target topology and usable private route before starting the x86 load. Measure Windows/WSL/VMware memory under the intended concurrent workload. Verify the OCI connector and co-tenant inventory later, when the host becomes available.

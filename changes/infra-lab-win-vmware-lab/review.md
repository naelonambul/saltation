# Review: `infra-lab-win-vmware-lab`

- **Reviewed commit:** `6b4047cdc7522fc9e288a3057efc7f4eca5aec6c`
- **Independent reviewer:** Codex `gpt-6.2-astra` (`medium`, read-only review via `codex-companion.mjs`)
- **Verification evidence:**
  - `python3 -m json.tool infra/lab/vms.tfvars.example.json >/dev/null` (exit 0)
  - `python3 scripts/repo.py status --change infra-lab-win-vmware-lab` (`readiness=ready`, `status: ok`)
  - `python3 scripts/repo.py verify --change infra-lab-win-vmware-lab --group core` (`control-plane` passed, exit 0, 63.8s; evidence `.evidence/20261128T084400000273Z/evidence.json`). Note: `oracle` group checks routed by `infra/**` (`compose-config`, `oracle-integration`) require Docker and are executed on the OCI CI runner.

## Review passes & Astra findings disposition

Independent review by Codex `gpt-6.2-astra` (`medium`) identified 3 Important findings and 1 Nit on the initial draft; all 4 were resolved before committing `6b4047c`:

1. **[Important — Resolved]** `plan.md` referenced the real Windows lab hostname on lines 5 and 31. Replaced both occurrences with `the verified Windows lab host` and obtained fresh owner approval (`sha256:4c5f328d8dd27884f961800027a78a515c6490a7b328359b7ae5c447faf69d33`).
2. **[Important — Resolved]** `infra/lab/README.md` native x86 calibration section omitted the `spec.md` N-7 criteria (vary one allocation at a time, keep VM/driver placement fixed, repeat under the same offered load, reject under-delivery, and compare application errors and N-5 ledger correctness). Restored all criteria alongside the Rust-native vs. classic stack comparison.
3. **[Important — Resolved]** `infra/lab/vms.tfvars.example.json` used public resolvers (`1.1.1.1`, `8.8.8.8`) instead of placeholder DNS addresses. Replaced with RFC 5737 documentation address `192.0.2.53` and updated `infra/lab/README.md` to instruct substituting IPs (preserving `/24` CIDR prefix), gateway, and DNS resolver.
4. **[Nit — Resolved]** `infra/lab/README.md` RPM copy snippet assumed the default `private_dir`. Added a comment labeling `$pdir` as the default and instructing operators to substitute `lab.json`'s `private_dir` if customized.

## Security, privacy, and OTN check

- No real hostnames, LAN IPs, credentials, or secrets committed.
- Zero Oracle Database measured performance figures included (OTN compliant).
- All changes remain strictly within approved `write_scope` (`infra/lab/README.md`, `infra/lab/vms.tfvars.example.json`, and packet files).

## Conclusion

All Important and Nit findings from Codex `gpt-6.2-astra` are resolved in `6b4047c`. Ready for final PR head verification and owner review/merge.

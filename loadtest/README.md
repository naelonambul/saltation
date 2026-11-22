# Saltation transfer load tests

Run from an isolated Saltation deployment. The tool creates synthetic customers and moves synthetic funds. Point it at the private generator listener and the bank's metrics listener. Keep all reports under ignored `.evidence/loadtest/`; Oracle performance measurements are private under [REVIEW.md](../REVIEW.md).

## Exploratory runs

Smoke runs, custom rates and single-scenario results are exploratory. A complete x86 run uses the prescribed rates and durations without `--qualify` to collect sizing evidence. Keep the bank, Oracle and the driver placement fixed between trials and collect Windows/VM health separately on a native VMware lab.

```sh
uv run --project loadtest --frozen saltation-loadtest \
  --bank-url http://127.0.0.1:8081 --metrics-url http://127.0.0.1:9100 \
  --report-dir .evidence/loadtest --smoke all
```

Remove `--smoke` for full-length windows. Use `--tps` for N-1/N-2, `--steps` for N-3 and `--duration` to shorten individual experimental windows. An exploratory exit 0 means a report was written. Its summary has `outcome: EXPLORATORY` and a null `passed` field. Examine dispatch, status and metric coverage before comparing runs.

## Primary-host qualification

Use a Linux operator process on the designated development host. Provide its local Docker Unix socket via `SALTATION_VERIFY_DOCKER_SOCKET` (default `/var/run/docker.sock`) and the target Compose project via `SALTATION_VERIFY_PROJECT`. Publish the generator, metrics and Oracle ports on IPv4 loopback in the qualification deployment. The endpoints supplied to the qualifier must match the Docker port bindings and the bank-to-Oracle data network. The required co-tenant containers and all Saltation server containers must be present before measurement. Qualify only the bank and Oracle belonging to that inspected deployment. Keep other traffic off the generator transfer endpoint during the windows.

Give the operator process a read-only Oracle account and set `SALTATION_DB_USER`, `SALTATION_DB_PASSWORD` and `SALTATION_DB_CONNECT` in its environment. Set `SALTATION_VERIFY_ORACLE_SCHEMA` if the bank schema differs from `saltation`. The account must be able to read the clock, account, ledger, daily-close and request tables referenced in `verify_oracle.py`. Start the load and evidence process together. The Docker socket and Oracle credentials stay on the host.

```sh
uv run --project loadtest --frozen saltation-loadtest \
  --bank-url http://127.0.0.1:8081 --metrics-url http://127.0.0.1:9100 \
  --report-dir .evidence/loadtest --revision "$(git rev-parse HEAD)" --qualify all
```

Apply the revision label to the deployed bank image at build time, for example
`docker build --target runtime --label "org.opencontainers.image.revision=$(git rev-parse HEAD)" -t saltation/bank:local bank`.
Use the revision of the deployed stack. Set `SALTATION_VERIFY_OCI_SHAPE` to record the operator-reported shape. The qualifier fixes the workload at N-1 50 TPS for 600 s, N-2 50 TPS for 300 s with at least 200 active senders to one receiver, and N-3 steps of 25, 50, 100 and 200 TPS for 180 s each. It requires server metric coverage and checks Oracle after each scenario. An open business date yields an incomplete close proof. Use `--wait-close SECONDS` to let the run wait for normal rollover after measuring all scenarios. This option leaves the business clock untouched.

- Exit 0, `PASS` means the full workload, performance, ledger, budget and health checks passed.
- Exit 1, `FAIL` records a demonstrated violation.
- Exit 2, `INCOMPLETE` means required evidence is missing or the run could not complete.

Each run writes a run-ID summary and scenario reports. The JSON contains scheduled sequence numbers and dispatch offsets, request outcomes, server counters and scrape snapshots, Oracle reconciliation results, and Docker quota and lifecycle evidence as available. Client latency helps diagnose the run. The acceptance decisions use server latency.

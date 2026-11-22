# Bound bank glibc allocator arenas and Argon2 semaphore lifetime to prevent OOM

## Summary

During a Stage 8 long-running generator execution on the primary OCI ARM64 host (4 vCPU, `saltation-bank-1` capped at 1 GiB RAM, `oraclelinux:9-slim` glibc), `saltation-bank-1` was OOM-killed (`OOMKilled=true`, `ExitCode=137`) after ~2h4m (~305,584 requests). A controlled A/B reproduction against the same `bank` image and live DB with 300 concurrent `200 OK` logins (`max_workers=20`) showed `RssAnon` jumping from `10.2 MiB` to `694.1 MiB` (remaining flat at `694.1 MiB`, `704.5 MiB / 1 GiB`) under default glibc settings, while bounding glibc arenas and fixing `mmap_threshold` kept `RssAnon` at `5.6 MiB -> 8.5 MiB` (`9.85 MiB / 1 GiB`).

This incident change applies the unified `GLIBC_TUNABLES` setting to both Docker (`bank/Dockerfile`) and the native x86-64 VMware lab (`infra/lab/ansible/wire-and-smoke.yml`), and fixes a semaphore permit lifetime bug in `bank/src/shared/auth/mod.rs` where cancelling a caller future released the Argon2 gate permit before the spawned blocking task finished.

## Files and components that change

| Paths | Work |
| --- | --- |
| `bank/Dockerfile` | Set `ENV GLIBC_TUNABLES=glibc.malloc.arena_max=2:glibc.malloc.mmap_threshold=131072` in the `runtime` stage so Compose inherits it. |
| `infra/lab/ansible/wire-and-smoke.yml` | Add `GLIBC_TUNABLES=glibc.malloc.arena_max=2:glibc.malloc.mmap_threshold=131072` to `/etc/saltation-bank.env` for native systemd runs. |
| `bank/src/shared/auth/mod.rs` | Change `gate()` to `Arc<Semaphore>` and acquire `OwnedSemaphorePermit` (`acquire_owned`) moved into the `spawn_blocking` closure in `hash_secret` and `verify_secret`, plus a unit test proving a cancelled caller does not release the gate before the blocking work completes. |

## Order of work

1. **Recorded reproduction (completed on base).** On OCI ARM64 with the unpatched `bank` image (`1 GiB` limit), 300 concurrent `POST /v2.0/ext/auth/login` (`200 OK`, `max_workers=20`) raised `RssAnon` from `10.2 MiB` to `694.1 MiB` (`704.5 MiB / 1 GiB`, unchanged after 5 s). With `arena_max=2` and `mmap_threshold=131072`, the same 300 logins stayed at `5.6 MiB -> 8.5 MiB` (`9.85 MiB / 1 GiB`).
2. **Apply unified `GLIBC_TUNABLES` to container and native lab environments.** Update `bank/Dockerfile` (`runtime` stage) and `infra/lab/ansible/wire-and-smoke.yml` (`/etc/saltation-bank.env`) to set `GLIBC_TUNABLES=glibc.malloc.arena_max=2:glibc.malloc.mmap_threshold=131072`. Proof: `docker image inspect` on the rebuilt `saltation/bank:local` image confirms the env var, and `grep` confirms `/etc/saltation-bank.env` template includes it.
3. **Bind Argon2 semaphore permit lifetime to blocking work.** Update `bank/src/shared/auth/mod.rs` so `gate()` returns `&'static Arc<Semaphore>` and `hash_secret` / `verify_secret` move an `OwnedSemaphorePermit` into `tokio::task::spawn_blocking`. Add a unit test verifying that cancelling a caller future does not allow more than 2 concurrent Argon2 tasks to execute. Proof: `cargo test --manifest-path bank/Cargo.toml --lib shared::auth` and `scripts/dev/ci-rust.sh` (`fmt`, `clippy`, `test`).
4. **Verify on live OCI container and full suite.** Rebuild `saltation/bank:local` on OCI, run 300 concurrent `200 OK` logins against the rebuilt image, and confirm `RssAnon` stays bounded (`<= +128 MiB` above idle baseline 5 s after completion) with zero OOM kills. Run `python3 scripts/repo.py status --change bank-argon2-oom` and `python3 scripts/repo.py verify --change bank-argon2-oom --full` (via CI if local Docker is stopped).

## Tests and proof

- Unit test in `bank/src/shared/auth/mod.rs`: cancelled future holding an Argon2 permit does not allow concurrent executions to exceed the gate bound (`2`).
- Live OCI A/B verification on rebuilt `saltation/bank:local`: 300 concurrent `200 OK` logins keep `RssAnon` increase `<= +128 MiB` 5 s after completion.
- Registered CI checks (`rust`, `control-plane`, `oracle`, `status`, `identity`, `summary`) all pass.

## Risks and mitigations

- Fixing `mmap_threshold=131072` with `arena_max=2` increases `mmap`/`munmap` syscalls for allocations above 128 KiB and may add minor arena lock contention under bursty allocation. Because Argon2id work itself dominates login CPU cost and is already gated to 2 concurrent operations, the overhead is small and will be observed during the OCI verification run.
- Global allocator replacement (`tikv-jemallocator` / `mimalloc`) and low-level Argon2 buffer reuse are intentionally deferred to keep this incident fix minimal and avoid ARM64 page-size compatibility or PHC-verification risks.

## Rollback or recovery

Revert the three scoped files (`bank/Dockerfile`, `infra/lab/ansible/wire-and-smoke.yml`, `bank/src/shared/auth/mod.rs`) and rebuild the `bank` image if unexpected latency regression appears.

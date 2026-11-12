# Development host

How to build, run and test Saltation. Stage 1 develops on the owner's Mac (Apple Silicon, arm64). The OCI host (arm64) runs the self-hosted CI runner for the Oracle integration checks, and later the stack itself (stage 6). Every toolchain runs in a container, so a host needs only Docker (with the Compose and Buildx plugins), Git, Bash and Python 3.

## Mac prerequisites

```sh
brew install colima docker docker-compose docker-buildx
colima start --cpu 6 --memory 10 --disk 100
```

Docker must find the Homebrew CLI plugins: `~/.docker/config.json` contains `"cliPluginsExtraDirs": ["/opt/homebrew/lib/docker/cli-plugins"]`.

## Oracle image

Oracle Database 19c Enterprise Edition is built locally from Oracle's official scripts and never published. The installation zip is downloaded by the owner after accepting Oracle's license.

```sh
ORACLE_ZIP=/path/to/LINUX.ARM64_1919000_db_home.zip infra/oracle/build.sh   # saltation/oracle-ee:19.19
infra/oracle/smoke.sh                                                        # prints the N-7 settings
```

See `infra/oracle/README.md`. To use the image on the OCI host:

```sh
docker save saltation/oracle-ee:19.19 | ssh oracle docker load
```

## Run the stack locally

```sh
cp infra/compose/.env.example infra/compose/.env    # then replace every value
docker compose -f infra/compose/compose.yml --env-file infra/compose/.env --profile local up -d --build --wait
```

The first Oracle start creates the database and takes 10 to 20 minutes. Then:

| What | Where |
|---|---|
| Mobile web app | http://localhost:8088 |
| Bank public listener | http://localhost:8080 (`/healthz`) |
| Grafana (admin, `GRAFANA_ADMIN_PASSWORD`) | http://localhost:3000, dashboard "Saltation bank" |

The generator listener (8081), Oracle, MongoDB, Loki and Prometheus are not published.

Seed customers and drive normal activity:

```sh
compose="docker compose -f infra/compose/compose.yml --env-file infra/compose/.env"
$compose --profile tools run --rm generator seed --customers 1000
$compose --profile tools run --rm generator run-normal --minutes 10 --rate 5
```

## Checks

Registered in `checks.json`; run them all with `python3 scripts/repo.py verify --change <id> --full`.

| Group | Command | Needs |
|---|---|---|
| `rust` | `scripts/dev/ci-rust.sh fmt\|clippy\|test` | Docker |
| `web` | `scripts/dev/ci-web.sh lint\|typecheck\|test\|build` | Docker |
| `python` | `scripts/dev/ci-py.sh lint\|test` | Docker |
| `oracle` | `scripts/dev/it-oracle.sh all`, Compose config check | Docker and the `saltation/oracle-ee:19.19` image |

`scripts/dev/it-oracle.sh` runs Oracle and MongoDB in a separate Compose project (`saltation-it`). Its credentials and lock live in `$SALTATION_IT_DIR` (default `~/.cache/saltation-it`), outside the checkout, so the test database is created once and reused: a typical run skips database creation. The volumes are recreated when the env file is new, when `--fresh` is given, or when the setup key changes (a hash of `infra/oracle/setup/*`, `infra/mongo/init/*`, `infra/oracle/build.sh` and the Oracle image ID), or when the database volumes are not the ones this state created (for example volumes recreated by an older version of the script). Each run resets the schema and `bank_logs.events`, stops application containers before the database targets, and holds a per-host lock. To recover by hand, run `scripts/dev/it-oracle.sh all --fresh` or delete the state directory. It accepts a single target: `mongo-users`, `migrations`, `listeners`, `auth`, `accounts`, `transfers`, `outbox`, `outbox-outage` (stops MongoDB for 120 s during load; about 5 minutes), `observability` or `all`. `--fresh` recreates its volumes.

## Self-hosted runner on the OCI host (owner)

The `oracle` group needs the locally built Oracle image, which GitHub-hosted runners do not have, so its CI job runs on a self-hosted runner on the OCI host. Registration needs an owner token, so the owner runs these steps.

1. Load the Oracle image on the host (see above), and confirm `docker`, `git`, `bash` and `python3` work for the runner's user, which must be in the `docker` group. Docker access is root-equivalent on that host.
2. In the repository: **Settings → Actions → General → Fork pull request workflows from outside collaborators → Require approval for all outside collaborators**. Code from forks must never run on the host without the owner's approval.
3. In the repository: **Settings → Actions → Runners → New self-hosted runner**, choose **Linux** and **ARM64**, and run the download and `./config.sh` commands shown there on the host, in `~/actions-runner`, adding the labels:

   ```sh
   ./config.sh --url https://github.com/naelonambul/saltation --token <TOKEN> \
     --name oci-saltation --labels saltation-oracle --work _work --unattended
   sudo ./svc.sh install && sudo ./svc.sh start
   ```

4. The job selects it with `runs-on: [self-hosted, linux, ARM64, saltation-oracle]`. Stop the service (`sudo ./svc.sh stop`) to take the runner offline; the `oracle-integration` job then stays queued and the `summary` check does not pass, so nothing merges unverified.
5. Remove it with `./config.sh remove --token <TOKEN>`.

## CI

`.github/workflows/repository.yml` runs one job per check group: `rust`, `web` and `python` on GitHub-hosted runners, and `oracle` on the self-hosted `saltation-oracle` runner. Each job runs `python3 scripts/repo.py verify --full --group <group>`, and `summary` requires all of them. Changes to the workflow file are pushed by the owner (`docs/host-setup.md`, "Workflow changes").

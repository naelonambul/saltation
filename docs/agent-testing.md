# Test Saltation without the full UI

Run from the root of your clone in Bash. This guide covers the bank API and synthetic generator. The initial mobile flow has a separate browser check. Full mobile and analyst UI acceptance is Stage 7. The 365-day backfill and 24-hour live run are Stage 8 acceptance. See the [API contracts](../contracts/openapi/) and [scenario matrix](../spec.md#scenario-matrix).

Use Git, Bash, Python 3.12 and Docker with Compose and Buildx. Oracle checks also need the local `saltation/oracle-ee:19.19` image. See [host setup](dev-host.md) and [Oracle image setup](../infra/oracle/README.md). If the licensed image is unavailable, report Oracle checks as blocked and run the Rust and Python checks if Docker is available.

## Reserve a disposable project

Run this setup before any Oracle command, including `repo.py verify --full`, and keep using the same Bash session. The default integration project `saltation-it` reuses data and can reset schemas or recreate volumes. The setup below reserves a fresh project. The launcher generates its own credentials and allocates loopback ports.

```bash
mkdir -p .evidence || exit 1
evidence=$(mktemp -d .evidence/agent-check.XXXXXXXX) || exit 1
suffix=$(python3 -c 'import uuid; print(uuid.uuid4().hex)') || exit 1
[[ $suffix =~ ^[0-9a-f]{32}$ ]] || exit 1
project="saltation-it-agent-$suffix"
state="$HOME/.cache/$project"
label="com.docker.compose.project=$project"
containers=$(docker ps -aq --filter "label=$label") || exit 1
networks=$(docker network ls -q --filter "label=$label") || exit 1
volumes=$(docker volume ls -q) || exit 1
[[ -z $containers && -z $networks ]] || exit 1
while IFS= read -r volume; do
  [[ $volume != "${project}_"* ]] || exit 1
done <<< "$volumes"
mkdir -p "$HOME/.cache" && mkdir -m 700 "$state" || exit 1
git rev-parse HEAD > "$evidence/commit.txt" || exit 1
printf '%s\n' "$project" > "$evidence/project.txt" || exit 1
export SALTATION_IT_PROJECT="$project"
unset SALTATION_IT_DIR
```

## Choose the check

| Goal | Command | Evidence |
| --- | --- | --- |
| Rust or generator tests without Oracle | `scripts/dev/ci-rust.sh test` or `scripts/dev/ci-py.sh test` | The selected test runner exits successfully. |
| Transfer contracts or recorded outcomes | `scripts/dev/it-oracle.sh migrations`, then invoke the same script with `transfers` or `outcomes` | The migration and selected test assertions pass. Supply one target per invocation. |
| Scenario records across Oracle, `bank_logs`, and `truth` | `scripts/dev/it-oracle.sh generator-scenarios` | A bounded A–D fixture drives the bank over HTTP and the verifier prints `test generator_scenarios_proof ... ok`. |
| Switch to live mode and recover after an interrupted handoff | `scripts/dev/it-oracle.sh generator-lifecycle` | The bounded fixture and `test generator_lifecycle_proof ... ok` both succeed. |
| Every registered check | `python3 scripts/repo.py verify --full` (add `--change <id>` for an approved change) | Inspect `.evidence/<run>/evidence.json` for a complete passing run. This includes the separate browser check. |

## Run the scenario smoke test

The harness starts the bank, sends synthetic requests and verifies stored results. Watch the output until it exits. A first startup can take several minutes. Use a fresh project if synthetic world credentials are already reserved or before repeating a full run.

```bash
(
  docker image inspect saltation/oracle-ee:19.19 --format '{{.Id}}' > "$evidence/image.txt" || exit 1
  cmd=(env CARGO_BUILD_JOBS=1 scripts/dev/it-oracle.sh generator-scenarios)
  printf '%q ' "${cmd[@]}" > "$evidence/command.txt" || exit 1
  set +e
  "${cmd[@]}" 2>&1 | tee "$evidence/scenarios.log"
  result=("${PIPESTATUS[@]}")
  printf '%s\n' "${result[0]}" > "$evidence/exit-code.txt" || exit 1
  (( result[0] == 0 )) || exit "${result[0]}"
  (( result[1] == 0 )) && grep -Fq 'test generator_scenarios_proof ... ok' "$evidence/scenarios.log"
)
```

Report the commit, command, exit code and named assertion. Keep the relevant synthetic run or request IDs for tracing rows and events. Keep local evidence under `.evidence/` and exclude credentials, dumps and measured Oracle benchmark results from shared output.

## Inspect and clean up

Use the same shell and environment file. Run the health check after the harness starts the bank. Health confirms readiness. The verifier establishes behavior.

```bash
[[ $project == "${SALTATION_IT_PROJECT:-}" && $project == "$(cat "$evidence/project.txt")" ]] || exit 1
compose=(docker compose -p "$project" -f infra/compose/compose.yml --env-file "$state/env" --profile ci)
"${compose[@]}" ps
"${compose[@]}" exec -T bank curl -fsS http://localhost:8080/healthz
```

After testing finishes, verify ownership of every volume with this project's name prefix. Stop the stack, then remove just the verified volumes. Keep the local evidence.

```bash
[[ $project == "${SALTATION_IT_PROJECT:-}" && $project == "$(cat "$evidence/project.txt")" ]] || exit 1
volumes=$(docker volume ls -q) || exit 1
owned=()
while IFS= read -r volume; do
  [[ $volume == "${project}_"* ]] || continue
  owner=$(docker volume inspect -f '{{index .Labels "com.docker.compose.project"}}' "$volume") || exit 1
  [[ $owner == "$project" ]] || exit 1
  owned+=("$volume")
done <<< "$volumes"
"${compose[@]}" down || exit 1
for volume in ${owned[@]+"${owned[@]}"}; do
  docker volume rm "$volume" || exit 1
done
```

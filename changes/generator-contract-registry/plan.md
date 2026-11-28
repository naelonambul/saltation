# Plan — generator-contract-registry

## Context

GPT-5.6 Pro audit finding `F-004`: the Python generator currently declares bank endpoint paths and accepted HTTP statuses in three separate places (`generator/saltation_gen/client/bank.py`, `generator/saltation_gen/drivers/stage6_execute.py`, and `generator/saltation_gen/drivers/cards.py`), and `checks.json` does not route `contracts/**` changes to `py-lint` or `py-test`.

## Implementation steps

1. **Create a single contract operation registry (`generator/saltation_gen/client/contracts.py`)**
   - Define frozen `OperationSpec(spec: str, method: str, path: str, statuses: tuple[int, ...])` and a single `OPERATIONS` mapping for every bank operation used by `BankClient`, Stage 6, and the card driver.
   - Keep zero new runtime or lockfile dependencies and zero new comments/docstrings (`check_new_comments.py`).
   - Preserve `generator/pyproject.toml` `import-linter` boundaries (`drivers -> client`).

2. **Adopt the registry across `BankClient` and drivers (`generator/saltation_gen/client/bank.py`, `stage6_execute.py`, `cards.py`)**
   - Update `BankClient` methods to resolve HTTP method, path template (with path-parameter formatting where needed), and accepted statuses from `OPERATIONS` rather than repeating inline path literals.
   - Replace the duplicated `ENDPOINTS` dicts in `generator/saltation_gen/drivers/stage6_execute.py` and `generator/saltation_gen/drivers/cards.py` with mappings backed by `OPERATIONS[...].path`.
   - Preserve all existing request IDs, simulated context headers, bounded 503 retry rules, truth-before-dispatch, and recovery semantics unchanged.

3. **Add fast OpenAPI conformance tests (`generator/tests/test_contract_conformance.py`)**
   - Parse `contracts/openapi/{public,internal,inbound,atm,card}.yaml` using Python stdlib (no external YAML dependency).
   - Verify every entry in `OPERATIONS` exists in its declared OpenAPI document with matching HTTP method, exact path template, and documented response statuses.
   - Verify every `stage6_execute.ENDPOINTS` and `cards.ENDPOINTS` value matches `OPERATIONS`.
   - Include negative fixture tests proving that a mutated OpenAPI path, method, or removed response status causes conformance verification to fail.

4. **Route `contracts/**` through Python generator checks (`checks.json`)**
   - Add `"contracts/**"` to `py-lint.paths` and `py-test.paths` in `checks.json`.

5. **Mount `contracts/` read-only in the Python CI container (`scripts/dev/ci-py.sh`)**
   - Add `-v "$root/contracts":/contracts:ro` to `docker run` in `scripts/dev/ci-py.sh` so `test_contract_conformance.py` (`Path(__file__).resolve().parents[2] / "contracts" / "openapi"`) resolves `/contracts/openapi` inside the container.

## Verification

- Run `bash -n scripts/dev/ci-py.sh`.

- Run `uv run --project generator --frozen pytest generator/tests` (all generator tests including `test_contract_conformance.py`).
- Run `uv run --project generator --frozen ruff check generator` and `ruff format --check generator`.
- Run `uv run --project generator --frozen lint-imports --config generator/pyproject.toml --no-cache`.
- Run `python3 scripts/ops/check_new_comments.py`.
- Run `python3 scripts/repo.py status --change generator-contract-registry --base stage6-run-identity-v2`.
- Commit locally on branch `generator-contract-registry` (no push while OCI 365-day backfill is active).

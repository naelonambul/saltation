import re
from pathlib import Path

import pytest

from saltation_gen.client.contracts import OPERATIONS, OperationSpec, operation_path
from saltation_gen.drivers import cards, stage6_execute

OPENAPI_DIR = Path(__file__).resolve().parents[2] / "contracts" / "openapi"
SPEC_NAMES = ("public", "internal", "inbound", "atm", "card")
HTTP_METHODS = frozenset({"get", "put", "post", "delete", "options", "head", "patch", "trace"})
PATH_KEY = re.compile(r"""(["']?)(/[^\s"']+)\1:\s*(?:#.*)?""")
STATUS_KEY = re.compile(r"""(["']?)([1-5][0-9]{2})\1:(?:\s|$)""")

type OpenAPIPaths = dict[str, dict[str, set[int]]]


def scan_flow_delta(entry: str) -> int:
    delta = 0
    quote: str | None = None
    i = 0
    length = len(entry)
    while i < length:
        ch = entry[i]
        if quote == '"':
            if ch == "\\":
                i += 2
                continue
            if ch == '"':
                quote = None
        elif quote == "'":
            if ch == "'":
                if i + 1 < length and entry[i + 1] == "'":
                    i += 2
                    continue
                quote = None
        elif ch == "#" and (i == 0 or entry[i - 1].isspace()):
            break
        elif ch in ('"', "'") and (i == 0 or entry[i - 1] in " \t:,[{"):
            quote = ch
        elif ch in "{[":
            delta += 1
        elif ch in "}]":
            delta -= 1
        i += 1
    if quote is not None:
        raise ValueError("OpenAPI paths must use single-line quoted scalars")
    return delta


def extract_paths(document: str) -> OpenAPIPaths:
    paths: OpenAPIPaths = {}
    in_paths = in_responses = False
    flow_depth = 0
    methods: dict[str, set[int]] | None = None
    statuses: set[int] | None = None
    for line in document.splitlines():
        entry = line.lstrip()
        if not entry or entry.startswith("#"):
            continue
        indent = len(line) - len(entry)
        if indent == 0 and flow_depth == 0:
            if in_paths:
                break
            key, sep, _ = entry.partition(":")
            in_paths = bool(sep) and key == "paths"
            continue
        if not in_paths:
            continue
        in_flow = flow_depth > 0
        flow_depth += scan_flow_delta(entry)
        if flow_depth < 0:
            raise ValueError("Unbalanced YAML flow collection in OpenAPI paths")
        if in_flow:
            continue
        key, sep, _ = entry.partition(":")
        if not sep:
            continue
        if indent == 2:
            path = PATH_KEY.fullmatch(entry)
            methods = paths.setdefault(path[2], {}) if path else None
            statuses = None
            in_responses = False
        elif indent == 4:
            statuses = methods.setdefault(key, set()) if methods is not None and key in HTTP_METHODS else None
            in_responses = False
        elif indent == 6:
            in_responses = key == "responses"
        elif indent == 8 and in_responses and statuses is not None and (status := STATUS_KEY.match(entry)):
            statuses.add(int(status[2]))
    return paths


def assert_conforms(name: str, operation: OperationSpec, paths: OpenAPIPaths) -> None:
    location = f"{operation.spec}.yaml: {name} ({operation.method} {operation.path})"
    assert operation.path in paths, f"{location}: missing path"
    methods = paths[operation.path]
    method = operation.method.lower()
    assert method in methods, f"{location}: missing method"
    missing_statuses = set(operation.statuses) - methods[method]
    assert not missing_statuses, f"{location}: missing response statuses {sorted(missing_statuses)}"


@pytest.fixture(scope="module")
def openapi_paths() -> dict[str, OpenAPIPaths]:
    return {
        spec: extract_paths((OPENAPI_DIR / f"{spec}.yaml").read_text(encoding="utf-8")) for spec in SPEC_NAMES
    }


@pytest.mark.parametrize("name", OPERATIONS)
def test_operation_conforms_to_openapi(name: str, openapi_paths: dict[str, OpenAPIPaths]) -> None:
    operation = OPERATIONS[name]
    assert_conforms(name, operation, openapi_paths[operation.spec])


def test_driver_endpoints_match_registry() -> None:
    stage6_operations = {
        "login": "login",
        "bad_login": "login",
        "reset_request": "password_reset_request",
        "sms": "sms",
        "reset_confirm": "password_reset_confirm",
        "payee": "register_payee",
        "transfer": "transfer",
        "balance": "balance",
        "term_identity": "deposit_identity",
        "term_open": "open_deposit",
        "term_close": "close_deposit",
        "inbound": "inbound_deposit",
        "atm_start": "atm_start_session",
        "atm_withdraw": "atm_withdraw",
        "atm_deposit": "atm_deposit",
        "card": "card_authorize",
    }
    expected_stage6 = {step: OPERATIONS[name].path for step, name in stage6_operations.items()}
    for step in ("term_identity", "term_close"):
        expected_stage6[step] = re.sub(r"\{[^{}]+\}", "{product}", expected_stage6[step])
    expected_cards = {
        action: OPERATIONS[stage6_operations[action]].path
        for action in ("login", "atm_start", "atm_withdraw", "card")
    }
    assert expected_stage6 == stage6_execute.ENDPOINTS
    assert expected_cards == cards.ENDPOINTS


@pytest.fixture
def contract_fixture() -> str:
    return """\
paths:
  /fixture/{id}:
    get:
      responses:
        "200": {description: Found}
        '404':
          description: Missing
    delete:
      responses:
        404: {description: Missing}
components:
  /outside:
    get:
      responses:
        "500": {description: Outside paths}
"""


@pytest.mark.parametrize(
    ("old", "new", "error"),
    [
        ("/fixture/{id}", "/renamed/{id}", "missing path"),
        ("    get:", "    post:", "missing method"),
        ("        '404':\n          description: Missing\n", "", r"missing response statuses \[404\]"),
    ],
)
def test_conformance_rejects_contract_drift(contract_fixture: str, old: str, new: str, error: str) -> None:
    operation = OperationSpec("fixture", "GET", "/fixture/{id}", (200, 404))
    assert_conforms("fixture_lookup", operation, extract_paths(contract_fixture))
    mutated = contract_fixture.replace(old, new, 1)
    with pytest.raises(AssertionError, match=error):
        assert_conforms("fixture_lookup", operation, extract_paths(mutated))


def test_extractor_rejects_multiline_response_description(contract_fixture: str) -> None:
    mutated = contract_fixture.replace(
        "        '404':\n          description: Missing\n",
        "        default: {description: 'Response text\n"
        '        "404": {description: Missing}\n'
        "        continued'}\n",
        1,
    )
    with pytest.raises(ValueError, match="single-line quoted scalars"):
        extract_paths(mutated)


def test_extractor_ignores_nested_status_in_multiline_flow_mapping(contract_fixture: str) -> None:
    mutated = contract_fixture.replace(
        "        '404':\n          description: Missing\n",
        '        x-metadata: {\n        "404": "nested"\n        }\n',
        1,
    )
    operation = OperationSpec("fixture", "GET", "/fixture/{id}", (200, 404))
    with pytest.raises(AssertionError, match=r"missing response statuses \[404\]"):
        assert_conforms("fixture_lookup", operation, extract_paths(mutated))


def test_operation_path_resolves_parameters() -> None:
    assert operation_path("sms", phone="01012345678") == "/internal/v1/sms/01012345678"
    with pytest.raises(ValueError, match="requires path parameters"):
        operation_path("sms")
    with pytest.raises(KeyError, match="phone"):
        operation_path("sms", id="1")

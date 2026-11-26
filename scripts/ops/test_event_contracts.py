from dataclasses import dataclass
from datetime import datetime
import json
import math
from pathlib import Path
import re
import unittest


ROOT = Path(__file__).resolve().parents[2]
CONTRACTS = ROOT / "contracts"
RUST_SOURCE = ROOT / "bank/src"
ATM = Path("features/atm")
DRAFT = "https://json-schema.org/draft/2020-12/schema"
KEYWORDS = {
    "$schema", "$id", "$defs", "$ref", "title", "description",
    "type", "const", "enum", "pattern", "format", "required", "properties",
    "additionalProperties", "anyOf", "oneOf", "minimum", "maximum",
}
DATE_TIME = re.compile(
    r"[0-9]{4}-[0-9]{2}-[0-9]{2}[Tt](?:[01][0-9]|2[0-3]):[0-5][0-9]:[0-5][0-9]"
    r"(?:\.[0-9]+)?(?:[Zz]|[+-](?:[01][0-9]|2[0-3]):[0-5][0-9])"
)
PATTERN_TOKEN = re.compile(r"\\.|\[(?:\\.|[^\]\\])*\]|(\$)")
JSON_TYPES = {
    "null": type(None), "boolean": bool, "string": str, "array": list, "object": dict,
}


class ValidationError(ValueError):
    pass


def is_number(value):
    return type(value) is int or (type(value) is float and math.isfinite(value))


def same_json_value(left, right):
    if is_number(left) and is_number(right):
        return left == right
    if type(left) is not type(right):
        return False
    if isinstance(left, dict):
        return left.keys() == right.keys() and all(
            same_json_value(left[key], right[key]) for key in left
        )
    if isinstance(left, list):
        return len(left) == len(right) and all(
            same_json_value(a, b) for a, b in zip(left, right)
        )
    return left == right


def matches_type(value, kind):
    if kind == "number":
        return is_number(value)
    if kind == "integer":
        return is_number(value) and (type(value) is int or value.is_integer())
    return type(value) is JSON_TYPES[kind]


def unique_strings(values):
    return (
        isinstance(values, list)
        and all(isinstance(value, str) for value in values)
        and len(values) == len(set(values))
    )


def check_schema(schema, root):
    if isinstance(schema, bool):
        return
    if not isinstance(schema, dict):
        raise ValueError("Schema must be an object or boolean")
    unsupported = schema.keys() - KEYWORDS
    if unsupported:
        raise ValueError(f"Unsupported schema keywords: {sorted(unsupported)}")
    for keyword in ("$schema", "$id", "$ref", "title", "description", "pattern", "format"):
        if keyword in schema and not isinstance(schema[keyword], str):
            raise ValueError(f"{keyword} must be a string")
    if "type" in schema:
        kinds = schema["type"]
        if isinstance(kinds, str):
            kinds = [kinds]
        if not kinds or not unique_strings(kinds) or set(kinds) - (JSON_TYPES.keys() | {"number", "integer"}):
            raise ValueError("type must name one or more distinct JSON types")
    if "required" in schema and not unique_strings(schema["required"]):
        raise ValueError("required must be an array of distinct strings")
    if "enum" in schema and (not isinstance(schema["enum"], list) or not schema["enum"]):
        raise ValueError("enum must be a non-empty array")
    for keyword in ("minimum", "maximum"):
        if keyword in schema and not is_number(schema[keyword]):
            raise ValueError(f"{keyword} must be a number")
    if "pattern" in schema:
        re.compile(schema["pattern"], flags=re.ASCII)
    if "format" in schema and schema["format"] != "date-time":
        raise ValueError(f"Unsupported format: {schema['format']}")
    if "$ref" in schema:
        resolve_reference(schema["$ref"], root)
    for keyword in ("properties", "$defs"):
        children = schema.get(keyword, {})
        if not isinstance(children, dict):
            raise ValueError(f"{keyword} must be an object")
        for child in children.values():
            check_schema(child, root)
    for keyword in ("anyOf", "oneOf"):
        if keyword not in schema:
            continue
        children = schema[keyword]
        if not isinstance(children, list) or not children:
            raise ValueError(f"{keyword} must be a non-empty array of schemas")
        for child in children:
            check_schema(child, root)
    if "additionalProperties" in schema:
        check_schema(schema["additionalProperties"], root)


def resolve_reference(reference, root):
    if not reference.startswith("#/$defs/"):
        raise ValueError(f"Unsupported schema reference: {reference}")
    target = root
    for part in reference[2:].split("/"):
        target = target[part.replace("~1", "/").replace("~0", "~")]
    return target


def matches_pattern(pattern, value):
    strict_pattern = PATTERN_TOKEN.sub(
        lambda token: r"\Z" if token[1] else token[0], pattern
    )
    return re.search(strict_pattern, value, flags=re.ASCII) is not None


def validate(value, schema, root, path="$"):
    if schema is False:
        raise ValidationError(f"{path}: rejected by false schema")
    if schema is True:
        return
    if "$ref" in schema:
        validate(value, resolve_reference(schema["$ref"], root), root, path)
    if "type" in schema:
        kinds = schema["type"]
        if isinstance(kinds, str):
            kinds = [kinds]
        if not any(matches_type(value, kind) for kind in kinds):
            raise ValidationError(f"{path}: expected type {', '.join(kinds)}, got {type(value).__name__}")
    if "const" in schema and not same_json_value(value, schema["const"]):
        raise ValidationError(f"{path}: value differs from const")
    if "enum" in schema and not any(same_json_value(value, item) for item in schema["enum"]):
        raise ValidationError(f"{path}: value outside enum")
    if isinstance(value, str):
        if "pattern" in schema and not matches_pattern(schema["pattern"], value):
            raise ValidationError(f"{path}: value does not match pattern")
        if schema.get("format") == "date-time":
            try:
                if not DATE_TIME.fullmatch(value):
                    raise ValueError(value)
                datetime.fromisoformat(value.upper())
            except ValueError:
                raise ValidationError(f"{path}: expected date-time with offset") from None
    if is_number(value):
        if "minimum" in schema and value < schema["minimum"]:
            raise ValidationError(f"{path}: below minimum")
        if "maximum" in schema and value > schema["maximum"]:
            raise ValidationError(f"{path}: above maximum")
    if isinstance(value, dict):
        missing = set(schema.get("required", ())) - value.keys()
        if missing:
            raise ValidationError(f"{path}: missing required properties {sorted(missing)}")
        properties = schema.get("properties", {})
        additional = schema.get("additionalProperties", True)
        for key, item in value.items():
            if key not in properties and additional is False:
                raise ValidationError(f"{path}.{key}: additional property")
            validate(item, properties.get(key, additional), root, f"{path}.{key}")
    for keyword in ("anyOf", "oneOf"):
        if keyword not in schema:
            continue
        failures = []
        matches = 0
        for alternative in schema[keyword]:
            try:
                validate(value, alternative, root, path)
            except ValidationError as error:
                failures.append(str(error))
            else:
                matches += 1
        if not matches or (keyword == "oneOf" and matches != 1):
            raise ValidationError(f"{path}: {keyword} matched {matches} alternatives. {'; '.join(failures)}")


RUST_TOKEN = re.compile(
    r'(?P<skip>\s+|//[^\n]*)|(?P<block>/\*)|'
    r'(?P<raw>(?:br|cr|r)(?P<hashes>\#*)"(?P<raw_text>.*?)"(?P=hashes))|'
    r'(?P<string>"(?:[^"\\]|\\.)*")|'
    r"(?P<char>'(?:[^'\\\n]|\\(?:u\{[0-9a-fA-F_]+\}|x[0-9a-fA-F]{2}|.))')|"
    r'(?:r\#)?[A-Za-z_][A-Za-z_0-9]*|::|->|=>|==|!=|<=|>=|.',
    re.DOTALL,
)
BLOCK_COMMENT = re.compile(r"/\*|\*/")
DELIMITERS = {"(": ")", "[": "]", "{": "}"}
IDENTIFIER = re.compile(r"[A-Za-z_][A-Za-z_0-9]*")


@dataclass(frozen=True)
class RustString:
    text: str


@dataclass
class RustGroup:
    opener: str
    tokens: list


def rust_tokens(source):
    stack = [RustGroup("", [])]
    pos = 0
    while pos < len(source):
        match = RUST_TOKEN.match(source, pos)
        pos = match.end()
        token = match[0]
        if match["skip"] is not None:
            continue
        if match["block"]:
            depth = 1
            while depth:
                boundary = BLOCK_COMMENT.search(source, pos)
                if boundary is None:
                    raise ValueError("Unclosed Rust block comment")
                pos = boundary.end()
                depth += 1 if boundary[0] == "/*" else -1
            continue
        if match["raw"] is not None:
            token = RustString(match["raw_text"])
        elif match["string"]:
            token = RustString(token[1:-1])
        elif token in DELIMITERS:
            group = RustGroup(token, [])
            stack[-1].tokens.append(group)
            stack.append(group)
            continue
        elif token in DELIMITERS.values():
            if len(stack) == 1 or token != DELIMITERS[stack[-1].opener]:
                raise ValueError(f"Unmatched Rust delimiter {token!r}")
            stack.pop()
            continue
        stack[-1].tokens.append(token)
    if len(stack) != 1:
        raise ValueError("Unclosed Rust delimiter")
    return stack[0].tokens


def is_group(token, opener):
    return isinstance(token, RustGroup) and token.opener == opener


def split_tokens(tokens, separator):
    start = 0
    for index, token in enumerate(tokens):
        if token == separator:
            yield tokens[start:index]
            start = index + 1
    if start < len(tokens):
        yield tokens[start:]


@dataclass
class Binding:
    expression: list
    scope: dict
    position: int | None = None
    mutable: bool = False


def event_values(expression, scope, position=None):
    match expression:
        case [RustString(text)] if position is None:
            return {text}
        case [RustGroup("(", inner)] if position is not None:
            return event_values(list(split_tokens(inner, ","))[position], scope)
        case [RustGroup("(", inner)]:
            return event_values(inner, scope)
        case [RustGroup("{", inner)]:
            return event_values(inner, scope, position)
        case [str(name)] if name in scope:
            binding = scope[name]
            if binding.mutable:
                raise ValueError(f"Unsupported mutable event type binding: {name}")
            if binding.position is not None:
                return event_values(binding.expression, binding.scope, binding.position)
            return event_values(binding.expression, binding.scope, position)
        case ["if", *body]:
            for index, token in enumerate(body):
                if is_group(token, "{") and body[index + 1:index + 2] == ["else"]:
                    return event_values([token], scope, position) | event_values(
                        body[index + 2:], scope, position
                    )
        case ["match", *_, RustGroup("{", arms)]:
            values = set()
            while arms:
                arrow = arms.index("=>")
                arm = arms[arrow + 1:]
                if is_group(arm[0], "{"):
                    result, arms = arm[:1], arm[1:]
                    if arms[:1] == [","]:
                        arms = arms[1:]
                else:
                    result = next(split_tokens(arm, ","))
                    arms = arm[len(result) + 1:]
                values.update(event_values(result, scope, position))
            return values
    raise ValueError(f"Unsupported event type expression: {repr(expression)[:200]}")


def remember_binding(statement, scope):
    pattern = statement[1:]
    mutable = pattern[:1] == ["mut"]
    if mutable:
        pattern = pattern[1:]
    if "=" not in pattern:
        return
    equals = pattern.index("=")
    expression = pattern[equals + 1:]
    previous = scope.copy()
    match pattern[0]:
        case str(name) if IDENTIFIER.fullmatch(name):
            scope[name] = Binding(expression, previous, mutable=mutable)
        case RustGroup("(", names):
            for position, target in enumerate(split_tokens(names, ",")):
                match target:
                    case [str(name)] if IDENTIFIER.fullmatch(name):
                        scope[name] = Binding(expression, previous, position)
                    case ["mut", str(name)] if IDENTIFIER.fullmatch(name):
                        scope[name] = Binding(expression, previous, position, mutable=True)


def producer_literals(source, path):
    atm_helper = path.is_relative_to(ATM)

    def walk(tokens, scope, forwarded=None):
        scope = scope.copy()
        helper_body = None
        helper_parameter = None
        for index, token in enumerate(tokens):
            tail = tokens[index + 1:index + 5]
            if token == "let":
                remember_binding(next(split_tokens(tokens[index:], ";")), scope)
            if path == ATM / "mod.rs" and token == "event" and len(tail) == 4:
                if tokens[index - 1:index] == ["fn"] and tail[1:3] == ["->", "Event"]:
                    parameter = list(split_tokens(tail[0].tokens, ","))[4]
                    helper_parameter = parameter[0]
                    helper_body = tail[3]
            producer = None
            if token == "Event" and tail[:2] in (["::", "for_request"], ["::", "system"]):
                if len(tail) >= 3 and is_group(tail[2], "("):
                    producer = tail[1]
                    arguments = tail[2].tokens
                    position = int(producer == "for_request")
            elif atm_helper and token == "event" and tail and is_group(tail[0], "("):
                previous = tokens[index - 1:index]
                if previous == ["::"]:
                    is_helper = tokens[index - 2:index - 1] in (["self"], ["super"])
                else:
                    is_helper = previous not in (["fn"], ["."])
                if is_helper:
                    producer, arguments, position = "ATM event", tail[0].tokens, 4
            if producer is not None:
                try:
                    expression = list(split_tokens(arguments, ","))[position]
                    if forwarded is None or producer != "for_request" or expression != [forwarded]:
                        yield from event_values(expression, scope)
                except (ValueError, IndexError) as error:
                    raise ValueError(f"{path}: {producer}: {error}") from error
            if isinstance(token, RustGroup):
                parameter = helper_parameter if token is helper_body else forwarded
                yield from walk(token.tokens, scope, parameter)

    return set(walk(rust_tokens(source), {}))


def check_inventory(emitted, catalog):
    missing = catalog - emitted
    unexpected = emitted - catalog
    if missing or unexpected:
        raise AssertionError(
            f"Event producer inventory differs from the catalog. "
            f"Without producer: {sorted(missing)}. Uncataloged: {sorted(unexpected)}."
        )


REQUEST = "01ARZ3NDEKTSV4RRFFQ69G5FAV"
EVENT = "01ARZ3NDEKTSV4RRFFQ69G5FAW"
SESSION = "01ARZ3NDEKTSV4RRFFQ69G5FAX"
ATTEMPT = "01ARZ3NDEKTSV4RRFFQ69G5FAY"
OCCURRED = "2026-11-26T12:34:56.123+09:00"
TERMINAL = {
    "terminal_id": "ATM-001", "region": "SEOUL", "latitude": "37.566500",
    "longitude": "126.978000", "account_no": "110123456789", "request_id": REQUEST,
}
CARD = {
    "client_id": "synthetic-client", "idempotency_key": "card-test-01",
    "card_no": "0000000000000000", "krw_amount": 1300, "amount": "0001",
    "currency": "USD", "merchant_name": "테스트 상점", "mcc": "0123", "country": "KR",
    "entry_mode": "online", "latitude": None, "longitude": None, "decline_reason": None,
}
PAYEE = {"payee_id": "7", "bank_code_std": "099", "account_num": "110123456789"}
ATM_ENDED = {
    **TERMINAL, "reason": "expired", "session_id": SESSION,
    "start_request_id": ATTEMPT, "ended_at": OCCURRED,
}
ATM_OPERATION = {
    **TERMINAL, "operation": "balance", "outcome": "approved", "amount": None,
    "rsp_code": "A0000", "bank_rsp_code": "000",
}
BALANCE_COMPLETION = {
    "endpoint": "/v2.0/account/balance/fin_num", "method": "GET",
    "http_status": 200, "duration_ms": 3,
}


def envelope(kind, payload, channel, **fields):
    system = channel == "SYSTEM"
    generator = channel in {"ATM", "CARD", "EXTERNAL"}
    return {
        "schema": "saltation.log/1",
        "event_id": EVENT,
        "event_type": kind,
        "occurred_at": OCCURRED,
        "recorded_at": "2026-11-26T03:34:57.234Z",
        "request_id": None if system else REQUEST,
        "session_id": None,
        "device_id": None,
        "customer_no": None,
        "account_no": None,
        "channel": channel,
        "listener": None if system else ("generator" if generator else "public"),
        "ip": None,
        "region": None,
        "outcome": "completed",
        "rsp_code": None,
        "bank_rsp_code": None,
        "human": not (system or generator),
        "payload": payload,
        **fields,
    }


FIXTURES = [
    envelope("request.completed", {
        "endpoint": "/oauth/2.0/token", "method": "POST", "http_status": 200, "duration_ms": 4,
    }, "EXTERNAL"),
    envelope("request.completed", BALANCE_COMPLETION, "MOBILE"),
    envelope("request.completed", {
        "endpoint": "/v2.0/transfer/withdraw/fin_num", "method": "POST", "duration_ms": 12,
        "http_status": 200, "request_type": "TRANSFER", "tran_amt": 10000,
        "transfer_id": 42, "counterparty_bank_code": "099",
        "counterparty_account_no": "110123456789",
    }, "MOBILE"),
    envelope("request.completed", {
        "endpoint": "/v2.0/transfer/withdraw/fin_num", "method": "POST", "duration_ms": 5,
        "http_status": 422, "request_type": "TRANSFER", "tran_amt": 10000, "reason": "잔액 부족",
    }, "MOBILE", outcome="rejected"),
    envelope("request.completed", {
        "endpoint": "/v2.0/transfer/withdraw/fin_num", "method": "POST", "duration_ms": 2,
        "http_status": 200, "request_type": "TRANSFER", "replayed": True,
    }, "MOBILE"),
    envelope("request.completed", {
        "endpoint": "/atm/v1/cash/withdraw", "method": "POST", "duration_ms": 8,
        "http_status": 422, "request_type": "ATM_WITHDRAW", "attempt_id": ATTEMPT,
        "tran_amt": 10000,
    }, "ATM", outcome="rejected"),
    envelope("request.completed", {
        "endpoint": "/v2.0/transfer/deposit/fin_num", "method": "POST", "duration_ms": 8,
        "http_status": 422, "request_type": "INBOUND", "tran_amt": 10000, "transfer_id": None,
    }, "EXTERNAL", outcome="rejected"),
    envelope("login.succeeded", {"outcome": "SUCCESS", "failed_count": 0}, "MOBILE", session_id=SESSION),
    envelope("login.failed", {"outcome": "WRONG_PASSWORD", "failed_count": 1}, "MOBILE", outcome="rejected"),
    envelope("analyst.login.succeeded", {"analyst_id": "7"}, "ADMIN"),
    envelope("analyst.login.failed", {"analyst_id": None}, "ADMIN", outcome="rejected"),
    envelope("analyst.login.failed", {"analyst_id": "7"}, "ADMIN", outcome="rejected"),
    envelope("logout", {}, "MOBILE", session_id=SESSION),
    envelope("device.new", {}, "MOBILE", session_id=SESSION),
    envelope("password_reset.requested", {"reason": "issued"}, "MOBILE"),
    envelope("password_reset.requested", {"reason": "unknown"}, "MOBILE", outcome="rejected"),
    envelope("password_reset.completed", {"reason": "completed"}, "MOBILE"),
    envelope("password_reset.failed", {"reason": "voided"}, "MOBILE", outcome="rejected"),
    envelope("payee.registered", PAYEE, "MOBILE"),
    envelope("payee.deleted", PAYEE, "MOBILE"),
    envelope("account.status_changed", {"from": "ACTIVE", "to": "DORMANT", "reason": "dormancy"}, "SYSTEM"),
    envelope("rollover.job_completed", {"business_date": "2026-11-26", "job": "atm_session_expiry"}, "SYSTEM"),
    envelope("atm.session_started", {**TERMINAL, "session_id": SESSION}, "ATM", session_id=SESSION),
    envelope("atm.session_rejected", {
        **TERMINAL, "bank_rsp_code": "403", "locked": False,
    }, "ATM", outcome="rejected"),
    envelope("atm.session_rejected", {
        **TERMINAL, "bank_rsp_code": "410", "locked": True,
    }, "ATM", outcome="rejected"),
    envelope("atm.session_ended", ATM_ENDED, "ATM", session_id=SESSION),
    envelope("atm.session_ended", {**ATM_ENDED, "request_id": None}, "SYSTEM", session_id=SESSION),
    envelope("atm.operation", ATM_OPERATION, "ATM", session_id=SESSION, outcome="approved"),
    envelope("atm.operation", {
        **ATM_OPERATION, "operation": "withdraw", "outcome": "rejected",
        "amount": 10000, "rsp_code": "A0004", "bank_rsp_code": "405",
    }, "ATM", session_id=SESSION, outcome="rejected"),
    envelope("card.authorized", CARD, "CARD", outcome="approved", account_no="110123456789"),
    envelope("card.declined", {
        **CARD, "entry_mode": "in_person", "latitude": "37.566500", "longitude": "126.978000",
        "decline_reason": "unknown_card",
    }, "CARD", outcome="rejected"),
]


class EventContracts(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.catalog = set(json.loads((CONTRACTS / "codes/event-types.json").read_text())["event_types"])
        cls.envelope_schema = json.loads((CONTRACTS / "schemas/log-envelope.schema.json").read_text())
        cls.payload_schema = json.loads((CONTRACTS / "schemas/event-payloads.schema.json").read_text())
        cls.definitions = cls.payload_schema["$defs"]

    def validate_event(self, event):
        validate(event, self.envelope_schema, self.envelope_schema)
        kind = event["event_type"]
        if kind not in self.catalog or kind not in self.definitions:
            raise ValidationError(f"$.event_type: unknown event type {kind!r}")
        validate(event["payload"], self.definitions[kind], self.payload_schema, "$.payload")

    def test_producer_catalog_and_payload_coverage(self):
        self.assertEqual(self.catalog, set(self.definitions))
        self.assertTrue(self.catalog)
        producers = {}
        for path in sorted(RUST_SOURCE.rglob("*.rs")):
            relative = path.relative_to(RUST_SOURCE)
            for kind in producer_literals(path.read_text(encoding="utf-8"), relative):
                producers.setdefault(kind, []).append(relative.as_posix())
        unknown_locations = {kind: producers[kind] for kind in sorted(producers.keys() - self.catalog)}
        self.assertFalse(unknown_locations, f"Uncataloged producers by file: {unknown_locations}")
        check_inventory(set(producers), self.catalog)

    def test_schema_documents(self):
        for name, schema in (("envelope", self.envelope_schema), ("payloads", self.payload_schema)):
            with self.subTest(schema=name):
                self.assertEqual(schema["$schema"], DRAFT)
                check_schema(schema, schema)
        for kind, schema in self.definitions.items():
            with self.subTest(kind=kind):
                self.assertIs(schema["additionalProperties"], False)

    def test_emitted_fixtures(self):
        self.assertEqual({event["event_type"] for event in FIXTURES}, self.catalog)
        for index, event in enumerate(FIXTURES):
            with self.subTest(kind=event["event_type"], fixture=index):
                self.validate_event(event)

    def test_missing_required_field(self):
        with self.assertRaisesRegex(ValidationError, r"missing required properties.*failed_count"):
            self.validate_event(envelope("login.failed", {"outcome": "LOCKED"}, "MOBILE"))
        for key in ("endpoint", "method", "duration_ms", "http_status"):
            with self.subTest(key=key):
                payload = BALANCE_COMPLETION.copy()
                del payload[key]
                with self.assertRaisesRegex(ValidationError, rf"missing required properties.*{key}"):
                    self.validate_event(envelope("request.completed", payload, "MOBILE"))

    def test_wrong_field_type(self):
        for amount in ("1000", True):
            with self.subTest(amount=amount):
                with self.assertRaisesRegex(ValidationError, r"\$\.payload\.tran_amt: expected type integer"):
                    self.validate_event(envelope("request.completed", {**BALANCE_COMPLETION, "tran_amt": amount}, "MOBILE"))

    def test_payload_pattern_rejects_trailing_newline(self):
        with self.assertRaisesRegex(ValidationError, r"\$\.payload\.mcc: value does not match pattern"):
            self.validate_event(envelope("card.authorized", {**CARD, "mcc": "0123\n"}, "CARD"))

    def test_extra_payload_field(self):
        with self.assertRaisesRegex(ValidationError, r"\$\.payload\.extra: additional property"):
            self.validate_event(envelope("device.new", {"extra": 1}, "MOBILE"))

    def test_unknown_event_type(self):
        with self.assertRaisesRegex(ValidationError, r"\$\.event_type: unknown event type"):
            self.validate_event(envelope("catalog.missing", {}, "MOBILE"))

    def test_uncataloged_producer(self):
        source = 'Event::for_request(&ctx, "invented.event", now, "MOBILE", "completed");'
        emitted = producer_literals(source, Path("synthetic.rs"))
        with self.assertRaisesRegex(AssertionError, r"Uncataloged: \['invented\.event'\]"):
            check_inventory(self.catalog | emitted, self.catalog)


class ProducerScannerTests(unittest.TestCase):
    def test_dynamic_arguments_and_tuple_bindings(self):
        source = '''
            let (kind, outcome) = if accepted {
                ("analyst.login.succeeded", "ignored.outcome")
            } else {
                ("analyst.login.failed", "ignored.other")
            };
            Event::for_request(context(a, b), kind, now, "ADMIN", outcome);
            let pair = match state {
                State::A => { (r#"case.a"#, "ignore.a") }
                State::B => ("case.b", "ignore.b"),
            };
            let (system_kind, system_outcome) = pair;
            Event::system(system_kind, now, system_outcome);
            super::event(ctx, now, &terminal, account, if accepted {
                "case.c"
            } else if expired {
                "case.d"
            } else {
                "case.e"
            });
        '''
        self.assertEqual(producer_literals(source, ATM / "synthetic.rs"), {
            "analyst.login.succeeded", "analyst.login.failed",
            "case.a", "case.b", "case.c", "case.d", "case.e",
        })

    def test_only_constructor_arguments_count(self):
        source = '''
            // Event::system("comment.event", now, outcome);
            /* outer /* Event::system("nested.comment", now, outcome); */ end */
            let example = r##"Event::system("quoted.example", now, outcome)"##;
            let event_type = "unrelated.type";
            unrelated(event_type);
            audit.event(a, b, c, d, "unrelated.method");
            audit::event(a, b, c, d, "unrelated.helper");
            let kind = "actual.event";
            {
                let kind = "not.emitted";
                show(kind);
            }
            Event::system(kind, now, if x { "not.an.event" } else { "also.not" });
        '''
        self.assertEqual(producer_literals(source, ATM / "synthetic.rs"), {"actual.event"})

    def test_mutable_event_binding_fails_instead_of_ignoring_assignment(self):
        source = '''
            let mut kind = "cataloged.event";
            if accepted {
                kind = "invented.event";
            }
            Event::system(kind, now, outcome);
        '''
        with self.assertRaisesRegex(ValueError, "Unsupported mutable event type binding: kind"):
            producer_literals(source, Path("synthetic.rs"))

    def test_unresolved_arguments_fail_with_location(self):
        with self.assertRaisesRegex(ValueError, r"synthetic\.rs: system: Unsupported event type expression"):
            producer_literals("Event::system(unknown(), now, outcome);", Path("synthetic.rs"))


class SchemaSubsetTests(unittest.TestCase):
    def test_malformed_supported_keywords_fail_schema_check(self):
        for schema in (
            {"required": ["field", "field"]}, {"type": ["integer", "integer"]},
            {"minimum": True}, {"properties": []}, {"oneOf": []},
        ):
            with self.subTest(schema=schema), self.assertRaises(ValueError):
                check_schema(schema, schema)

    def test_local_reference_applies_with_siblings(self):
        schema = {
            "$defs": {"a/b~c": {"type": "number", "minimum": 1}},
            "$ref": "#/$defs/a~1b~0c",
            "maximum": 3,
        }
        check_schema(schema, schema)
        validate(2.5, schema, schema)
        for value in (True, 0, 4):
            with self.subTest(value=value), self.assertRaises(ValidationError):
                validate(value, schema, schema)

    def test_enum_and_const_use_json_equality(self):
        for schema in ({"const": {"value": 1}}, {"enum": [{"value": 1}]}):
            validate({"value": 1.0}, schema, schema)
            with self.assertRaises(ValidationError):
                validate({"value": True}, schema, schema)

    def test_one_of_requires_exactly_one_match(self):
        schema = {"oneOf": [{"type": "integer"}, {"type": "number"}]}
        validate(2.5, schema, schema)
        for value in (2, False):
            with self.subTest(value=value), self.assertRaises(ValidationError):
                validate(value, schema, schema)

    def test_date_time_requires_a_valid_date_and_offset(self):
        schema = {"type": "string", "format": "date-time"}
        validate(OCCURRED, schema, schema)
        for value in (
            "2026-02-30T00:00:00Z", "2026-11-26T12:34:56",
            "2026-11-26 12:34:56Z", "2026-11-26T12:34:56+09:60",
        ):
            with self.subTest(value=value), self.assertRaises(ValidationError):
                validate(value, schema, schema)

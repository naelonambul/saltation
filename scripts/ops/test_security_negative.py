import re
import unittest
from collections.abc import Iterator
from pathlib import Path

from scripts.ops.security_negative_test import (
    SPOOFED_DEVICE,
    SPOOFED_IP,
    SPOOFED_REGION,
    NegativeCheck,
    build_summary,
    evaluate_admin_unauthenticated,
    evaluate_rbac_denial,
    evaluate_restricted_route,
    evaluate_spoofed_event,
)

ROOT = Path(__file__).resolve().parents[2]
COMPOSE_KEY = re.compile(r"""(?P<quote>['"]?)(?P<key>[\w.-]+)(?P=quote)\s*:\s*""")
PORT_SCALAR = r"""(?:"[^"\\]*"|'[^']*')"""


def compose_port_entries(compose: str) -> Iterator[tuple[str, str]]:
    in_services = False
    service = ""
    service_indent = None
    property_indent = None
    ports_indent = None
    for line in compose.splitlines():
        content = line.strip()
        if not content or content.startswith("#"):
            continue
        indent = len(line) - len(line.lstrip())
        if indent == 0:
            key = COMPOSE_KEY.match(content)
            if key is not None and key["key"] == "include":
                raise ValueError(f"Unsupported Compose top-level include: {content}")
            in_services = key is not None and key["key"] == "services"
            if in_services and re.fullmatch(r"(?:#.*)?", content[key.end():]) is None:
                raise ValueError(f"Unsupported Compose services mapping: {content}")
            service_indent = None
            ports_indent = None
            continue
        if not in_services:
            continue
        if service_indent is None:
            service_indent = indent
        if indent == service_indent:
            key = COMPOSE_KEY.match(content)
            if key is None or re.fullmatch(r"(?:#.*)?", content[key.end():]) is None:
                raise ValueError(f"Unsupported Compose service mapping: {content}")
            service = key["key"]
            property_indent = None
            ports_indent = None
            continue
        if ports_indent is not None:
            if indent > ports_indent or (indent == ports_indent and content.startswith("-")):
                item = re.fullmatch(rf"-\s+({PORT_SCALAR})\s*(?:#.*)?", content)
                if item is None:
                    raise ValueError(f"Unsupported Compose port entry: {content}")
                yield service, item[1][1:-1]
                continue
            ports_indent = None
        if property_indent is None:
            property_indent = indent
        if indent != property_indent or content.startswith("- "):
            continue
        key = COMPOSE_KEY.match(content)
        if key is None:
            raise ValueError(f"Unsupported Compose service property: {content}")
        if key["key"] in {"extends", "network_mode"}:
            raise ValueError(f"Unsupported Compose service property: {content}")
        if key["key"] != "ports":
            continue
        value = content[key.end():]
        if not value or value.startswith("#"):
            ports_indent = indent
        else:
            items = re.fullmatch(
                rf"\[\s*((?:{PORT_SCALAR}(?:\s*,\s*{PORT_SCALAR})*\s*,?)?)\s*\]\s*(?:#.*)?", value
            )
            if items is None:
                raise ValueError(f"Unsupported Compose ports list: {content}")
            for item in re.findall(PORT_SCALAR, items[1]):
                yield service, item[1:-1]


def default_loopback_mapping(entry: str) -> tuple[str, str, str]:
    default = re.sub(r"^\$\{[A-Za-z_][A-Za-z0-9_]*:-([^{}]*)\}", r"\1", entry)
    mapping = re.fullmatch(r"127\.0\.0\.1:([0-9]*):([0-9]+)(?:/(tcp|udp|sctp))?", default)
    if mapping is None:
        raise ValueError(f"Compose port default must bind to 127.0.0.1: {entry}")
    return mapping[1], mapping[2], mapping[3] or "tcp"


class SecurityNegativeTests(unittest.TestCase):
    def test_compose_defaults_publish_only_on_ipv4_loopback(self) -> None:
        compose = (ROOT / "infra/compose/compose.yml").read_text(encoding="utf-8")
        mappings = {(service, *default_loopback_mapping(entry)) for service, entry in compose_port_entries(compose)}
        expected = (
            ("oracle", "1521"),
            ("mongo", "27017"),
            ("bank", "8080"),
            ("bank", "8081"),
            ("bank", "9100"),
            ("web", "8088"),
            ("grafana", "3000"),
        )
        self.assertEqual(len(mappings), len(expected))
        for service, port in expected:
            with self.subTest(service=service, port=port):
                self.assertTrue(
                    {(service, port, port, "tcp"), (service, "", port, "tcp")} & mappings,
                    f"Missing TCP loopback mapping for {service}:{port}",
                )

    def test_compose_port_entries_rejects_include_extends_and_network_mode(self) -> None:
        for snippet in (
            "include:\n  - other.yml\nservices:\n  bank:\n    ports: ['127.0.0.1:8080:8080']\n",
            "services:\n  bank:\n    extends:\n      file: other.yml\n      service: bank\n",
            "services:\n  bank:\n    network_mode: host\n",
        ):
            with self.subTest(snippet=snippet):
                with self.assertRaises(ValueError):
                    list(compose_port_entries(snippet))

    def test_compose_port_entries_reads_quoted_inline_and_block_lists(self) -> None:
        compose = (
            'services:\n'
            '  bank:\n'
            '    "ports": ["0.0.0.0:8081:8081", \'127.0.0.1:8080:8080\']\n'
            '  web:\n'
            "    'ports':\n"
            "      - '127.0.0.1:8088:8088'\n"
        )
        self.assertEqual(
            list(compose_port_entries(compose)),
            [("bank", "0.0.0.0:8081:8081"), ("bank", "127.0.0.1:8080:8080"), ("web", "127.0.0.1:8088:8088")],
        )

    def test_compose_loopback_mapping_accepts_fixed_and_ephemeral_ports(self) -> None:
        self.assertEqual(default_loopback_mapping("127.0.0.1:8081:8081"), ("8081", "8081", "tcp"))
        self.assertEqual(default_loopback_mapping("${BANK_PUBLISH:-127.0.0.1:}:8081"), ("", "8081", "tcp"))
        self.assertEqual(default_loopback_mapping("127.0.0.1:8081:8081/udp"), ("8081", "8081", "udp"))

    def test_compose_loopback_mapping_rejects_non_loopback_defaults(self) -> None:
        for binding in ("0.0.0.0:8081", "8081", "[::]:8081"):
            for entry in (f"{binding}:8081", f"${{BANK_PUBLISH:-{binding}}}:8081"):
                with self.subTest(entry=entry), self.assertRaisesRegex(ValueError, "must bind to 127"):
                    default_loopback_mapping(entry)

    def test_restricted_route_requires_404(self) -> None:
        self.assertTrue(evaluate_restricted_route("POST", "/atm/v1/session", 404).passed)
        self.assertFalse(evaluate_restricted_route("POST", "/atm/v1/session", 200).passed)
        self.assertFalse(evaluate_restricted_route("GET", "/metrics", 500).passed)

    def test_admin_unauthenticated_requires_401_or_403(self) -> None:
        self.assertTrue(evaluate_admin_unauthenticated(401).passed)
        self.assertTrue(evaluate_admin_unauthenticated(403).passed)
        self.assertFalse(evaluate_admin_unauthenticated(200).passed)
        self.assertFalse(evaluate_admin_unauthenticated(404).passed)

    def test_spoofed_event_evaluation(self) -> None:
        good = [{"listener": "public", "human": True, "ip": "203.0.113.10", "region": "KR", "device_id": None}]
        self.assertTrue(evaluate_spoofed_event(good).passed)

        leaked_ip = [{"listener": "public", "human": True, "ip": SPOOFED_IP, "region": "KR", "device_id": None}]
        self.assertFalse(evaluate_spoofed_event(leaked_ip).passed)

        leaked_region = [
            {"listener": "public", "human": True, "ip": "203.0.113.10", "region": SPOOFED_REGION, "device_id": None}
        ]
        self.assertFalse(evaluate_spoofed_event(leaked_region).passed)

        leaked_device = [
            {"listener": "public", "human": True, "ip": "203.0.113.10", "region": "KR", "device_id": SPOOFED_DEVICE}
        ]
        self.assertFalse(evaluate_spoofed_event(leaked_device).passed)

        missing_context = [{"listener": "public", "human": True, "ip": "", "region": "KR", "device_id": None}]
        self.assertFalse(evaluate_spoofed_event(missing_context).passed)
        missing_region = [{"listener": "public", "human": True, "ip": "203.0.113.10"}]
        self.assertFalse(evaluate_spoofed_event(missing_region).passed)

        wrong_listener = [{"listener": "generator", "human": False, "ip": "203.0.113.10", "region": "KR"}]
        self.assertFalse(evaluate_spoofed_event(wrong_listener).passed)
        self.assertFalse(evaluate_spoofed_event([]).passed)

    def test_skip_mongo_reports_partial_not_passed(self) -> None:
        checks = [NegativeCheck("http", True, "ok")]
        partial_summary, partial_code = build_summary(checks, skip_mongo=True)
        self.assertEqual(partial_summary["status"], "partial")
        self.assertFalse(partial_summary["passed"])
        self.assertTrue(partial_summary["partial"])
        self.assertEqual(partial_code, 2)

        full_summary, full_code = build_summary(checks, skip_mongo=False)
        self.assertEqual(full_summary["status"], "passed")
        self.assertTrue(full_summary["passed"])
        self.assertFalse(full_summary["partial"])
        self.assertEqual(full_code, 0)

    def test_rbac_denial(self) -> None:
        self.assertTrue(evaluate_rbac_denial("bank->truth", "DENIED: Unauthorized").passed)
        self.assertFalse(evaluate_rbac_denial("bank->truth", "UNEXPECTED_ALLOW").passed)

    def test_cloudflare_worker_scripts_invariants(self) -> None:
        sal_js = (ROOT / "infra/cloudflare/workers/sal.js").read_text(encoding="utf-8")
        sal_api_js = (ROOT / "infra/cloudflare/workers/sal-api.js").read_text(encoding="utf-8")
        self.assertIn("env.SAL.fetch", sal_js)
        self.assertIn('"50080"', sal_js)
        self.assertIn("env.SAL_API.fetch", sal_api_js)
        self.assertIn('"50081"', sal_api_js)
        self.assertIn("CF-Connecting-IP", sal_api_js)
        self.assertIn("CF-IPCountry", sal_api_js)
        self.assertIn('redirect: "manual"', sal_api_js)


if __name__ == "__main__":
    unittest.main()

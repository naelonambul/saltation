import unittest
from pathlib import Path

from scripts.ops.security_negative_test import (
    SPOOFED_IP,
    SPOOFED_REGION,
    evaluate_rbac_denial,
    evaluate_restricted_route,
    evaluate_spoofed_event,
)

ROOT = Path(__file__).resolve().parents[2]


class SecurityNegativeTests(unittest.TestCase):
    def test_restricted_route_requires_404(self) -> None:
        self.assertTrue(evaluate_restricted_route("POST", "/atm/v1/session", 404).passed)
        self.assertFalse(evaluate_restricted_route("POST", "/atm/v1/session", 200).passed)
        self.assertFalse(evaluate_restricted_route("GET", "/metrics", 500).passed)

    def test_spoofed_event_evaluation(self) -> None:
        good = [{"listener": "public", "human": True, "ip": "203.0.113.10", "region": "KR"}]
        self.assertTrue(evaluate_spoofed_event(good).passed)

        leaked_ip = [{"listener": "public", "human": True, "ip": SPOOFED_IP, "region": "KR"}]
        self.assertFalse(evaluate_spoofed_event(leaked_ip).passed)

        leaked_region = [{"listener": "public", "human": True, "ip": "203.0.113.10", "region": SPOOFED_REGION}]
        self.assertFalse(evaluate_spoofed_event(leaked_region).passed)

        wrong_listener = [{"listener": "generator", "human": False, "ip": "203.0.113.10", "region": "KR"}]
        self.assertFalse(evaluate_spoofed_event(wrong_listener).passed)
        self.assertFalse(evaluate_spoofed_event([]).passed)

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

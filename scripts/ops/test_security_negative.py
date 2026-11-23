import unittest
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


class SecurityNegativeTests(unittest.TestCase):
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

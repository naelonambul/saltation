#!/usr/bin/env python3

from __future__ import annotations

import argparse
import json
import os
import subprocess
import sys
import time
import urllib.error
import urllib.request
from dataclasses import asdict, dataclass
from pathlib import Path
from typing import Any

RESTRICTED_ROUTES: tuple[tuple[str, str], ...] = (
    ("POST", "/atm/v1/session"),
    ("POST", "/card/v1/authorize"),
    ("GET", "/internal/v1/clock"),
    ("POST", "/v2.0/transfer/deposit/fin_num"),
    ("GET", "/metrics"),
)

SPOOFED_IP = "198.51.100.99"
SPOOFED_REGION = "US-CA"
SPOOFED_DEVICE = "spoofed-device-probe"


@dataclass
class NegativeCheck:
    name: str
    passed: bool
    detail: str


def http_request(
    method: str,
    url: str,
    *,
    headers: dict[str, str] | None = None,
    body: bytes | None = None,
    timeout: float = 10.0,
) -> tuple[int, dict[str, str], bytes]:
    req_headers = {"User-Agent": "saltation-security-probe/1.0"}
    if headers:
        req_headers.update(headers)
    req = urllib.request.Request(url, data=body, headers=req_headers, method=method)
    try:
        with urllib.request.urlopen(req, timeout=timeout) as resp:
            return resp.status, {k.lower(): v for k, v in resp.headers.items()}, resp.read()
    except urllib.error.HTTPError as err:
        return err.code, {k.lower(): v for k, v in err.headers.items()}, err.read()


def evaluate_restricted_route(method: str, path: str, status: int) -> NegativeCheck:
    return NegativeCheck(
        name=f"tunnel blocks {method} {path}",
        passed=status == 404,
        detail=f"HTTP {status} (expected 404)",
    )


def evaluate_admin_unauthenticated(status: int) -> NegativeCheck:
    return NegativeCheck(
        name="unauthenticated /admin/v1/customers denied",
        passed=status in (401, 403),
        detail=f"HTTP {status} (expected 401 or 403)",
    )


def evaluate_spoofed_event(events: list[dict[str, Any]]) -> NegativeCheck:
    if not events:
        return NegativeCheck("public ignores X-Sim-* headers", False, "no bank_logs.events found for probe")
    for ev in events:
        if ev.get("listener") != "public" or ev.get("human") is not True:
            return NegativeCheck(
                "public ignores X-Sim-* headers",
                False,
                f"unexpected listener/human: {ev.get('listener')}/{ev.get('human')}",
            )
        ip = ev.get("ip")
        region = ev.get("region")
        device_id = ev.get("device_id")
        if not isinstance(ip, str) or not ip.strip() or not isinstance(region, str) or not region.strip():
            return NegativeCheck(
                "public ignores X-Sim-* headers",
                False,
                f"missing trusted edge context: ip={ip!r} region={region!r}",
            )
        if ip == SPOOFED_IP or region == SPOOFED_REGION:
            return NegativeCheck(
                "public ignores X-Sim-* headers",
                False,
                f"spoofed context leaked: ip={ip} region={region}",
            )
        if device_id is not None:
            return NegativeCheck(
                "public ignores X-Sim-* headers",
                False,
                f"unexpected device_id persisted (X-Sim-Device leak): {device_id!r}",
            )
    return NegativeCheck(
        "public ignores X-Sim-* headers",
        True,
        f"verified {len(events)} event(s): listener=public, human=true, device_id=None, region={events[0].get('region')}",
    )


def evaluate_rbac_denial(label: str, output: str) -> NegativeCheck:
    denied = "DENIED: Unauthorized" in output
    return NegativeCheck(
        name=label,
        passed=denied,
        detail=output.strip() or "empty output",
    )


def load_env_file(path: Path) -> dict[str, str]:
    env: dict[str, str] = {}
    if not path.exists():
        return env
    for line in path.read_text(encoding="utf-8").splitlines():
        line = line.strip()
        if not line or line.startswith("#") or "=" not in line:
            continue
        k, v = line.split("=", 1)
        env[k.strip()] = v.strip()
    return env


def run_mongo_eval(
    *,
    container: str,
    user: str,
    env_var_name: str,
    auth_db: str,
    target_db: str,
    js_expr: str,
    env_map: dict[str, str],
) -> str:
    shell_cmd = (
        f'mongosh --quiet -u {user} -p "${env_var_name}" '
        f'--authenticationDatabase {auth_db} {target_db} --eval "$1"'
    )
    proc = subprocess.run(
        ["docker", "exec", "-e", env_var_name, container, "sh", "-c", shell_cmd, "_", js_expr],
        check=True,
        capture_output=True,
        text=True,
        env=env_map,
    )
    return proc.stdout.strip()


def run_suite(
    *,
    web_url: str,
    api_url: str,
    mongo_container: str,
    env_file: Path,
    skip_mongo: bool = False,
) -> list[NegativeCheck]:
    web = web_url.rstrip("/")
    api = api_url.rstrip("/")
    results: list[NegativeCheck] = []

    w_status, _, _ = http_request("GET", f"{web}/healthz")
    results.append(NegativeCheck("web /healthz reachable", w_status == 200, f"HTTP {w_status}"))
    a_status, _, _ = http_request("GET", f"{api}/healthz")
    results.append(NegativeCheck("api /healthz reachable", a_status == 200, f"HTTP {a_status}"))

    c_status, c_headers, _ = http_request(
        "OPTIONS",
        f"{api}/v2.0/ext/auth/login",
        headers={
            "Origin": web,
            "Access-Control-Request-Method": "POST",
            "Access-Control-Request-Headers": "content-type,authorization",
        },
    )
    acao = c_headers.get("access-control-allow-origin", "")
    results.append(
        NegativeCheck(
            "CORS allows configured web origin",
            c_status == 200 and acao == web,
            f"HTTP {c_status}, ACAO={acao!r}",
        )
    )

    for method, path in RESTRICTED_ROUTES:
        st, _, _ = http_request(method, f"{api}{path}")
        results.append(evaluate_restricted_route(method, path, st))

    admin_status, _, _ = http_request("GET", f"{api}/admin/v1/customers")
    results.append(evaluate_admin_unauthenticated(admin_status))

    probe_body = json.dumps(
        {"phone": "010-0000-0000", "password": "invalid-probe-value"}
    ).encode("utf-8")
    p_status, _, p_raw = http_request(
        "POST",
        f"{api}/v2.0/ext/auth/login",
        headers={
            "Content-Type": "application/json",
            "X-Sim-Ip": SPOOFED_IP,
            "X-Sim-Region": SPOOFED_REGION,
            "X-Sim-Device": SPOOFED_DEVICE,
        },
        body=probe_body,
    )
    try:
        probe_json = json.loads(p_raw.decode("utf-8")) if p_raw else {}
    except (UnicodeDecodeError, json.JSONDecodeError):
        probe_json = {}
    req_id = probe_json.get("request_id", "")
    results.append(
        NegativeCheck(
            "login probe rejected with request_id",
            p_status == 401 and bool(req_id),
            f"HTTP {p_status}, request_id={req_id!r}",
        )
    )

    if skip_mongo:
        return results

    env_map = dict(os.environ)
    for k, v in load_env_file(env_file).items():
        env_map.setdefault(k, v)

    events: list[dict[str, Any]] = []
    if req_id and env_map.get("MONGO_BANK_PASSWORD"):
        for _ in range(10):
            out = run_mongo_eval(
                container=mongo_container,
                user="bank",
                env_var_name="MONGO_BANK_PASSWORD",
                auth_db="bank_logs",
                target_db="bank_logs",
                js_expr=f'print(JSON.stringify(db.events.find({{request_id: "{req_id}"}}, {{_id: 0, event_type: 1, listener: 1, human: 1, ip: 1, region: 1, device_id: 1}}).toArray()))',
                env_map=env_map,
            )
            if out:
                events = json.loads(out.splitlines()[-1])
                if events:
                    break
            time.sleep(0.5)
    results.append(evaluate_spoofed_event(events))

    bank_to_truth = run_mongo_eval(
        container=mongo_container,
        user="bank",
        env_var_name="MONGO_BANK_PASSWORD",
        auth_db="bank_logs",
        target_db="truth",
        js_expr='try { db.labels.findOne(); print("UNEXPECTED_ALLOW"); } catch (e) { print("DENIED: " + e.codeName); }',
        env_map=env_map,
    )
    results.append(evaluate_rbac_denial("MongoDB RBAC denies bank on truth", bank_to_truth))

    gen_to_logs = run_mongo_eval(
        container=mongo_container,
        user="generator",
        env_var_name="MONGO_GENERATOR_PASSWORD",
        auth_db="truth",
        target_db="bank_logs",
        js_expr='try { db.events.findOne(); print("UNEXPECTED_ALLOW"); } catch (e) { print("DENIED: " + e.codeName); }',
        env_map=env_map,
    )
    results.append(evaluate_rbac_denial("MongoDB RBAC denies generator on bank_logs", gen_to_logs))

    return results


def build_parser() -> argparse.ArgumentParser:
    p = argparse.ArgumentParser(description="Stage 8 automated security negative tests")
    p.add_argument("--web-url", required=True, help="Public web origin URL (e.g. https://sal.i234.workers.dev)")
    p.add_argument(
        "--api-url", required=True, help="Public API origin URL (e.g. https://sal-api.i234.workers.dev)"
    )
    p.add_argument("--mongo-container", default="saltation-mongo-1")
    p.add_argument("--env-file", default="infra/compose/.env")
    p.add_argument("--skip-mongo", action="store_true", help="Skip Docker MongoDB checks (HTTP-only mode)")
    return p


def build_summary(checks: list[NegativeCheck], *, skip_mongo: bool) -> tuple[dict[str, Any], int]:
    all_ok = all(c.passed for c in checks)
    if skip_mongo:
        status = "partial" if all_ok else "failed"
        return (
            {"status": status, "passed": False, "partial": True, "checks": [asdict(c) for c in checks]},
            2 if all_ok else 1,
        )
    return (
        {"status": "passed" if all_ok else "failed", "passed": all_ok, "partial": False, "checks": [asdict(c) for c in checks]},
        0 if all_ok else 1,
    )


def main() -> None:
    args = build_parser().parse_args()
    checks = run_suite(
        web_url=args.web_url,
        api_url=args.api_url,
        mongo_container=args.mongo_container,
        env_file=Path(args.env_file),
        skip_mongo=args.skip_mongo,
    )
    summary, code = build_summary(checks, skip_mongo=args.skip_mongo)
    print(json.dumps(summary, indent=2))
    if code != 0:
        sys.exit(code)


if __name__ == "__main__":
    main()

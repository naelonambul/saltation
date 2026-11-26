"""Read-only N-7/N-8 evidence from the development host's Docker Engine."""

from __future__ import annotations

import asyncio
import json
import os
import re
import time
from collections import Counter
from datetime import datetime
from fractions import Fraction
from hashlib import sha256
from pathlib import Path
from typing import Any

import httpx

CORE_SERVICES = frozenset({"oracle", "mongo", "bank", "web", "alloy", "loki", "prometheus", "grafana"})
OPTIONAL_SERVICES = frozenset({"generator", "loadtest", "cloudflared"})
OPTIONAL_MEMBERS = frozenset(f"saltation/{service}" for service in OPTIONAL_SERVICES)
CO_TENANTS: frozenset[str] = frozenset()
CPU_BUDGET = 4
MEMORY_BUDGET = 14 * 1024**3
PROJECT_LABEL = "com.docker.compose.project"
SERVICE_LABEL = "com.docker.compose.service"
CONFIG_HASH_LABEL = "com.docker.compose.config-hash"
REVISION_LABEL = "org.opencontainers.image.revision"
REVISION = re.compile(r"[0-9a-f]{40}")
NETWORK_LABEL = "com.docker.compose.network"
IMAGE_ID = re.compile(r"sha256:[0-9a-f]{64}")
ORACLE_USER = re.compile(r"[a-zA-Z][a-zA-Z0-9_$#]{0,127}")
HTTP_TARGET = re.compile(r"http://127\.0\.0\.1:([0-9]{1,5})/?")
ORACLE_TARGET = re.compile(r"127\.0\.0\.1:([0-9]{1,5})/([a-zA-Z][a-zA-Z0-9_.-]{0,127})")
ORACLE_INTERNAL = re.compile(r"oracle:1521/([a-zA-Z][a-zA-Z0-9_.-]{0,127})")
BANK_LISTENER = re.compile(r"0\.0\.0\.0:([0-9]{1,5})")
LIFECYCLE_EVENTS = frozenset(
    {"oom", "restart", "start", "die", "stop", "kill", "destroy", "create", "rename", "pause", "unpause"}
)
HEALTH_STATES = frozenset({"healthy", "unhealthy", "starting", "none"})
ID = re.compile(r"[0-9a-f]{64}")
DAEMON_ID = re.compile(r"[a-zA-Z0-9][a-zA-Z0-9:-]{0,127}")
NAME = re.compile(r"[a-zA-Z0-9][a-zA-Z0-9_.-]{0,127}")
PROJECT = re.compile(r"[a-z0-9][a-z0-9_-]*")
TIMESTAMP = re.compile(r"(\d{4}-\d\d-\d\dT\d\d:\d\d:\d\d)(?:\.(\d{1,9}))?(Z|[+-]\d\d:\d\d)")
HISTORY_LIMIT = 256
CHECKPOINT_SECONDS = 10
REPLAY_DRAIN_SECONDS = 0.5
REPLAY_FENCE_NS = 500_000_000
TIMEOUT_SECONDS = 10
MAX_MATERIAL_EVENTS = 20_000

Record = dict[str, Any]


def _matching(value: Any, pattern: re.Pattern[str]) -> str | None:
    return value if isinstance(value, str) and pattern.fullmatch(value) else None


def _timestamp_ns(value: Any) -> int:
    match = TIMESTAMP.fullmatch(value) if isinstance(value, str) else None
    if match is None:
        raise ValueError("invalid Docker timestamp")
    whole, fraction, zone = match.groups()
    return int(datetime.fromisoformat(whole + zone).timestamp()) * 10**9 + int((fraction or "").ljust(9, "0"))


def _event_time(value: int) -> str:
    seconds, nanos = divmod(value, 10**9)
    return f"{seconds}.{nanos:09d}"


def _integer(value: Any) -> int:
    if type(value) is not int:
        raise ValueError("expected integer from Docker")
    return value


def _cpu_limit(config: Record) -> Fraction | None:
    nano = _integer(config.get("NanoCpus", 0))
    quota = _integer(config.get("CpuQuota", 0))
    period = _integer(config.get("CpuPeriod", 0))
    if nano > 0:
        if quota > 0:
            raise ValueError("Docker reported both NanoCpus and an explicit CPU quota")
        return Fraction(nano, 10**9)
    if quota > 0 and period >= 0:
        return Fraction(quota, period or 100_000)
    return None


def _membership(name: Any, labels: Record, project: str) -> str | None:
    if isinstance(name, str) and name in CO_TENANTS:
        if labels.get(PROJECT_LABEL) == project:
            return None
        return f"co-tenant/{name}"
    if labels.get(PROJECT_LABEL) == project:
        service = labels.get(SERVICE_LABEL)
        if isinstance(service, str) and (service in CORE_SERVICES or service in OPTIONAL_SERVICES):
            return f"saltation/{service}"
    return None


def _port(value: Any) -> int | None:
    if isinstance(value, str) and re.fullmatch(r"[0-9]{1,5}", value):
        port = int(value)
        return port if 0 < port <= 65535 and str(port) == value else None
    return None


def _bank_configuration(config: Record, host_config: Record) -> Record:
    defaults = {"SALTATION_GENERATOR_ADDR": "0.0.0.0:8081", "SALTATION_METRICS_ADDR": "0.0.0.0:9100"}
    settings: dict[str, str] = {}
    for entry in config.get("Env") or []:
        key, separator, value = entry.partition("=")
        if separator and (key in defaults or key in {"SALTATION_DB_CONNECT", "SALTATION_DB_USER"}):
            if key in settings:
                raise ValueError("ambiguous bank listener or Oracle setting")
            settings[key] = value
    listeners = {}
    for listener, key in (("bank", "SALTATION_GENERATOR_ADDR"), ("metrics", "SALTATION_METRICS_ADDR")):
        match = BANK_LISTENER.fullmatch(settings.get(key, defaults[key]))
        listeners[listener] = _port(match[1]) if match else None
    db = ORACLE_INTERNAL.fullmatch(settings.get("SALTATION_DB_CONNECT", ""))
    host_override = any(host_config.get(key) for key in ("ExtraHosts", "Links"))
    user = _matching(settings.get("SALTATION_DB_USER"), ORACLE_USER)
    return {
        "listeners": listeners,
        "oracle_service": db[1].upper() if db and not host_override else None,
        "oracle_user": user.upper() if user else None,
    }


def _network_settings(raw: Record) -> dict[str, Record]:
    settings = raw.get("NetworkSettings") or {}
    networks = {}
    for values in (settings.get("Networks") or {}).values():
        network_id = _matching(values.get("NetworkID"), ID)
        if network_id is not None:
            aliases = (values.get("Aliases") or []) + (values.get("DNSNames") or [])
            networks[network_id] = {"has_oracle_alias": "oracle" in aliases}
    return networks


def _loopback_ports(raw: Record) -> dict[str, list[int]]:
    ports = (raw.get("NetworkSettings") or {}).get("Ports") or {}
    return {
        private_port: sorted(
            {
                port
                for mapping in mappings or []
                if mapping.get("HostIp") == "127.0.0.1" and (port := _port(mapping.get("HostPort")))
            }
        )
        for private_port, mappings in ports.items()
        if re.fullmatch(r"[0-9]{1,5}/tcp", private_port)
    }


def _inspect_record(raw: Record, container_id: str, name: str, membership: str) -> Record:
    state, config = raw["State"], raw["HostConfig"]
    if raw["Id"] != container_id or raw["Name"] != f"/{name}":
        raise ValueError("Docker inspect identity disagrees with list")
    running = state["Running"]
    oom = state["OOMKilled"]
    if type(running) is not bool or type(oom) is not bool:
        raise ValueError("missing running or OOM state")
    health = state.get("Health")
    healthcheck = (raw["Config"].get("Healthcheck") or {}).get("Test") or ["NONE"]
    if health is None and healthcheck[0] != "NONE":
        raise ValueError("configured Docker health check has no runtime status")
    health_status = health["Status"] if health is not None else "none"
    if health_status not in HEALTH_STATES:
        raise ValueError("unknown health state")
    _timestamp_ns(state["StartedAt"])
    restarts = _integer(raw["RestartCount"])
    if restarts < 0:
        raise ValueError("invalid restart count")

    cpu = _cpu_limit(config)
    image_id = _matching(raw.get("Image"), IMAGE_ID)
    labels = raw["Config"]["Labels"] or {}
    config_hash = _matching(labels.get(CONFIG_HASH_LABEL), ID)
    if image_id is None or (membership.startswith("saltation/") and config_hash is None):
        raise ValueError("missing Docker image ID or Compose config hash")
    record = {
        "id": container_id,
        "name": name,
        "membership": membership,
        "image_id": image_id,
        "compose_config_hash": config_hash,
        "running": running and state["Status"] == "running",
        "oom_killed": oom,
        "health": health_status,
        "started_at": state["StartedAt"],
        "restart_count": restarts,
        "cpu_limit_vcpu": float(cpu) if cpu is not None else None,
        "cpu_limit_ratio": [cpu.numerator, cpu.denominator] if cpu is not None else None,
        "memory_limit_bytes": _integer(config["Memory"]),
        "oracle_alias_network_ids": sorted(
            network_id
            for network_id, network in _network_settings(raw).items()
            if network["has_oracle_alias"]
        ),
    }
    if membership in {"saltation/bank", "saltation/oracle"}:
        record["network_ids"] = sorted(_network_settings(raw))
        record["loopback_tcp_ports"] = _loopback_ports(raw)
    if membership == "saltation/bank":
        record.update(_bank_configuration(raw["Config"], config))
        container_revision = labels.get(REVISION_LABEL)
        record["container_image_revision"] = _matching(container_revision, REVISION)
        record["container_revision_label_valid"] = (
            container_revision is None or record["container_image_revision"] is not None
        )
    return record


def _material_event(raw: Record) -> Record | None:
    event_type = raw["Type"]
    if event_type == "network" and raw.get("Action") in {"connect", "disconnect", "destroy", "remove"}:
        network_id = _matching(raw["Actor"]["ID"], ID)
        if network_id is None:
            raise ValueError("invalid Docker network event identity")
        return {"id": network_id, "time_ns": raw["timeNano"], "action": f"network {raw['Action']}"}
    if event_type != "container":
        return None
    action = raw["Action"]
    if action not in LIFECYCLE_EVENTS and action != "update" and not action.startswith("health_status"):
        return None
    actor = raw["Actor"]
    attributes = actor["Attributes"]
    container_id = _matching(actor["ID"], ID)
    timestamp = _integer(raw["timeNano"])
    if container_id is None or timestamp <= 0:
        raise ValueError("invalid Docker container event identity or timestamp")
    if action.startswith("health_status"):
        _, _, status = action.partition(": ")
        action = f"health_status: {status}" if status in HEALTH_STATES else "health_status: unknown"
    return {
        "id": container_id,
        "time_ns": timestamp,
        "action": action,
        "name": _matching(attributes.get("name"), NAME),
        "project": _matching(attributes.get(PROJECT_LABEL), PROJECT),
        "service": _matching(attributes.get(SERVICE_LABEL), NAME),
    }


class HostVerifier:
    """Start on the Docker host before issuing load and finish after drain.

    A failed preflight returns a verifier with populated violations/incomplete
    lists. The caller can inspect them before deciding whether to issue load.
    """

    def __init__(self, project: str | None) -> None:
        self.project = project
        self.before: Record | None = None
        self.after: Record | None = None
        self.violations: list[str] = []
        self.incomplete: list[str] = []
        self._client: httpx.AsyncClient | None = None
        self._live: asyncio.Task | None = None
        self._audit: asyncio.Task | None = None
        self._ready = asyncio.Event()
        self._events: dict[tuple, Record] = {}
        self._since: int | None = None
        self._audited_until: int | None = None
        self._daemon_id: str | None = None
        self._checkpoints = 0
        self._state_checks = 0
        self._live_watermark: int | None = None
        self._replay_witness: tuple[int, bytes] | None = None
        self._live_window: Counter[tuple[int, bytes]] = Counter()
        self._target_inputs: tuple[int, int, int, str, str] | None = None
        self._target_binding: Record | None = None
        self._expected_revision: str | None = None
        self._oci_shape: str | None = None
        self._architecture: str | None = None
        self._closed = False
        self._result: Record | None = None

    @classmethod
    async def start(
        cls, *, project: str | None = None, socket_path: str | os.PathLike[str] | None = None
    ) -> HostVerifier:
        if project is None:
            project = os.environ.get("SALTATION_VERIFY_PROJECT")
        verifier = cls(_matching(project, PROJECT))
        if verifier.project is None:
            verifier.incomplete.append("Set SALTATION_VERIFY_PROJECT to the target Compose project.")
            return verifier

        path = os.fspath(
            socket_path
            if socket_path is not None
            else os.environ.get("SALTATION_VERIFY_DOCKER_SOCKET", "/var/run/docker.sock")
        )
        if not Path(path).is_absolute():
            verifier.incomplete.append("Docker verification requires an absolute Unix socket path.")
            return verifier
        verifier._client = httpx.AsyncClient(
            base_url="http://docker",
            transport=httpx.AsyncHTTPTransport(uds=path, trust_env=False),
            trust_env=False,
            timeout=TIMEOUT_SECONDS,
        )
        try:
            await verifier._start()
        except BaseException:
            await verifier.close()
            raise
        return verifier

    async def _get(self, path: str, **params: str) -> Any:
        assert self._client is not None
        async with asyncio.timeout(TIMEOUT_SECONDS):
            response = await self._client.get(path, params=params)
            response.raise_for_status()
            return response.json()

    async def _daemon_time(self) -> int:
        info = await self._get("/info")
        daemon_id = _matching(info["ID"], DAEMON_ID)
        if daemon_id is None:
            raise ValueError("unavailable Docker daemon ID")
        if self._daemon_id is None:
            self._daemon_id = daemon_id
        elif self._daemon_id != daemon_id:
            raise ValueError("Docker daemon changed")
        architecture = _matching(info.get("Architecture"), NAME)
        if self._architecture is None:
            self._architecture = architecture
        elif self._architecture != architecture:
            raise ValueError("Docker daemon architecture changed or could no longer be read")
        if info.get("OSType") != "linux" or not all(
            info.get(field) is True for field in ("MemoryLimit", "CpuCfsQuota")
        ):
            self._gap("Docker daemon did not confirm Linux CPU quota and memory limit support.")
        return _timestamp_ns(info["SystemTime"])

    async def _start(self) -> None:
        try:
            self._since = self._audited_until = await self._daemon_time()
        except (httpx.HTTPError, OSError, KeyError, TypeError, ValueError, AttributeError):
            self._gap("Could not establish Docker daemon identity or clock.")
            return

        self._live = asyncio.create_task(self._collect_live())
        try:
            async with asyncio.timeout(TIMEOUT_SECONDS):
                await self._ready.wait()
        except TimeoutError:
            self._gap("Docker event stream did not become ready before the baseline.")
        self.before = await self._snapshot("before")
        await self._checkpoint()
        self._audit = asyncio.create_task(self._audit_history())

    def _gap(self, message: str) -> None:
        if message not in self.incomplete:
            self.incomplete.append(message)

    async def _collect_live(self) -> None:
        assert self._client is not None
        assert self._since is not None
        try:
            async with self._client.stream(
                "GET",
                "/events",
                params={"since": _event_time(self._since)},
                timeout=httpx.Timeout(TIMEOUT_SECONDS, read=None),
            ) as response:
                response.raise_for_status()
                self._ready.set()
                async for line in response.aiter_lines():
                    observed = self._record_event(line)
                    if observed is None:
                        continue
                    stamp, _ = observed
                    self._live_watermark = max(stamp, self._live_watermark or 0)
                    if self._audited_until is not None and stamp >= self._audited_until:
                        if len(self._live_window) < MAX_MATERIAL_EVENTS:
                            self._live_window[observed] += 1
                        elif observed not in self._live_window:
                            self._gap("Docker live watermark exceeded the bounded reconciliation buffer.")
            self._gap("Docker live event stream ended before verification stopped it.")
        except (httpx.HTTPError, OSError):
            self._gap("Docker live event stream failed before verification stopped it.")
        finally:
            self._ready.set()

    def _record_event(self, line: str) -> tuple[int, bytes] | None:
        try:
            raw = json.loads(line)
            timestamp = _integer(raw["timeNano"])
            if timestamp <= 0:
                raise ValueError("invalid Docker event timestamp")
            event = _material_event(raw) if self._since is not None and timestamp >= self._since else None
        except (KeyError, TypeError, ValueError, AttributeError):
            self._gap("Malformed Docker event prevents complete lifecycle coverage.")
            return None

        if event is not None:
            key = tuple(event.values())
            if len(self._events) < MAX_MATERIAL_EVENTS:
                self._events[key] = event
            elif key not in self._events:
                self._gap("Docker lifecycle evidence exceeded the bounded event buffer.")
        fingerprint = sha256(json.dumps(raw, sort_keys=True).encode()).digest()
        return timestamp, fingerprint

    async def _fenced_replay(self) -> tuple[int, Counter[tuple[int, bytes]]]:
        """Read a server-terminated replay so delayed replay cannot look empty."""
        assert self._client is not None
        cutoff = await self._daemon_time()
        deadline = cutoff + REPLAY_FENCE_NS
        replay: Counter[tuple[int, bytes]] = Counter()
        async with asyncio.timeout(TIMEOUT_SECONDS):
            async with self._client.stream(
                "GET", "/events", params={"since": "1", "until": _event_time(deadline)}
            ) as response:
                response.raise_for_status()
                async for line in response.aiter_lines():
                    observed = self._record_event(line)
                    if observed is None or len(replay) >= MAX_MATERIAL_EVENTS:
                        raise ValueError("invalid or excessive Docker fenced replay")
                    replay[observed] += 1
        if await self._daemon_time() < deadline:
            raise ValueError("Docker fenced replay ended early")
        return cutoff, replay

    async def _checkpoint(self) -> None:
        """Check the latest ring, including events published after the cutoff."""
        assert self._client is not None
        assert self._audited_until is not None
        previous = self._audited_until
        try:
            cutoff, fenced = await self._fenced_replay()
            if cutoff < previous:
                raise ValueError("Docker clock moved backwards")

            # With no upper bound, the initial replay exposes every entry in
            # the latest ring, including ones that arrived after the cutoff.
            replay: Counter[tuple[int, bytes]] = Counter()
            async with self._client.stream("GET", "/events", params={"since": "1"}) as response:
                response.raise_for_status()
                try:
                    async with asyncio.timeout(REPLAY_DRAIN_SECONDS):
                        async for line in response.aiter_lines():
                            observed = self._record_event(line)
                            if observed is None or len(replay) >= MAX_MATERIAL_EVENTS:
                                raise ValueError("invalid or excessive Docker replay")
                            replay[observed] += 1
                        raise ValueError("unbounded Docker replay ended early")
                except TimeoutError:
                    pass

            for records in (fenced, replay):
                overlap = any(stamp <= previous for stamp, _ in records)
                witness_lost = self._replay_witness is not None and self._replay_witness not in records
                if not overlap and (records.total() >= HISTORY_LIMIT or witness_lost):
                    self._gap("Docker latest event history lost overlap with an audited checkpoint.")
                    return
            fenced_through_cutoff = Counter(
                {event: count for event, count in fenced.items() if previous <= event[0] <= cutoff}
            )
            if fenced_through_cutoff - replay:
                self._gap("Docker latest event history did not complete its fenced replay overlap.")
                return
            live_through_cutoff = Counter(
                {event: count for event, count in self._live_window.items() if previous <= event[0] <= cutoff}
            )
            if live_through_cutoff - replay:
                self._gap("Docker latest event history disagreed with the continuous live watermark.")
                return
            if replay:
                self._replay_witness = max(
                    (event for event in replay if event[0] <= cutoff), default=self._replay_witness
                )
            self._audited_until = cutoff
            self._live_window = Counter(
                {event: count for event, count in self._live_window.items() if event[0] >= cutoff}
            )
            self._checkpoints += 1
        except (httpx.HTTPError, KeyError, TypeError, ValueError, AttributeError, OSError):
            self._gap("Could not reconcile Docker latest event history through a checkpoint.")

    async def _audit_history(self) -> None:
        while True:
            await asyncio.sleep(CHECKPOINT_SECONDS)
            snapshot = await self._snapshot("periodic")
            self._compare(snapshot)
            self._state_checks += 1
            self._validate_binding(snapshot, "periodic")
            await self._checkpoint()

    async def _snapshot(self, phase: str) -> Record:
        assert self.project is not None
        snapshot: Record = {
            "containers": {},
            "cpu_limit_vcpu": None,
            "memory_limit_bytes": None,
            "observed_start_unix_ns": time.time_ns(),
        }
        containers = snapshot["containers"]
        gaps_before = len(self.incomplete)
        try:
            listed = await self._get("/containers/json", all="1")
            if not isinstance(listed, list):
                raise ValueError("invalid Docker inventory")
            candidates: dict[str, list[tuple[str, str]]] = {}
            for row in listed:
                self._inventory_row(row, candidates, phase)
        except (httpx.HTTPError, OSError, KeyError, TypeError, ValueError, AttributeError):
            self._gap(f"{phase}: Docker inventory could not be read.")
            return snapshot

        for membership in sorted(candidates):
            matches = candidates[membership]
            if len(matches) != 1:
                self._gap(f"{phase}: ambiguous Docker inventory for {membership}.")
                continue
            container_id, name = matches[0]
            try:
                raw = await self._get(f"/containers/{container_id}/json")
                labels = raw["Config"]["Labels"] or {}
                if _membership(name, labels, self.project) != membership:
                    raise ValueError("Docker inspect membership disagrees with list")
                containers[membership] = _inspect_record(raw, container_id, name, membership)
            except (httpx.HTTPError, OSError, KeyError, IndexError, TypeError, ValueError, AttributeError):
                self._gap(f"{phase}: incomplete Docker inspect evidence for {membership}.")

        required = {f"saltation/{service}" for service in CORE_SERVICES} | {
            f"co-tenant/{name}" for name in CO_TENANTS
        }
        for membership in sorted(required - candidates.keys()):
            self._gap(f"{phase}: missing {membership} from Docker inventory.")
        bank = containers.get("saltation/bank")
        if bank:
            bank["image_revision"] = await self._bank_image_revision(bank["image_id"])
        snapshot["oracle_data_network_id"] = await self._oracle_route(containers)
        snapshot["observed_end_unix_ns"] = time.time_ns()
        self._check_snapshot(snapshot, phase, complete=len(self.incomplete) == gaps_before)
        return snapshot

    async def _bank_image_revision(self, image_id: str) -> str | None:
        try:
            image = await self._get(f"/images/{image_id}/json")
            if image["Id"] == image_id:
                return _matching((image["Config"]["Labels"] or {}).get(REVISION_LABEL), REVISION)
        except (httpx.HTTPError, OSError, KeyError, TypeError, ValueError, AttributeError):
            pass
        return None

    async def _oracle_route(self, containers: Record) -> str | None:
        bank = containers.get("saltation/bank")
        oracle = containers.get("saltation/oracle")
        if not bank or not oracle or not bank["oracle_service"]:
            return None
        bank_networks = set(bank["network_ids"])
        routes = {
            (member, network_id)
            for member, container in containers.items()
            for network_id in bank_networks.intersection(container["oracle_alias_network_ids"])
        }
        if len(routes) != 1:
            return None
        member, network_id = routes.pop()
        if member != "saltation/oracle" or network_id not in oracle["network_ids"]:
            return None
        # Inspect each attached network so an untracked container cannot claim
        # the oracle alias or provide a competing DNS route from the bank.
        known_ids = {
            container["id"] for member, container in containers.items() if member.startswith("saltation/")
        }
        try:
            for attached in sorted(bank_networks):
                network = await self._get(f"/networks/{attached}")
                labels = network["Labels"] or {}
                members = set(network["Containers"])
                if (
                    network["Id"] != attached
                    or labels.get(PROJECT_LABEL) != self.project
                    or not members <= known_ids
                    or bank["id"] not in members
                    or (
                        attached == network_id
                        and (labels.get(NETWORK_LABEL) != "data" or oracle["id"] not in members)
                    )
                ):
                    return None
        except (httpx.HTTPError, OSError, KeyError, TypeError, ValueError, AttributeError):
            return None
        return network_id

    def bind_targets(
        self,
        *,
        bank_url: str,
        metrics_url: str,
        oracle_dsn: str,
        oracle_user: str | None = None,
        revision: str | None = None,
        oci_shape: str | None = None,
    ) -> bool:
        """Bind exact host-side qualification targets to the inspected project.

        Call before any load or database reads. The caller must use these same
        endpoints directly, with HTTP proxies and redirects disabled.
        """
        if not all(isinstance(value, str) for value in (bank_url, metrics_url, oracle_dsn)):
            self._gap("Qualification requires explicit bank, metrics, and Oracle targets.")
            return False
        bank = HTTP_TARGET.fullmatch(bank_url)
        metrics = HTTP_TARGET.fullmatch(metrics_url)
        oracle = ORACLE_TARGET.fullmatch(oracle_dsn)
        if not bank or not metrics or not oracle:
            self._gap(
                "Qualification requires numeric IPv4 loopback origins and a loopback Oracle Easy Connect DSN."
            )
            return False
        bank_port, metrics_port, oracle_port = (_port(match[1]) for match in (bank, metrics, oracle))
        if (
            not bank_port
            or not metrics_port
            or not oracle_port
            or len({bank_port, metrics_port, oracle_port}) != 3
        ):
            self._gap("Qualification target ports are invalid or ambiguous.")
            return False
        user = _matching(
            oracle_user if oracle_user is not None else os.environ.get("SALTATION_DB_USER"), ORACLE_USER
        )
        if user is None:
            self._gap("Qualification requires the inspected bank's Oracle schema user.")
            return False
        expected_revision = _matching(revision, REVISION)
        if revision is not None and expected_revision is None:
            self._gap("Qualification requires a full 40-character Git revision.")
            return False
        shape_input = oci_shape if oci_shape is not None else os.environ.get("SALTATION_VERIFY_OCI_SHAPE")
        shape = _matching(shape_input, NAME)
        if shape_input is not None and shape is None:
            self._gap("Operator-provided OCI shape is invalid.")
            return False
        targets = (bank_port, metrics_port, oracle_port, oracle[2].upper(), user.upper())
        if self._closed or (
            self._target_inputs is not None
            and (targets, expected_revision, shape)
            != (self._target_inputs, self._expected_revision, self._oci_shape)
        ):
            self._gap("Qualification targets cannot be rebound.")
            return False
        self._target_inputs = targets
        self._expected_revision = expected_revision
        self._oci_shape = shape
        return self._validate_binding(self.before or {}, "before")

    def _validate_binding(self, snapshot: Record, phase: str) -> bool:
        if self._target_inputs is None:
            return False
        bank_port, metrics_port, oracle_port, service, user = self._target_inputs
        containers = snapshot.get("containers", {})
        bank = containers.get("saltation/bank")
        oracle = containers.get("saltation/oracle")
        network_id = snapshot.get("oracle_data_network_id")
        if not bank or not oracle or not network_id or bank["oracle_service"] != service:
            self._gap(f"{phase}: Oracle target is not linked to the inspected bank via Compose DNS.")
            return False
        if self._expected_revision is not None and (
            bank["image_revision"] != self._expected_revision
            or not bank["container_revision_label_valid"]
            or bank["container_image_revision"] not in {None, self._expected_revision}
        ):
            self._gap(f"{phase}: inspected bank image revision is missing or does not match qualification.")
            return False
        if bank["oracle_user"] != user:
            self._gap(f"{phase}: Oracle proof schema user differs from the inspected bank configuration.")
            return False
        targets = (
            (bank, bank["listeners"]["bank"], bank_port),
            (bank, bank["listeners"]["metrics"], metrics_port),
            (oracle, 1521, oracle_port),
        )
        bank_listeners = bank["listeners"]
        if bank_listeners["bank"] == bank_listeners["metrics"] or any(
            private is None or public not in container["loopback_tcp_ports"].get(f"{private}/tcp", ())
            for container, private, public in targets
        ):
            self._gap(
                f"{phase}: qualification target is not published by its inspected listener on loopback."
            )
            return False
        binding = {
            "bank_id": bank["id"],
            "bank_image_revision": bank["image_revision"],
            "expected_revision": self._expected_revision,
            "oracle_id": oracle["id"],
            "oracle_data_network_id": network_id,
            "bank_network_ids": bank["network_ids"],
            "oracle_service": service,
            "bank_host_port": bank_port,
            "metrics_host_port": metrics_port,
            "oracle_host_port": oracle_port,
        }
        if self._target_binding is None:
            self._target_binding = binding
        elif binding != self._target_binding:
            self._gap(f"{phase}: inspected qualification target linkage changed.")
            return False
        return True

    def _inventory_row(self, row: Record, candidates: dict[str, list[tuple[str, str]]], phase: str) -> None:
        labels = row["Labels"] or {}
        names = row["Names"]
        if not isinstance(names, list) or not all(isinstance(name, str) for name in names):
            raise ValueError("invalid Docker inventory names")
        names = [name.removeprefix("/") for name in names]
        if labels.get(PROJECT_LABEL) != self.project and not CO_TENANTS.intersection(names):
            if any(name.startswith(f"{self.project}-") for name in names):
                self._gap(f"{phase}: Docker container uses the project name without its Compose label.")
            return
        if len(names) != 1 or _matching(names[0], NAME) is None:
            self._gap(f"{phase}: ambiguous name on a Docker project or co-tenant container.")
            return
        name = names[0]
        assert self.project is not None
        membership = _membership(name, labels, self.project)
        container_id = _matching(row["Id"], ID)
        if membership is None or container_id is None:
            self._gap(f"{phase}: unknown Docker project membership or container identity.")
            return
        if membership in OPTIONAL_MEMBERS and row.get("State") in {
            "created",
            "exited",
            "dead",
        }:
            return
        candidates.setdefault(membership, []).append((container_id, name))

    def _check_snapshot(self, snapshot: Record, phase: str, *, complete: bool) -> None:
        cpu = Fraction()
        memory = 0
        cpu_complete = memory_complete = complete
        for membership, container in snapshot["containers"].items():
            if not container["running"]:
                self.violations.append(f"{phase}: {membership} is not running.")
            if container["oom_killed"]:
                self.violations.append(f"{phase}: {membership} has OOMKilled set.")
            if container["health"] not in {"none", "healthy"}:
                self.violations.append(f"{phase}: {membership} health is {container['health']}.")
            if membership.startswith("saltation/"):
                if container["cpu_limit_ratio"] is None:
                    self.violations.append(f"{phase}: {membership} has no hard CPU limit.")
                    cpu_complete = False
                else:
                    cpu += Fraction(*container["cpu_limit_ratio"])
                if container["memory_limit_bytes"] <= 0:
                    self.violations.append(f"{phase}: {membership} has no hard memory limit.")
                    memory_complete = False
                else:
                    memory += container["memory_limit_bytes"]
        if cpu_complete:
            snapshot["cpu_limit_vcpu"] = float(cpu)
        if memory_complete:
            snapshot["memory_limit_bytes"] = memory
        if cpu > CPU_BUDGET:
            self.violations.append(
                f"{phase}: Saltation CPU limits total {float(cpu):g} vCPU (> {CPU_BUDGET})."
            )
        if memory > MEMORY_BUDGET:
            self.violations.append(f"{phase}: Saltation memory limits total {memory} bytes (> 14 GiB).")

    def _compare(self, snapshot: Record | None = None) -> dict[str, list[str]]:
        ids: dict[str, list[str]] = {}
        before = (self.before or {}).get("containers", {})
        after = (snapshot or self.after or {}).get("containers", {})
        for inventory in (before, after):
            for membership, container in inventory.items():
                names = ids.setdefault(container["id"], [])
                if membership not in names:
                    names.append(membership)
        for membership in sorted(before.keys() | after.keys()):
            first, last = before.get(membership), after.get(membership)
            if first is None or last is None:
                self._gap(f"{membership} was not identified in both Docker snapshots.")
                continue
            if first["id"] != last["id"]:
                self.violations.append(f"{membership} was replaced during the run.")
                continue
            for field in ("name", "started_at", "restart_count", "health"):
                if first[field] != last[field]:
                    self.violations.append(f"{membership} changed {field} during the run.")
            if membership.startswith("saltation/"):
                for field in ("cpu_limit_ratio", "memory_limit_bytes", "image_id", "compose_config_hash"):
                    if first[field] != last[field]:
                        self._gap(f"{membership} changed {field} during the run.")
        return ids

    def _event_evidence(self, ids: dict[str, list[str]]) -> list[Record]:
        evidence = []
        bound_networks = (self._target_binding or {}).get("bank_network_ids", ())
        for event in sorted(self._events.values(), key=lambda event: event["time_ns"]):
            if self._since is None or event["time_ns"] < self._since:
                continue
            if event["action"].startswith("network "):
                if event["id"] in bound_networks:
                    evidence.append(event)
                    self._gap("Docker network membership changed on an inspected bank network.")
                continue
            labels = {PROJECT_LABEL: event["project"], SERVICE_LABEL: event["service"]}
            assert self.project is not None
            membership = _membership(event["name"], labels, self.project)
            matches = set(ids.get(event["id"], ()))
            if membership:
                matches.add(membership)
            concerns_project = event["project"] == self.project or (event["name"] or "").startswith(
                f"{self.project}-"
            )
            if not matches and not concerns_project and event["name"] is not None:
                continue
            identity = {key: event[key] for key in ("id", "time_ns", "action", "name")}
            if len(matches) != 1:
                self._gap("Docker event has unknown or ambiguous project membership.")
                evidence.append(identity)
                continue

            membership = matches.pop()
            evidence.append({**identity, "membership": membership})
            action = event["action"]
            if action in LIFECYCLE_EVENTS:
                self.violations.append(f"{membership} emitted Docker {action} during the run.")
            elif action == "update" and membership.startswith("saltation/"):
                self._gap(f"{membership} had a Docker update; continuous quotas could not be proven.")
            elif action.startswith("health_status"):
                status = action.removeprefix("health_status: ")
                baseline = (self.before or {}).get("containers", {}).get(membership, {}).get("health")
                if status == "unknown":
                    self._gap(f"{membership} emitted an unrecognized Docker health event.")
                elif status != baseline or status in {"unhealthy", "starting"}:
                    self.violations.append(f"{membership} emitted Docker {action} during the run.")
        return evidence

    async def finish(self) -> Record:
        """Capture final Docker state and release the subscription and HTTP client."""
        if self._result is not None:
            return self._result
        try:
            if self._closed:
                self._gap("Docker verification was closed before the final snapshot.")
            elif self._since is not None:
                await self._cancel_audit()
                self.after = await self._snapshot("after")
                self._validate_binding(self.after, "after")
                await self._checkpoint()
            events = self._event_evidence(self._compare())
        finally:
            await self.close()

        self._result = {
            "violations": sorted(set(self.violations)),
            "incomplete": sorted(set(self.incomplete)),
            "before": self.before,
            "after": self.after,
            "events": events,
            "target_binding": self._target_binding,
            "provenance": {
                "source": "docker_engine_unix_socket",
                "project": self.project,
                "daemon_id": self._daemon_id,
                "architecture": self._architecture or "unknown",
                "oci_shape": self._oci_shape or "unknown",
                "oci_shape_source": "operator" if self._oci_shape else "unknown",
                "since_ns": self._since,
                "audited_until_ns": self._audited_until,
                "history_checkpoints": self._checkpoints,
                "periodic_state_checks": self._state_checks,
                "live_watermark_ns": self._live_watermark,
                "cpu_budget_vcpu": CPU_BUDGET,
                "memory_budget_bytes": MEMORY_BUDGET,
            },
        }
        return self._result

    async def _cancel_audit(self) -> None:
        if self._audit is not None:
            self._audit.cancel()
            # Suppress cancellation of the child while preserving cancellation of the caller.
            (result,) = await asyncio.gather(self._audit, return_exceptions=True)
            if isinstance(result, Exception):
                self._gap("Docker periodic state and history audit stopped unexpectedly.")
            self._audit = None

    async def close(self) -> None:
        """Release resources when the caller cannot reach finish."""
        if self._closed:
            return
        self._closed = True
        tasks = [task for task in (self._audit, self._live) if task is not None]
        for task in tasks:
            task.cancel()
        try:
            results = await asyncio.gather(*tasks, return_exceptions=True)
            if any(isinstance(result, Exception) for result in results):
                self._gap("Docker verification background collection stopped unexpectedly.")
        finally:
            if self._client is not None:
                await self._client.aclose()


def _pressure_total(path: str) -> int | None:
    try:
        row = next(line for line in Path(path).read_text().splitlines() if line.startswith("some "))
        values = dict(field.split("=") for field in row.split()[1:])
        return int(values["total"])
    except (OSError, StopIteration, ValueError, KeyError):
        return None


def _host_cpu_ticks() -> tuple[int, int, int] | None:
    try:
        fields = Path("/proc/stat").read_text().splitlines()[0].split()
        if fields[0] == "cpu" and len(fields) >= 9:
            user, nice, system, idle, iowait, irq, softirq, steal = map(int, fields[1:9])
            busy = user + nice + system + irq + softirq + steal
            return busy + idle + iowait, busy, iowait
    except (OSError, IndexError, ValueError):
        pass
    return None


def _cpu_throttle() -> tuple[str, int, int] | None:
    """Read throttling for the host caller's cgroup v2, when available."""
    try:
        row = next(
            line for line in Path("/proc/self/cgroup").read_text().splitlines() if line.startswith("0::")
        )
        path = row.removeprefix("0::")
        directory = Path(path)
        if directory.is_absolute() and ".." not in directory.parts:
            stat = Path("/sys/fs/cgroup") / path.lstrip("/") / "cpu.stat"
            counters = dict(line.split() for line in stat.read_text().splitlines())
            return path, int(counters["nr_throttled"]), int(counters["throttled_usec"])
    except (OSError, StopIteration, ValueError, KeyError):
        pass
    return None


def capture_resource() -> dict:
    """Take a synchronous Linux host sample at a step boundary."""
    try:
        boot_id = Path("/proc/sys/kernel/random/boot_id").read_text().strip()
    except OSError:
        boot_id = None
    total, busy, iowait = _host_cpu_ticks() or (None, None, None)
    cgroup, throttled, throttled_usec = _cpu_throttle() or (None, None, None)
    return {
        "source": "linux_procfs",
        "monotonic_ns": time.monotonic_ns(),
        "boot_id": boot_id,
        "cpu_total_ticks": total,
        "cpu_busy_ticks": busy,
        "cpu_iowait_ticks": iowait,
        "cpu_pressure_usec": _pressure_total("/proc/pressure/cpu"),
        "io_pressure_usec": _pressure_total("/proc/pressure/io"),
        "cpu_cgroup": cgroup,
        "cpu_throttled_periods": throttled,
        "cpu_throttled_usec": throttled_usec,
    }


def _counter_delta(before: dict, after: dict, name: str) -> int | None:
    first, last = before.get(name), after.get(name)
    if type(first) is int and type(last) is int and 0 <= first <= last:
        return last - first
    return None


def classify_resource(before: dict, after: dict) -> str:
    """Classify a positive, same-host step interval using saturation signals."""
    if (
        before.get("source") != "linux_procfs"
        or after.get("source") != "linux_procfs"
        or not before.get("boot_id")
        or before["boot_id"] != after.get("boot_id")
    ):
        return "unknown"
    elapsed = _counter_delta(before, after, "monotonic_ns")
    total = _counter_delta(before, after, "cpu_total_ticks")
    busy = _counter_delta(before, after, "cpu_busy_ticks")
    iowait = _counter_delta(before, after, "cpu_iowait_ticks")
    cpu_pressure = _counter_delta(before, after, "cpu_pressure_usec")
    io_pressure = _counter_delta(before, after, "io_pressure_usec")
    if (
        not elapsed
        or not total
        or busy is None
        or iowait is None
        or busy + iowait > total
        or cpu_pressure is None
        or io_pressure is None
        or max(cpu_pressure, io_pressure) * 1000 > elapsed
    ):
        return "unknown"

    # Require sustained contention within this window. Cgroup throttling is
    # supporting evidence only for the host caller's own cgroup.
    throttled = _counter_delta(before, after, "cpu_throttled_periods")
    throttle_us = _counter_delta(before, after, "cpu_throttled_usec")
    same_cgroup = before.get("cpu_cgroup") and before["cpu_cgroup"] == after.get("cpu_cgroup")
    cpu = cpu_pressure * 10_000 >= elapsed and (
        busy * 10 >= total * 9
        or (same_cgroup and bool(throttled) and throttle_us is not None and throttle_us * 10_000 >= elapsed)
    )
    disk = io_pressure * 10_000 >= elapsed and iowait * 20 >= total
    if bool(cpu) == disk:
        return "unknown"
    return "cpu" if cpu else "disk"

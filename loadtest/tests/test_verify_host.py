from __future__ import annotations

import asyncio
import json

import httpx
import pytest

from saltation_loadtest import verify_host as host

PROJECT = "target"
BASE = 1_800_000_000_000_000_000
SECRET = "private-credential-should-not-be-in-evidence"
DATA_NETWORK = "d" * 64
CONFIG_HASH = "c" * 64
IMAGE = "sha256:" + "a" * 64
DEPLOYED_REVISION = "f" * 40


class LiveStream(httpx.AsyncByteStream):
    def __init__(self) -> None:
        self.items: asyncio.Queue[dict | None] = asyncio.Queue()
        self.closed = False

    async def __aiter__(self):
        while (event := await self.items.get()) is not None:
            yield (json.dumps(event) + "\n").encode()

    async def aclose(self) -> None:
        self.closed = True


class ReplayStream(httpx.AsyncByteStream):
    def __init__(self, entries):
        self.entries = entries

    async def __aiter__(self):
        for row in self.entries:
            yield (json.dumps(row) + "\n").encode()
        await asyncio.Future()


class Engine:
    def __init__(self) -> None:
        self.rows: dict[str, dict] = {}
        self.inspect: dict[str, dict] = {}
        self.clock = BASE
        self.history: list[dict] = []
        self.live = LiveStream()
        self.live_requested = False
        self.before_replay = None
        self.data_members = set()
        self.image_revision = DEPLOYED_REVISION
        for service in sorted(host.CORE_SERVICES):
            self.add(f"{PROJECT}-{service}-1", service)
        for name in sorted(host.CO_TENANTS):
            self.add(name)

    def add(self, name: str, service: str | None = None) -> str:
        container_id = f"{len(self.inspect) + 1:064x}"
        labels = {"private-label": SECRET}
        if service is not None:
            labels.update(
                {
                    host.PROJECT_LABEL: PROJECT,
                    host.SERVICE_LABEL: service,
                    host.CONFIG_HASH_LABEL: CONFIG_HASH,
                }
            )
        self.rows[container_id] = {
            "Id": container_id,
            "Names": [f"/{name}"],
            "Labels": labels,
            "State": "running",
        }
        self.inspect[container_id] = {
            "Id": container_id,
            "Name": f"/{name}",
            "Image": IMAGE,
            "RestartCount": 0,
            "Config": {"Labels": labels, "Env": [f"PASSWORD={SECRET}"]},
            "HostConfig": {"Memory": 1024**3, "NanoCpus": 300_000_000},
            "State": {
                "Running": True,
                "Status": "running",
                "OOMKilled": False,
                "StartedAt": "2026-01-01T00:00:00.123456789Z",
                "Health": {"Status": "healthy", "Log": [{"Output": SECRET}]} if service else None,
            },
        }
        return container_id

    def find(self, name: str) -> str:
        return next(container_id for container_id, row in self.rows.items() if row["Names"] == [f"/{name}"])

    def event(self, container_id: str, action: str, *, deliver: bool = True) -> None:
        row = self.rows[container_id]
        self.clock += 1
        event = {
            "Type": "container",
            "Action": action,
            "timeNano": self.clock,
            "Actor": {"ID": container_id, "Attributes": {**row["Labels"], "name": row["Names"][0][1:]}},
        }
        self.history.append(event)
        if deliver:
            self.live.items.put_nowait(event)

    def request(self, request: httpx.Request) -> httpx.Response:
        path = request.url.path
        if path == "/info":
            self.clock += 1_000
            return httpx.Response(
                200,
                json={
                    "ID": "daemon-1",
                    "OSType": "linux",
                    "Architecture": "aarch64",
                    "MemoryLimit": True,
                    "CpuCfsQuota": True,
                    "SystemTime": f"2027-01-15T08:00:{(self.clock - BASE) // 10**9:02d}."
                    f"{self.clock % 10**9:09d}Z",
                },
            )
        if path == "/containers/json":
            assert self.live_requested
            assert request.url.params["all"] == "1"
            return httpx.Response(200, json=list(self.rows.values()))
        if path.startswith("/containers/") and path.endswith("/json"):
            return httpx.Response(200, json=self.inspect[path.split("/")[2]])
        if path == f"/images/{IMAGE}/json":
            return httpx.Response(
                200,
                json={"Id": IMAGE, "Config": {"Labels": {host.REVISION_LABEL: self.image_revision}}},
            )
        if path == f"/networks/{DATA_NETWORK}":
            return httpx.Response(
                200,
                json={
                    "Id": DATA_NETWORK,
                    "Labels": {host.PROJECT_LABEL: PROJECT, host.NETWORK_LABEL: "data"},
                    "Containers": {member: {} for member in self.data_members},
                },
            )
        if path == "/events":
            assert "filters" not in request.url.params
            if request.url.params["since"] == "1":
                if "until" in request.url.params:
                    seconds, nanos = request.url.params["until"].split(".")
                    until = int(seconds) * 10**9 + int(nanos)
                    rows = [row for row in self.history[-256:] if row["timeNano"] <= until]
                    self.clock = max(until, self.clock)
                    return httpx.Response(200, text="".join(f"{json.dumps(row)}\n" for row in rows))
                if self.before_replay is not None:
                    callback, self.before_replay = self.before_replay, None
                    callback()
                return httpx.Response(200, stream=ReplayStream(self.history[-256:]))
            self.live_requested = True
            return httpx.Response(200, stream=self.live)
        raise AssertionError(f"Unexpected request: {request.url}")


@pytest.fixture
def engine(monkeypatch):
    engine = Engine()
    monkeypatch.setattr(
        host.httpx, "AsyncHTTPTransport", lambda **_kwargs: httpx.MockTransport(engine.request)
    )
    monkeypatch.setattr(host, "REPLAY_DRAIN_SECONDS", 0.01)
    return engine


async def test_complete_inspection_and_coverage(engine):
    bank = engine.find(f"{PROJECT}-bank-1")
    engine.inspect[bank]["HostConfig"] = {"Memory": 1024**3, "CpuQuota": 30_000, "CpuPeriod": 100_000}
    verifier = await host.HostVerifier.start(project=PROJECT)
    assert not verifier.violations
    assert not verifier.incomplete
    engine.event(bank, "health_status: healthy")
    result = await verifier.finish()

    assert result["violations"] == []
    assert result["incomplete"] == []
    for phase in ("before", "after"):
        assert len(result[phase]["containers"]) == 8
        assert result[phase]["cpu_limit_vcpu"] == 2.4
        assert result[phase]["memory_limit_bytes"] == 8 * 1024**3
    assert len(result["events"]) == 1
    assert result["events"][0]["id"] == bank
    assert result["provenance"]["daemon_id"] == "daemon-1"
    assert result["provenance"]["architecture"] == "aarch64"
    assert result["provenance"]["oci_shape"] == "unknown"
    assert result["provenance"]["history_checkpoints"] >= 2
    assert SECRET not in json.dumps(result)
    assert engine.live.closed
    assert await verifier.finish() is result


async def test_detects_transient_events_and_inspect_changes(engine):
    verifier = await host.HostVerifier.start(project=PROJECT)
    bank = engine.find(f"{PROJECT}-bank-1")
    oracle = engine.find(f"{PROJECT}-oracle-1")
    engine.event(bank, "oom", deliver=False)  # Recovered by history after a live-delivery omission.
    engine.event(bank, "health_status: unhealthy")
    engine.event(bank, "health_status: healthy")
    engine.event(oracle, "restart")
    engine.inspect[oracle]["RestartCount"] += 1
    removed = engine.find(f"{PROJECT}-mongo-1")
    engine.event(removed, "destroy")
    del engine.rows[removed]
    engine.event(engine.add(f"{PROJECT}-mongo-1", "mongo"), "create")

    extra = engine.add(f"{PROJECT}-generator-run-1", "generator")
    engine.event(extra, "start")
    del engine.rows[extra]
    result = await verifier.finish()
    assert result["incomplete"] == []
    assert any("bank emitted Docker oom" in line for line in result["violations"])
    assert any("bank emitted Docker health_status: unhealthy" in line for line in result["violations"])
    assert any("oracle changed restart_count" in line for line in result["violations"])
    assert any("mongo was replaced" in line for line in result["violations"])
    assert any("generator emitted Docker start" in line for line in result["violations"])


async def test_initial_unknown_missing_and_duplicate_inventory(engine):
    engine.add(f"{PROJECT}-unknown-1", "unknown")
    engine.add(f"{PROJECT}-bank-2", "bank")
    del engine.rows[engine.find(f"{PROJECT}-mongo-1")]
    verifier = await host.HostVerifier.start(project=PROJECT)
    result = await verifier.finish()
    assert {"before", "after"} == {line.partition(":")[0] for line in result["incomplete"]}
    assert any("unknown Docker project" in line for line in result["incomplete"])
    assert any("ambiguous Docker inventory for saltation/bank" in line for line in result["incomplete"])
    assert any("missing saltation/mongo" in line for line in result["incomplete"])
    assert result["before"]["cpu_limit_vcpu"] is None


async def test_unlimited_and_aggregate_limits(engine):
    oracle = engine.find(f"{PROJECT}-oracle-1")
    bank = engine.find(f"{PROJECT}-bank-1")
    engine.inspect[oracle]["HostConfig"].update(NanoCpus=3_000_000_000, Memory=host.MEMORY_BUDGET)
    engine.inspect[bank]["HostConfig"].update(NanoCpus=0, Memory=0, CpusetCpus="0")
    verifier = await host.HostVerifier.start(project=PROJECT)
    result = await verifier.finish()
    assert any("no hard CPU limit" in line for line in result["violations"])
    assert any("no hard memory limit" in line for line in result["violations"])
    assert any("total 4.8 vCPU" in line for line in result["violations"])
    assert any("14 GiB" in line for line in result["violations"])


async def test_limit_update_with_identical_endpoints_is_incomplete(engine):
    verifier = await host.HostVerifier.start(project=PROJECT)
    engine.event(engine.find(f"{PROJECT}-bank-1"), "update")
    result = await verifier.finish()
    assert not result["violations"]
    assert any("continuous quotas" in line for line in result["incomplete"])


async def test_lost_live_stream_and_truncated_history_are_incomplete(engine):
    verifier = await host.HostVerifier.start(project=PROJECT)
    engine.live.items.put_nowait(None)
    for _ in range(256):
        engine.clock += 1
        engine.history.append({"Type": "image", "timeNano": engine.clock})
    result = await verifier.finish()
    assert any("live event stream ended" in line for line in result["incomplete"])
    assert any("lost overlap" in line for line in result["incomplete"])
    assert engine.live.closed


async def test_full_history_with_independent_overlap_is_complete(engine):
    engine.history = [{"Type": "image", "timeNano": BASE - n} for n in range(300, 0, -1)]
    verifier = await host.HostVerifier.start(project=PROJECT)
    result = await verifier.finish()
    assert result["incomplete"] == []


async def test_close_releases_live_stream_on_caller_failure(engine):
    verifier = await host.HostVerifier.start(project=PROJECT)
    await verifier.close()
    await verifier.close()
    assert engine.live.closed
    result = await verifier.finish()
    assert any("closed before the final snapshot" in line for line in result["incomplete"])


async def test_explicit_project_is_required(monkeypatch):
    monkeypatch.delenv("SALTATION_VERIFY_PROJECT", raising=False)
    verifier = await host.HostVerifier.start()
    result = await verifier.finish()
    assert "SALTATION_VERIFY_PROJECT" in result["incomplete"][0]
    assert result["before"] is None


@pytest.mark.parametrize(
    ("busy", "iowait", "cpu_pressure", "io_pressure", "expected"),
    [
        (90, 0, 120_000, 0, "cpu"),
        (30, 25, 0, 110_000, "disk"),
        (90, 5, 120_000, 110_000, "unknown"),
        (40, 0, 1, 1, "unknown"),
        (90, 0, 120_000, None, "unknown"),
    ],
)
def test_resource_step_classification(busy, iowait, cpu_pressure, io_pressure, expected):
    before = {
        "source": "linux_procfs",
        "boot_id": "same-boot",
        "monotonic_ns": 1,
        "cpu_total_ticks": 0,
        "cpu_busy_ticks": 0,
        "cpu_iowait_ticks": 0,
        "cpu_pressure_usec": 0,
        "io_pressure_usec": 0,
    }
    after = {
        **before,
        "monotonic_ns": 1_000_000_001,
        "cpu_total_ticks": 100,
        "cpu_busy_ticks": busy,
        "cpu_iowait_ticks": iowait,
        "cpu_pressure_usec": cpu_pressure,
        "io_pressure_usec": io_pressure,
    }
    assert host.classify_resource(before, after) == expected
    assert host.classify_resource(before, {**after, "boot_id": "different-boot"}) == "unknown"


async def test_unknown_project_event_fails_coverage_without_exposing_labels(engine):
    verifier = await host.HostVerifier.start(project=PROJECT)
    unexpected = engine.add(f"{PROJECT}-unknown-1", SECRET)
    engine.event(unexpected, "create")
    del engine.rows[unexpected]

    result = await verifier.finish()
    assert any("unknown or ambiguous project membership" in line for line in result["incomplete"])
    assert SECRET not in json.dumps(result)


async def test_rejects_baseline_oom_and_inactive_container(engine):
    bank = engine.find(f"{PROJECT}-bank-1")
    engine.inspect[bank]["State"]["OOMKilled"] = True
    mongo = engine.find(f"{PROJECT}-mongo-1")
    engine.inspect[mongo]["State"].update(Running=False, Status="exited")
    verifier = await host.HostVerifier.start(project=PROJECT)
    try:
        assert any("bank has OOMKilled set" in line for line in verifier.violations)
        assert any("mongo is not running" in line for line in verifier.violations)
    finally:
        await verifier.close()


def test_cgroup_throttling_can_corroborate_host_cpu_pressure():
    before = {
        "source": "linux_procfs",
        "boot_id": "same-boot",
        "monotonic_ns": 1,
        "cpu_total_ticks": 0,
        "cpu_busy_ticks": 0,
        "cpu_iowait_ticks": 0,
        "cpu_pressure_usec": 0,
        "io_pressure_usec": 0,
        "cpu_cgroup": "/caller",
        "cpu_throttled_periods": 1,
        "cpu_throttled_usec": 10,
    }
    after = {
        **before,
        "monotonic_ns": 1_000_000_001,
        "cpu_total_ticks": 100,
        "cpu_busy_ticks": 55,
        "cpu_pressure_usec": 120_000,
        "cpu_throttled_periods": 3,
        "cpu_throttled_usec": 150_010,
    }
    assert host.classify_resource(before, after) == "cpu"
    assert host.classify_resource(before, {**after, "cpu_cgroup": "/other"}) == "unknown"


async def test_post_cutoff_churn_cannot_hide_history_eviction(engine):
    verifier = await host.HostVerifier.start(project=PROJECT)
    checkpoint_before = verifier._audited_until
    engine.event(engine.find(f"{PROJECT}-bank-1"), "oom", deliver=False)

    def evict_after_cutoff():
        # The daemon has already emitted the closing /info timestamp.
        for _ in range(256):
            engine.clock += 1
            engine.history.append({"Type": "image", "timeNano": engine.clock})

    engine.before_replay = evict_after_cutoff
    result = await verifier.finish()
    assert any("lost overlap" in message for message in result["incomplete"])
    assert result["provenance"]["audited_until_ns"] == checkpoint_before


async def test_disagreement_with_live_watermark_is_incomplete(engine):
    verifier = await host.HostVerifier.start(project=PROJECT)
    engine.event(engine.find(f"{PROJECT}-bank-1"), "oom")
    engine.history.clear()
    result = await verifier.finish()
    assert any("disagreed with the continuous live watermark" in message for message in result["incomplete"])
    assert result["provenance"]["live_watermark_ns"] is not None


async def test_periodic_state_check_detects_state_that_recovers_before_finish(engine, monkeypatch):
    monkeypatch.setattr(host, "CHECKPOINT_SECONDS", 0)
    inspected = asyncio.Event()
    original_snapshot = host.HostVerifier._snapshot

    async def notify_periodic(verifier, phase):
        snapshot = await original_snapshot(verifier, phase)
        if phase == "periodic":
            inspected.set()
        return snapshot

    monkeypatch.setattr(host.HostVerifier, "_snapshot", notify_periodic)
    verifier = await host.HostVerifier.start(project=PROJECT)
    bank_state = engine.inspect[engine.find(f"{PROJECT}-bank-1")]["State"]
    bank_state["Health"]["Status"] = "unhealthy"
    try:
        async with asyncio.timeout(1):
            await inspected.wait()
        bank_state["Health"]["Status"] = "healthy"
        result = await verifier.finish()
    finally:
        await verifier.close()
    assert any("periodic: saltation/bank health is unhealthy" in line for line in result["violations"])
    assert result["provenance"]["periodic_state_checks"] >= 1


def _published(port, ip="127.0.0.1"):
    return [{"HostIp": ip, "HostPort": str(port)}]


def _configure_targets(engine):
    bank = engine.find(f"{PROJECT}-bank-1")
    oracle = engine.find(f"{PROJECT}-oracle-1")
    engine.data_members.update({bank, oracle})
    engine.inspect[bank]["Config"]["Env"].extend(
        ["SALTATION_DB_CONNECT=oracle:1521/ORCLPDB1", "SALTATION_DB_USER=saltation"]
    )
    engine.inspect[bank]["NetworkSettings"] = {
        "Ports": {
            "8080/tcp": _published(18080),
            "8081/tcp": _published(18081),
            "9100/tcp": _published(19100),
        },
        "Networks": {"target_data": {"NetworkID": DATA_NETWORK, "Aliases": ["bank"]}},
    }
    engine.inspect[oracle]["NetworkSettings"] = {
        "Ports": {"1521/tcp": _published(11521)},
        "Networks": {"target_data": {"NetworkID": DATA_NETWORK, "Aliases": ["oracle"]}},
    }
    return bank, oracle


TARGETS = {
    "bank_url": "http://127.0.0.1:18081",
    "metrics_url": "http://127.0.0.1:19100",
    "oracle_dsn": "127.0.0.1:11521/ORCLPDB1",
    "oracle_user": "saltation",
}


async def test_compare_linked_targets_and_record_sanitized_digests(engine):
    bank, oracle = _configure_targets(engine)
    verifier = await host.HostVerifier.start(project=PROJECT)
    assert verifier.bind_targets(**TARGETS)
    result = await verifier.finish()

    assert result["incomplete"] == []
    assert result["target_binding"]["bank_id"] == bank
    assert result["target_binding"]["oracle_id"] == oracle
    assert result["target_binding"]["oracle_data_network_id"] == DATA_NETWORK
    assert result["before"]["containers"]["saltation/bank"]["image_id"] == IMAGE
    assert result["before"]["containers"]["saltation/oracle"]["compose_config_hash"] == CONFIG_HASH
    assert SECRET not in json.dumps(result)


@pytest.mark.parametrize(
    "overrides",
    [
        {"bank_url": "http://127.0.0.1:18080"},
        {"bank_url": "http://localhost:18081"},
        {"metrics_url": "http://127.0.0.1:18081"},
        {"oracle_dsn": "127.0.0.1:11521/OTHER"},
        {"oracle_dsn": "127.0.0.1:11522/ORCLPDB1"},
        {"oracle_user": "different_schema"},
        {"oracle_dsn": f"{SECRET}@127.0.0.1:11521/ORCLPDB1"},
    ],
)
async def test_reject_mismatched_or_noncanonical_targets(engine, overrides):
    _configure_targets(engine)
    verifier = await host.HostVerifier.start(project=PROJECT)
    assert not verifier.bind_targets(**(TARGETS | overrides))
    result = await verifier.finish()
    assert result["incomplete"]
    assert result["target_binding"] is None
    assert SECRET not in json.dumps(result)


async def test_reject_new_alias_or_unknown_container_on_bank_network(engine):
    _configure_targets(engine)
    verifier = await host.HostVerifier.start(project=PROJECT)
    assert verifier.bind_targets(**TARGETS)
    engine.data_members.add(engine.add("foreign"))
    result = await verifier.finish()
    assert any("Oracle target is not linked" in line for line in result["incomplete"])


async def test_reject_transient_bank_network_change(engine, monkeypatch):
    _configure_targets(engine)
    monkeypatch.setenv("SALTATION_DB_USER", "saltation")
    verifier = await host.HostVerifier.start(project=PROJECT)
    assert verifier.bind_targets(
        bank_url=TARGETS["bank_url"], metrics_url=TARGETS["metrics_url"], oracle_dsn=TARGETS["oracle_dsn"]
    )
    for action in ("connect", "disconnect"):
        engine.clock += 1
        engine.history.append(
            {
                "Type": "network",
                "Action": action,
                "Actor": {"ID": DATA_NETWORK, "Attributes": {"private-label": SECRET}},
                "timeNano": engine.clock,
            }
        )
    result = await verifier.finish()

    assert any("network membership changed" in line for line in result["incomplete"])
    assert [event["action"] for event in result["events"]] == ["network connect", "network disconnect"]
    assert SECRET not in json.dumps(result)


async def test_binds_deployed_bank_revision_and_operator_shape(engine):
    _configure_targets(engine)
    verifier = await host.HostVerifier.start(project=PROJECT)
    assert verifier.bind_targets(**TARGETS, revision=DEPLOYED_REVISION, oci_shape="VM.Standard.A1.Flex")
    result = await verifier.finish()

    assert result["incomplete"] == []
    assert result["target_binding"]["bank_image_revision"] == DEPLOYED_REVISION
    assert result["target_binding"]["expected_revision"] == DEPLOYED_REVISION
    assert result["provenance"]["oci_shape"] == "VM.Standard.A1.Flex"
    assert result["provenance"]["oci_shape_source"] == "operator"


@pytest.mark.parametrize(
    ("image_label", "container_label"),
    [(None, DEPLOYED_REVISION), ("e" * 40, DEPLOYED_REVISION), (DEPLOYED_REVISION, SECRET)],
)
async def test_revision_binding_requires_the_real_image_label(engine, image_label, container_label):
    bank, _ = _configure_targets(engine)
    engine.image_revision = image_label
    engine.inspect[bank]["Config"]["Labels"][host.REVISION_LABEL] = container_label
    verifier = await host.HostVerifier.start(project=PROJECT)
    assert not verifier.bind_targets(**TARGETS, revision=DEPLOYED_REVISION)
    result = await verifier.finish()

    assert any("bank image revision" in message for message in result["incomplete"])
    assert SECRET not in json.dumps(result)

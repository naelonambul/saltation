"""Check Saltation's OCI container budget without changing Docker state."""

import argparse
import json
import os
import re
import subprocess
import sys
from decimal import Decimal, InvalidOperation
from fractions import Fraction
from pathlib import Path
from typing import NamedTuple, Union


CPU_CAP = 4
MEMORY_CAP = 14_000_000_000
SERVER = ("oracle", "mongo", "bank", "web", "alloy", "loki", "prometheus", "grafana")
TOOLS = ("generator", "loadtest")
SERVICES = frozenset(SERVER + TOOLS)
ROOT = Path(__file__).resolve().parents[2]


class Limits(NamedTuple):
    cpu: Union[Decimal, Fraction]
    memory: int


CONNECTOR_RESERVATION = Limits(Decimal("0.15"), 128 * 1024**2)


class BudgetError(ValueError):
    pass


def total_limits(phase, members):
    total = Limits(
        sum(member.cpu for member in members),
        sum(member.memory for member in members),
    )
    if total.cpu > CPU_CAP or total.memory > MEMORY_CAP:
        raise BudgetError(
            f"{phase} exceeds budget: {total.cpu} vCPU (cap {CPU_CAP}), "
            f"{total.memory} bytes (cap {MEMORY_CAP})"
        )
    return total


def static_limits(name, service):
    deploy = service.get("deploy", {})
    if (deploy.get("mode", "replicated") != "replicated"
            or deploy.get("replicas", 1) != 1 or service.get("scale", 1) != 1):
        raise BudgetError(f"{name} must run exactly one replica")
    limits = deploy.get("resources", {}).get("limits", {})
    cpu_value = str(limits.get("cpus", service.get("cpus")))
    try:
        cpu = Decimal(cpu_value)
    except InvalidOperation:
        cpu = Decimal(0)
    if not cpu.is_finite() or cpu <= 0 or "_" in cpu_value:
        raise BudgetError(f"{name} needs a positive CPU limit")

    memory = str(limits.get("memory", service.get("mem_limit")))
    if not re.fullmatch(r"[0-9]+", memory) or int(memory) <= 0:
        raise BudgetError(f"{name} needs a positive memory limit in rendered bytes")
    return Limits(cpu, int(memory))


def static_matrix(model):
    services = model["services"]
    missing = SERVICES - services.keys()
    unknown = services.keys() - SERVICES
    if missing or unknown:
        raise BudgetError(
            f"rendered services differ from the phase matrix "
            f"(missing: {sorted(missing)}, unknown: {sorted(unknown)})"
        )

    limits = {}
    for name in sorted(SERVICES):
        service = services[name]
        profiles = service.get("profiles", [])
        if name in TOOLS:
            if profiles != ["tools"]:
                raise BudgetError(f"{name} must use the legacy tools profile exclusively")
        elif profiles and "local" not in profiles:
            raise BudgetError(f"{name} must be enabled in the local profile")
        if set(service.get("depends_on", {})).intersection(TOOLS):
            raise BudgetError(f"{name} must not start a tools service as a dependency")
        limits[name] = static_limits(name, service)

    base = [limits[name] for name in SERVER] + [CONNECTOR_RESERVATION]
    return {
        "server": total_limits("server", base),
        **{tool: total_limits(tool, base + [limits[tool]]) for tool in TOOLS},
    }


def docker_output(operation, *args, env=None):
    # Keep Compose interpolation output and Docker diagnostics out of reports.
    try:
        result = subprocess.run(
            ["docker", *args], cwd=ROOT, env=env, capture_output=True,
            text=True, timeout=30, check=False,
        )
    except (OSError, subprocess.TimeoutExpired) as error:
        raise BudgetError(f"{operation}: Docker failed ({type(error).__name__})") from error
    if result.returncode:
        raise BudgetError(f"{operation}: Docker exited with status {result.returncode}")
    return result.stdout


def render_compose():
    # Pass only CLI discovery settings so local secrets and COMPOSE_* settings
    # cannot override the checked-in example environment or profile selection.
    env = {key: os.environ[key] for key in ("HOME", "PATH", "DOCKER_CONFIG")
           if key in os.environ}
    output = docker_output(
        "render Compose", "compose", "-f", "infra/compose/compose.yml",
        "--env-file", "infra/compose/.env.example", "--profile", "*",
        "config", "--format", "json", env=env,
    )
    return json.loads(output, parse_float=Decimal)


LABEL_PREFIX = "com.docker.compose."
# Docker formats the typed inspect response first. Its Go field names for
# these quotas differ from the JSON keys in unformatted inspect output.
INSPECT_FIELDS = (
    ".ID", ".State.Status",
    *(f'(index .Config.Labels "{LABEL_PREFIX}{label}")'
      for label in ("project", "service", "oneoff")),
    ".HostConfig.NanoCPUs", ".HostConfig.CPUQuota", ".HostConfig.CPUPeriod",
    ".HostConfig.Memory", ".State.Pid",
)
INSPECT_FORMAT = "[" + ",".join("{{json " + field + "}}" for field in INSPECT_FIELDS) + "]"


class Container(NamedTuple):
    id: str
    status: str
    project: str
    service: str
    oneoff: str
    nano_cpus: int
    cpu_quota: int
    cpu_period: int
    memory: int
    pid: int = 0

    def limits(self, member):
        cpu = Fraction(0)
        if self.nano_cpus > 0:
            if self.cpu_quota != 0 or self.cpu_period != 0:
                raise BudgetError(f"{member} has conflicting Docker CPU quota settings")
            cpu = Fraction(self.nano_cpus, 1_000_000_000)
        elif self.nano_cpus == 0 and self.cpu_quota > 0 and self.cpu_period >= 0:
            cpu = Fraction(self.cpu_quota, self.cpu_period or 100_000)
        if cpu <= 0:
            raise BudgetError(f"{member} needs a positive Docker CPU quota")
        if self.memory <= 0:
            raise BudgetError(f"{member} needs a positive Docker memory limit")
        return Limits(cpu, self.memory)


def inspect_containers(references):
    output = docker_output(
        "inspect containers", "container", "inspect", "--format", INSPECT_FORMAT,
        "--", *references,
    )
    containers = [Container(*json.loads(line)) for line in output.splitlines()]
    if len(containers) != len(references):
        raise BudgetError("Docker returned an incomplete container inventory")
    return containers


def runtime_inventory(project, connectors):
    output = docker_output(
        "list project containers", "container", "ls",
        "--no-trunc", "--format", '{"id":{{json .ID}},"project":{{json (.Label "com.docker.compose.project")}}}',
    )
    rows = [json.loads(line) for line in output.splitlines()]
    project_ids = []
    for row in rows:
        owner = row["project"]
        if owner == project:
            project_ids.append(row["id"])
        elif owner.startswith("saltation-") or owner == "saltation":
            raise BudgetError(f"other active Saltation Compose project: {owner!r}")
    project_ids.sort()
    info = json.loads(docker_output(
        "inspect Docker quota support", "info", "--format",
        "{{json .}}",
    ))
    if (info.get("OSType") != "linux" or info.get("CpuCfsQuota") is not True
            or info.get("MemoryLimit") is not True):
        raise BudgetError("Docker daemon did not confirm Linux CPU and memory enforcement")
    if not project_ids:
        raise BudgetError(f"no active containers found for Compose project {project!r}")
    containers = inspect_containers(project_ids + list(connectors))
    project_containers = containers[:len(project_ids)]
    if {container.id for container in project_containers} != set(project_ids):
        raise BudgetError("Docker project membership changed during inspection")
    return project_containers, containers[len(project_ids):]


def runtime_phase(project, containers, connectors, planned_tool=None):
    seen_ids = set()
    project_limits = {}
    external_limits = []
    for container in [*containers, *connectors]:
        if container.id in seen_ids:
            raise BudgetError("duplicate container in Docker inventory")
        seen_ids.add(container.id)
        if container.status not in {"running", "restarting", "paused"}:
            raise BudgetError("container inventory changed or an external connector is inactive")

    for container in containers:
        name = container.service
        if container.project != project or name not in SERVICES:
            raise BudgetError(f"unknown active project member {name!r}")
        if name in project_limits:
            raise BudgetError(f"duplicate active containers for {name}")
        if name in SERVER and container.oneoff.lower() == "true":
            raise BudgetError(f"{name} must be a regular Compose service")
        if name in TOOLS and container.oneoff.lower() != "true":
            raise BudgetError(f"{name} must be a Compose one-off")
        project_limits[name] = container.limits(name)

    active_tools = sorted(project_limits.keys() & set(TOOLS))
    if len(active_tools) > 1:
        raise BudgetError("generator and loadtest are active concurrently")
    for connector in connectors:
        if connector.project == project:
            raise BudgetError("an external connector belongs to the selected Compose project")
        external_limits.append(connector.limits(f"external connector {connector.id[:12]}"))

    if planned_tool:
        if active_tools:
            raise BudgetError("a tool is already active before the proposed launch")
        driver = render_compose()["services"][planned_tool]
        proposed = static_limits(planned_tool, driver)
        project_limits[planned_tool] = Limits(Fraction(proposed.cpu), proposed.memory)
    phase = planned_tool or (active_tools[0] if active_tools else "server")
    if project_limits.keys() & set(SERVER) != set(SERVER):
        phase = f"{phase} (partial inventory)"
    return phase, total_limits(phase, [*project_limits.values(), *external_limits])


def effective_cgroup(container):
    limits = container.limits(container.service or container.id)
    try:
        if type(container.pid) is not int or container.pid < 1:
            raise ValueError("missing Docker PID")
        lines = (Path("/proc") / str(container.pid) / "cgroup").read_text().splitlines()
        path, = [line[3:] for line in lines if line.startswith("0::/")]
        root = Path("/sys/fs/cgroup").resolve()
        group = (root / path.lstrip("/")).resolve()
        if root not in group.parents:
            raise ValueError("cgroup path outside controller")
        limit, period = (group / "cpu.max").read_text().strip().split()
        effective_cpu = Fraction(int(limit), int(period))
        memory = int((group / "memory.max").read_text().strip())
        cpuset = (group / "cpuset.cpus.effective").read_text().strip()
        if not re.fullmatch(r"[0-9,-]+", cpuset):
            raise ValueError("missing effective CPU set")
        if effective_cpu > limits.cpu or memory > limits.memory:
            raise ValueError("applied cgroup values exceed Docker settings")
    except (OSError, ValueError, ZeroDivisionError) as error:
        raise BudgetError(
            f"effective cgroup evidence unavailable or invalid for {container.service or container.id[:12]}"
        ) from error
    return effective_cpu, memory, cpuset


def parse_args(argv):
    parser = argparse.ArgumentParser(description=__doc__)
    subcommands = parser.add_subparsers(dest="mode", required=True)
    subcommands.add_parser("static", help="render the OCI phase matrix with one connector reserved")
    runtime = subcommands.add_parser(
        "runtime",
        help="inspect active limits on the current Docker context",
        description="Audit active containers in one Compose project and all declared external connectors.",
        epilog="Run against the OCI Docker daemon. This is a read-only snapshot. "
               "Supply the full external connector inventory for a host-wide Saltation check.",
    )
    runtime.add_argument("--plan-driver", choices=TOOLS, help="include proposed single tool in the preflight")
    runtime.add_argument("--effective", action="store_true", help="read cgroup v2 on the local Linux Docker host")
    runtime.add_argument("--project", default="saltation", help="Compose project (default: saltation)")
    external = runtime.add_mutually_exclusive_group(required=True)
    external.add_argument(
        "--external-connector", action="append", default=[], metavar="NAME_OR_ID",
        help="active out-of-project connector to inspect (repeat for each connector)",
    )
    external.add_argument(
        "--no-external-connectors", action="store_true",
        help="assert that no external Saltation connectors are active on this host",
    )
    args = parser.parse_args(argv)
    if args.mode == "runtime":
        if not re.fullmatch(r"[a-z0-9][a-z0-9_-]*", args.project):
            runtime.error("invalid Compose project name")
        if any(not re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9_.-]*", reference)
               for reference in args.external_connector):
            runtime.error("invalid external connector name or ID")
        if len(set(args.external_connector)) != len(args.external_connector):
            runtime.error("duplicate external connector reference")
    return args


def main(argv=None):
    args = parse_args(argv)
    try:
        if args.mode == "static":
            results = static_matrix(render_compose())
        else:
            containers, connectors = runtime_inventory(args.project, args.external_connector)
            phase, total = runtime_phase(args.project, containers, connectors, args.plan_driver)
            if "partial inventory" in phase:
                raise BudgetError("partial inventory cannot establish a phase budget")
            if args.effective:
                for member in containers + connectors:
                    cpu, memory, cpuset = effective_cgroup(member)
                    print(f"{member.service or member.id[:12]}: effective {cpu} vCPU, {memory} bytes, CPUs {cpuset}")
            results = {phase: total}
    except BudgetError as error:
        print(f"Compose budget check failed: {error}", file=sys.stderr)
        return 1
    except (AttributeError, KeyError, TypeError, ValueError):
        print("Compose budget check failed: invalid Docker output", file=sys.stderr)
        return 1
    for phase, total in results.items():
        print(f"{phase}: {total.cpu} vCPU, {total.memory} bytes")
    return 0


if __name__ == "__main__":
    sys.exit(main())

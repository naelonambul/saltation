import io
import json
import unittest
from contextlib import redirect_stderr, redirect_stdout
from decimal import Decimal
from fractions import Fraction
from unittest.mock import patch

from scripts.ops import check_compose_budget as budget


MIB = 1024**2


def rendered_service(profile, cpu, memory):
    return {
        "profiles": [profile],
        "deploy": {"resources": {"limits": {"cpus": cpu, "memory": str(memory)}}},
    }


def container(name, *, id=None, project="saltation", oneoff="False", nano=1_000_000_000,
              quota=0, period=0, memory=MIB, status="running"):
    return budget.Container(id or name, status, project, name, oneoff, nano, quota, period, memory)


class StaticMatrixTests(unittest.TestCase):
    def setUp(self):
        self.services = {
            "oracle": rendered_service("local", "1.50", 6144 * MIB),
            "mongo": rendered_service("local", "0.40", 3072 * MIB),
            "bank": rendered_service("local", "0.75", 1024 * MIB),
            "web": rendered_service("local", "0.10", 128 * MIB),
            "alloy": rendered_service("local", "0.10", 256 * MIB),
            "loki": rendered_service("local", "0.10", 768 * MIB),
            "prometheus": rendered_service("local", "0.10", 768 * MIB),
            "grafana": rendered_service("local", "0.05", 256 * MIB),
            "generator": rendered_service("tools", "0.50", 768 * MIB),
            "loadtest": rendered_service("tools", "0.75", 768 * MIB),
        }

    def matrix(self):
        return budget.static_matrix({"services": self.services})

    def test_phase_totals_include_external_reservation(self):
        self.assertEqual(self.matrix(), {
            "server": (Decimal("3.25"), 12544 * MIB),
            "generator": (Decimal("3.75"), 13312 * MIB),
            "loadtest": (Decimal("4.00"), 13312 * MIB),
        })

    def test_both_caps_are_inclusive(self):
        exact_memory = 14_000_000_000 - 12544 * MIB
        self.services["loadtest"] = rendered_service("tools", "0.75", exact_memory)
        self.assertEqual(self.matrix()["loadtest"], (Decimal("4"), 14_000_000_000))
        for cpu, memory in (("0.750000001", exact_memory), ("0.75", exact_memory + 1)):
            with self.subTest(cpu=cpu, memory=memory):
                self.services["loadtest"] = rendered_service("tools", cpu, memory)
                with self.assertRaisesRegex(budget.BudgetError, "loadtest exceeds budget"):
                    self.matrix()

    def test_missing_invalid_or_unbounded_limits(self):
        limits = self.services["bank"]["deploy"]["resources"]["limits"]
        for key, values in (("cpus", [None, "NaN", "Infinity", "0", "-1", "abc", "0_1"]),
                            ("memory", [None, "0", "-1", "1.5", "8m"])):
            for value in values:
                with self.subTest(limit=key, value=value):
                    original = limits[key]
                    if value is None:
                        del limits[key]
                    else:
                        limits[key] = value
                    with self.assertRaisesRegex(budget.BudgetError, "bank needs a positive"):
                        self.matrix()
                    limits[key] = original

    def test_rejects_replicas_and_unknown_services(self):
        self.services["web"]["deploy"]["replicas"] = 2
        with self.assertRaisesRegex(budget.BudgetError, "web must run exactly one replica"):
            self.matrix()
        del self.services["web"]["deploy"]["replicas"]
        self.services["extra"] = rendered_service("tools", "0.05", MIB)
        with self.assertRaisesRegex(budget.BudgetError, "unknown: .*extra"):
            self.matrix()

    def test_rejects_tools_enabled_with_the_server_or_other_tool(self):
        self.services["generator"]["profiles"].append("local")
        with self.assertRaisesRegex(budget.BudgetError, "legacy tools profile exclusively"):
            self.matrix()
        self.services["generator"]["profiles"].remove("local")
        self.services["generator"]["depends_on"] = {"loadtest": {}}
        with self.assertRaisesRegex(budget.BudgetError, "must not start a tools service"):
            self.matrix()


class RuntimePhaseTests(unittest.TestCase):
    def phase(self, members, connectors=()):
        return budget.runtime_phase("saltation", members, connectors)

    def test_uses_actual_external_limits_and_accepts_exact_caps(self):
        bank = container("bank", nano=3_850_000_000, memory=14_000_000_000 - 128 * MIB)
        connector = container("edge", project="", nano=0, quota=15_000, memory=128 * MIB)
        self.assertEqual(self.phase([bank], [connector]), ("server (partial inventory)", (4, 14_000_000_000)))
        for change in ({"cpu_quota": 15_001}, {"memory": connector.memory + 1}):
            with self.subTest(change=change):
                with self.assertRaisesRegex(budget.BudgetError, r"server \(partial inventory\) exceeds budget"):
                    self.phase([bank], [connector._replace(**change)])

    def test_preserves_exact_quota_period_ratios(self):
        members = [
            container("bank", nano=0, quota=10_000, period=30_000),
            container("web", nano=0, quota=20_000, period=30_000),
            container("oracle", nano=3_000_000_000),
        ]
        self.assertEqual(self.phase(members), ("server (partial inventory)", (Fraction(4), 3 * MIB)))

    def test_rejects_missing_cpu_or_memory(self):
        for member in (container("oracle", nano=0), container("oracle", nano=0, quota=-1),
                       container("oracle", nano=0, period=100_000), container("oracle", memory=0)):
            with self.subTest(member=member):
                with self.assertRaisesRegex(budget.BudgetError, "oracle needs a positive Docker"):
                    self.phase([member])
        connector = container("edge", project="", nano=0)
        with self.assertRaisesRegex(budget.BudgetError, "external connector .* positive Docker"):
            self.phase([container("oracle")], [connector])

    def test_allows_one_tool_and_rejects_concurrent_or_duplicate_tools(self):
        generator = container("generator", oneoff="True")
        loadtest = container("loadtest", oneoff="True")
        bank = container("bank")
        self.assertEqual(self.phase([bank, generator])[0], "generator (partial inventory)")
        self.assertEqual(self.phase([bank, loadtest])[0], "loadtest (partial inventory)")
        with self.assertRaisesRegex(budget.BudgetError, "active concurrently"):
            self.phase([bank, generator, loadtest])
        with self.assertRaisesRegex(budget.BudgetError, "duplicate active containers for generator"):
            self.phase([bank, generator, generator._replace(id="generator-2")])
        with self.assertRaisesRegex(budget.BudgetError, "must be a Compose one-off"):
            self.phase([bank, loadtest._replace(oneoff="False")])

    def test_rejects_replicas_and_unknown_project_members(self):
        with self.assertRaisesRegex(budget.BudgetError, "duplicate active containers for oracle"):
            self.phase([container("oracle"), container("oracle", id="replica", status="paused")])
        with self.assertRaisesRegex(budget.BudgetError, "unknown active project member"):
            self.phase([container("unbudgeted")])

    def test_rejects_inactive_and_duplicate_external_connectors(self):
        bank = container("bank")
        connector = container("edge", project="")
        with self.assertRaisesRegex(budget.BudgetError, "external connector is inactive"):
            self.phase([bank], [connector._replace(status="exited")])
        with self.assertRaisesRegex(budget.BudgetError, "duplicate container in Docker inventory"):
            self.phase([bank], [connector, connector])


class RuntimeCliTests(unittest.TestCase):
    def test_inventory_declaration_and_inputs_are_validated(self):
        invalid = [
            ["runtime"],
            ["runtime", "--no-external-connectors", "--external-connector", "edge"],
            ["runtime", "--external-connector=--bad"],
            ["runtime", "--external-connector", "edge", "--external-connector", "edge"],
            ["runtime", "--no-external-connectors", "--project", ""],
        ]
        for argv in invalid:
            with self.subTest(argv=argv), redirect_stderr(io.StringIO()):
                with self.assertRaises(SystemExit) as error:
                    budget.main(argv)
                self.assertEqual(error.exception.code, 2)

    def test_runtime_lists_project_and_inspects_selected_fields_and_connectors(self):
        members = [container(name, project="oci", nano=400_000_000) for name in sorted(budget.SERVER)]
        edge = container("", project="", id="edge", nano=150_000_000, memory=128 * MIB)
        rows = "\n".join(json.dumps({"id": member.id, "project": "oci"}) for member in members)
        info = '{"OSType":"linux","CpuCfsQuota":true,"MemoryLimit":true}'
        inspection = "\n".join(map(json.dumps, [*members, edge]))
        with patch.object(budget, "docker_output", side_effect=[rows, info, inspection]) as docker:
            with redirect_stdout(io.StringIO()) as output:
                result = budget.main(["runtime", "--project", "oci", "--external-connector", "edge"])
        self.assertEqual(result, 0)
        self.assertIn("server: 67/20 vCPU, 142606336 bytes", output.getvalue())
        self.assertIn('Label "com.docker.compose.project"', docker.call_args_list[0].args[-1])
        inspect_args = docker.call_args_list[2].args
        self.assertEqual(inspect_args[-(len(members) + 2):], ("--", *(m.id for m in members), "edge"))
        for field in (".ID", ".HostConfig.NanoCPUs", ".HostConfig.CPUQuota",
                      ".HostConfig.CPUPeriod", ".HostConfig.Memory"):
            self.assertIn("{{json " + field + "}}", inspect_args[4])
        self.assertNotIn(".Config.Env", inspect_args[4])


    def test_preflight_accounts_for_actual_connector_and_proposed_load(self):
        members = [container(name, nano=390_000_000) for name in budget.SERVER]
        edge = container("edge", project="", nano=200_000_000)
        rendered = {"services": {"loadtest": rendered_service("tools", "0.75", MIB)}}
        with patch.object(budget, "render_compose", return_value=rendered):
            with self.assertRaisesRegex(budget.BudgetError, "exceeds budget"):
                budget.runtime_phase("saltation", members, [edge], planned_tool="loadtest")
            with self.assertRaisesRegex(budget.BudgetError, "already active"):
                budget.runtime_phase(
                    "saltation", [*members, container("generator", oneoff="true")], [edge],
                    planned_tool="loadtest",
                )


if __name__ == "__main__":
    unittest.main()

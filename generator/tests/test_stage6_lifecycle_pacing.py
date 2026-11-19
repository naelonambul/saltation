from dataclasses import replace
from datetime import datetime
from unittest.mock import patch

import pytest
from stage6_lifecycle_harness import fixture_pacing, fixture_steps, standard_steps

from saltation_gen.drivers import stage6, stage6_schedule
from saltation_gen.drivers.backfill_clock import KST
from saltation_gen.drivers.stage6_types import Episode, Step

AT = datetime(2026, 1, 15, 12, tzinfo=KST)
BC_ROLES = {"victim_1": "victim", "mule": "mule"}


def test_fixture_live_bc_offsets_and_cash_sessions():
    episode = Episode("fixture", "BC", AT, BC_ROLES, "live")
    baseline = standard_steps(episode)
    paced = fixture_steps(episode)
    assert [(step.name, step.offset) for step in paced] == [
        ("victim_1_login", 0),
        ("victim_1_open", 10),
        ("victim_1_identity", 45),
        ("mule_login", 50),
        ("victim_1_close", 55),
        ("victim_1_payee", 60),
        ("victim_1_transfer", 65),
        ("victim_1_withdraw_start", 70),
        ("victim_1_withdraw", 70),
        ("mule_settlement", 105),
        ("mule_inbound", 115),
        ("mule_withdraw_start", 125),
        ("mule_withdraw", 125),
    ]
    assert [replace(step, offset=old.offset) for step, old in zip(paced, baseline, strict=True)] == baseline


@pytest.mark.parametrize(
    ("kind", "window", "roles"),
    [("BC", "annual", BC_ROLES), ("D", "live", {"actor": "first", "second": "second"})],
)
def test_pacing_leaves_other_workloads_unchanged(kind, window, roles):
    episode = Episode("fixture", kind, AT, roles, window)
    assert fixture_steps(episode) == standard_steps(episode)


def test_both_step_expanders_are_restored_after_failure():
    episode = Episode("fixture", "BC", AT, BC_ROLES, "live")
    with pytest.raises(RuntimeError, match="fixture stopped"), fixture_pacing():
        assert stage6.steps(episode) == stage6_schedule.steps(episode) == fixture_steps(episode)
        assert stage6.steps(episode)[-1].offset == 125
        raise RuntimeError("fixture stopped")
    assert stage6.steps is stage6_schedule.steps is standard_steps


def test_out_of_window_pacing_is_rejected():
    episode = Episode("fixture", "BC", AT, BC_ROLES, "live")
    actions = [Step("late", "atm_withdraw", "mule", offset=394)]
    with (
        patch("stage6_lifecycle_harness.standard_steps", return_value=actions),
        pytest.raises(ValueError, match="exceeds the LIVE window"),
    ):
        fixture_steps(episode)

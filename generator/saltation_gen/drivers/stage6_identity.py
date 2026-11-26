import hashlib
import json
from collections import Counter
from dataclasses import asdict
from datetime import date, datetime

from saltation_gen.drivers.stage6_schedule import Schedule
from saltation_gen.drivers.stage6_types import Episode, Step

WORKLOAD_VERSION = 2


def _digest(value: object) -> str:
    encoded = json.dumps(value, sort_keys=True, separators=(",", ":"))
    return f"sha256:{hashlib.sha256(encoded.encode()).hexdigest()}"


def build_day_waves_digest(waves: list[list[tuple[Episode, Step, datetime]]]) -> str:
    return _digest(
        [
            [
                [episode.id, step.name, at.isoformat(), step.op, step.actor, step.peer, step.args]
                for episode, step, at in wave
            ]
            for wave in waves
        ]
    )


def build_schedule_identity(
    schedule: Schedule,
    planned: dict[date, list[tuple[Episode, Step, datetime]]],
    backfill_end: datetime,
) -> dict:
    items = []
    episodes = {"backfill": {}, "live": {}}
    operations = Counter()
    step_order = Counter()
    for day in sorted(planned):
        for episode, step, at in planned[day]:
            window = "live" if episode.window == "live" else "backfill"
            episodes[window][episode.id] = episode.kind
            operations[window] += 1
            items.append(
                (
                    at,
                    {
                        "at": at.isoformat(),
                        "episode": asdict(episode) | {"start": episode.start.isoformat()},
                        "step": asdict(step)
                        | {
                            "expect_declined": bool(step.args.get("expect_declined")),
                            "region": step.context.get("region"),
                            "device": step.context.get("device"),
                            "foreign_ip": step.context.get("ip"),
                        },
                        "step_order": step_order[episode.id],
                    },
                )
            )
            step_order[episode.id] += 1
    items.sort(
        key=lambda entry: (
            entry[0],
            entry[1]["episode"]["id"],
            entry[1]["step"]["name"],
            entry[1]["step_order"],
        )
    )
    schedule_fields = {"dormant": sorted(schedule.dormant), "profiles": list(schedule.profiles.items())}
    return {
        "workload_version": WORKLOAD_VERSION,
        "schedule_digest": _digest(schedule_fields | {"items": [item for _, item in items]}),
        "backfill_schedule_digest": _digest(
            schedule_fields | {"items": [item for at, item in items if at < backfill_end]}
        ),
        "planned_counts": {
            window: {
                "episodes": dict(sorted(Counter(kinds.values()).items())),
                "operations": operations[window],
            }
            for window, kinds in episodes.items()
        },
    }

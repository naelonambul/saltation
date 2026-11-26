import hashlib
import json
from collections import Counter
from dataclasses import asdict
from datetime import date, datetime

from saltation_gen.drivers.stage6_schedule import Schedule
from saltation_gen.drivers.stage6_types import Episode, Step

WORKLOAD_VERSION = 2


def _schedule_digest(dormant: list[str], items: list[dict]) -> str:
    encoded = json.dumps({"dormant": dormant, "items": items}, sort_keys=True, separators=(",", ":"))
    return f"sha256:{hashlib.sha256(encoded.encode()).hexdigest()}"


def build_schedule_identity(
    schedule: Schedule, planned: dict[date, list[tuple[Episode, Step, datetime]]]
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
                }
            )
            step_order[episode.id] += 1
    items.sort(key=lambda item: (item["at"], item["episode"]["id"], item["step"]["name"], item["step_order"]))
    dormant = sorted(schedule.dormant)
    return {
        "workload_version": WORKLOAD_VERSION,
        "schedule_digest": _schedule_digest(dormant, items),
        "backfill_schedule_digest": _schedule_digest(
            dormant, [item for item in items if item["episode"]["window"] != "live"]
        ),
        "planned_counts": {
            window: {
                "episodes": dict(sorted(Counter(kinds.values()).items())),
                "operations": operations[window],
            }
            for window, kinds in episodes.items()
        },
    }

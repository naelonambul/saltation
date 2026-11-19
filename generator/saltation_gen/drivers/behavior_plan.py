"""Fixed business-time schedule for the E/F and benign behavior proof."""

from dataclasses import dataclass
from datetime import date, datetime, time, timedelta

from saltation_gen.drivers.backfill_clock import KST

SUBJECTS = ("e", "f", "travel", "phone", "buyer", "house1", "house2", "receiver")
FIXTURE_START = 97_200_000
VERSION = 1
F_ACTIVE_HOURS = (8, 22)
LABELS = {"velocity": "E", "late_hour": "F", "background": "normal"}


@dataclass(frozen=True)
class Action:
    name: str
    at: datetime
    actor: str
    motif: str = "background"
    amount: int | None = None

    @property
    def parties(self) -> tuple[str, ...]:
        return (self.actor,) if self.amount is None else (self.actor, "receiver")


@dataclass(frozen=True)
class Reservation:
    motif: str
    start: datetime
    end: datetime
    parties: tuple[str, ...]


def instant(day: date, hour: int, minute: int, second: int = 0) -> datetime:
    return datetime.combine(day, time(hour, minute, second), KST)


def fixture_plan(day: date) -> tuple[list[Action], list[Reservation]]:
    next_day = day + timedelta(days=1)
    actions = [
        Action("e_baseline_login", instant(day, 9, 59), "e"),
        Action("e_baseline_transfer", instant(day, 10, 0), "e", amount=30_000),
        Action("f_baseline", instant(day, 10, 5), "f"),
        Action("travel_baseline", instant(day, 10, 6), "travel"),
        Action("phone_baseline", instant(day, 10, 7), "phone"),
        Action("buyer_baseline", instant(day, 10, 8), "buyer"),
        Action("f_anomaly", instant(next_day, 3, 0), "f", "late_hour"),
        Action("e_setup", instant(next_day, 9, 59), "e"),
        *[
            Action(f"e_{index + 1}", instant(next_day, 10, 0, index * 15), "e", "velocity", amount)
            for index, amount in enumerate((30_000, 30_000, 30_000, 240_000))
        ],
        Action("buyer_setup", instant(next_day, 10, 9), "buyer"),
        Action("purchase", instant(next_day, 10, 10), "buyer", "purchase", 240_000),
        Action("travel_login", instant(next_day, 11, 0), "travel", "travel"),
        Action("phone_login", instant(next_day, 12, 0), "phone", "new_phone"),
        Action("household_1", instant(next_day, 13, 0), "house1", "household"),
        Action("household_2", instant(next_day, 13, 1), "house2", "household"),
    ]
    reservations = [
        Reservation("late_hour", instant(next_day, 3, 0), instant(next_day, 3, 0), ("f",)),
        Reservation("velocity", instant(next_day, 10, 0), instant(next_day, 10, 0, 59), ("e", "receiver")),
        Reservation("purchase", instant(next_day, 10, 10), instant(next_day, 10, 10), ("buyer", "receiver")),
        Reservation("travel", instant(next_day, 11, 0), instant(next_day, 11, 0), ("travel",)),
        Reservation("new_phone", instant(next_day, 12, 0), instant(next_day, 12, 0), ("phone",)),
        Reservation("household", instant(next_day, 13, 0), instant(next_day, 13, 1), ("house1", "house2")),
    ]
    return actions, reservations


def validate_plan(actions: list[Action], reservations: list[Reservation]) -> None:
    if len({action.name for action in actions}) != len(actions):
        raise ValueError("fixture action names must be unique")
    instants = [action.at for action in actions]
    if instants != sorted(instants):
        raise ValueError("fixture clock order must be monotonic")
    for index, this in enumerate(reservations):
        for other in reservations[index + 1 :]:
            if this.start <= other.end and other.start <= this.end and set(this.parties) & set(other.parties):
                raise ValueError("fixture episode reservations overlap")
    for action in actions:
        if action.actor == "f":
            ordinary = F_ACTIVE_HOURS[0] <= action.at.astimezone(KST).hour < F_ACTIVE_HOURS[1]
            if ordinary == (action.motif == "late_hour"):
                raise ValueError("time anomaly does not match the holder's configured active hours")
        active = [
            reservation
            for reservation in reservations
            if reservation.start <= action.at <= reservation.end
            and set(action.parties) & set(reservation.parties)
        ]
        if action.motif == "background":
            if active:
                raise ValueError("normal activity overlaps a reserved episode")
        elif (
            len(active) != 1
            or active[0].motif != action.motif
            or not set(action.parties).issubset(active[0].parties)
        ):
            raise ValueError("fixture action does not match its reserved episode")

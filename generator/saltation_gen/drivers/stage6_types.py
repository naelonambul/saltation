"""Inputs shared by the stage 6 planner and operation executor."""

from dataclasses import dataclass, field
from datetime import datetime
from typing import Any


@dataclass(frozen=True)
class Episode:
    id: str
    kind: str
    start: datetime
    roles: dict[str, str]
    window: str
    carry_in: bool = False


@dataclass(frozen=True)
class Step:
    name: str
    op: str
    actor: str
    offset: int = 0
    args: dict[str, Any] = field(default_factory=dict)
    peer: str | None = None
    label: str | None = None
    peer_label: str | None = None
    context: dict[str, str] = field(default_factory=dict)

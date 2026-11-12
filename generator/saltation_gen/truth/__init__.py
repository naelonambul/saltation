"""Ground truth (spec F-34): labels per request and subject, the run manifest,
and the generator's roster of seeded customers. Lives in the MongoDB `truth`
database, which the bank cannot read."""

from .store import Label, TruthStore

__all__ = ["Label", "TruthStore"]

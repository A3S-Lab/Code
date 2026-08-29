"""Typed, non-sensitive memory maintenance health snapshots."""

from typing import List, Literal, Optional, TypedDict


MemoryMaintenancePhase = Literal[
    "disabled",
    "running",
    "degraded",
    "closing",
    "closed",
]


class MemoryMaintenanceJobHealth(TypedDict):
    name: str
    intervalMs: int
    workerAlive: bool
    runInProgress: bool
    successfulRuns: int
    failedRuns: int
    totalAffectedItems: int
    lastAffectedItems: Optional[int]
    lastError: Optional[str]


class MemoryMaintenanceHealth(TypedDict):
    phase: MemoryMaintenancePhase
    jobs: List[MemoryMaintenanceJobHealth]


__all__ = [
    "MemoryMaintenanceHealth",
    "MemoryMaintenanceJobHealth",
    "MemoryMaintenancePhase",
]

"""Per-user daily token budget tracking."""

from __future__ import annotations

import time
from collections import defaultdict
from dataclasses import dataclass, field


@dataclass
class _UserUsage:
    total_tokens: int = 0
    day_start: float = field(default_factory=time.monotonic)


class CostTracker:
    """Tracks per-user token usage with daily budgets."""

    def __init__(self, budget_per_user: int = 100_000, day_seconds: int = 86400) -> None:
        self._budget = budget_per_user
        self._day_seconds = day_seconds
        self._usage: dict[str, _UserUsage] = defaultdict(_UserUsage)

    def check_budget(self, user_id: str) -> tuple[bool, int]:
        """Check if user is within budget. Returns (within_budget, remaining_tokens)."""
        usage = self._get_or_reset(user_id)
        remaining = max(0, self._budget - usage.total_tokens)
        return remaining > 0, remaining

    def record_usage(self, user_id: str, tokens: int) -> bool:
        """Record token usage. Returns True if still within budget."""
        usage = self._get_or_reset(user_id)
        usage.total_tokens += tokens
        return usage.total_tokens <= self._budget

    def _get_or_reset(self, user_id: str) -> _UserUsage:
        usage = self._usage[user_id]
        now = time.monotonic()
        if now - usage.day_start > self._day_seconds:
            usage.total_tokens = 0
            usage.day_start = now
        return usage

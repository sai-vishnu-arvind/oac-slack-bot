"""Sliding window rate limiter."""

from __future__ import annotations

import time
from collections import defaultdict, deque


class RateLimiter:
    """Sliding window rate limiter for users and channels."""

    def __init__(
        self,
        user_limit: int = 20,
        channel_limit: int = 50,
        window_secs: int = 60,
    ) -> None:
        self._user_limit = user_limit
        self._channel_limit = channel_limit
        self._window_secs = window_secs
        self._user_timestamps: dict[str, deque[float]] = defaultdict(deque)
        self._channel_timestamps: dict[str, deque[float]] = defaultdict(deque)

    def check_user(self, user_id: str) -> bool:
        """Check and record a user request. Returns True if allowed."""
        return self._check(self._user_timestamps[user_id], self._user_limit)

    def check_channel(self, channel_id: str) -> bool:
        """Check and record a channel request. Returns True if allowed."""
        return self._check(self._channel_timestamps[channel_id], self._channel_limit)

    def _check(self, timestamps: deque[float], limit: int) -> bool:
        now = time.monotonic()
        cutoff = now - self._window_secs

        # Remove expired entries
        while timestamps and timestamps[0] < cutoff:
            timestamps.popleft()

        if len(timestamps) >= limit:
            return False

        timestamps.append(now)
        return True

    def cleanup(self) -> None:
        """Remove empty timestamp queues."""
        now = time.monotonic()
        cutoff = now - self._window_secs

        for store in (self._user_timestamps, self._channel_timestamps):
            empty_keys = []
            for key, timestamps in store.items():
                while timestamps and timestamps[0] < cutoff:
                    timestamps.popleft()
                if not timestamps:
                    empty_keys.append(key)
            for key in empty_keys:
                del store[key]

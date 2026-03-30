"""Channel whitelist guardrail."""

from __future__ import annotations


class ChannelWhitelist:
    """Optional channel whitelist. If no channels specified, all are allowed."""

    def __init__(self, channels: set[str] | None = None) -> None:
        self._channels = channels

    def is_allowed(self, channel_id: str) -> bool:
        if self._channels is None:
            return True
        return channel_id in self._channels

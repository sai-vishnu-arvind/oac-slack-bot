"""Session management with LRU eviction and TTL-based cleanup."""

from __future__ import annotations

from collections import OrderedDict, deque
from datetime import UTC, datetime

from oac_slack_bot.claude.types import Message

MAX_SESSION_MESSAGES = 50


class Session:
    """A conversation session for one Slack thread/DM."""

    __slots__ = ("messages", "plugin_name", "last_activity")

    def __init__(self) -> None:
        self.messages: deque[Message] = deque(maxlen=MAX_SESSION_MESSAGES)
        self.plugin_name: str | None = None
        self.last_activity: datetime = datetime.now(UTC)

    def push(self, msg: Message) -> None:
        self.messages.append(msg)
        self.last_activity = datetime.now(UTC)

    def messages_list(self) -> list[Message]:
        return list(self.messages)


class SessionStore:
    """LRU session store with TTL-based cleanup.

    Thread-safe when used with the asyncio.Lock wrapper in app.py.
    """

    def __init__(self, max_sessions: int = 500, ttl_secs: int = 1800) -> None:
        if max_sessions <= 0:
            raise ValueError("max_sessions must be positive")
        self._sessions: OrderedDict[str, Session] = OrderedDict()
        self._max_sessions = max_sessions
        self._ttl_secs = ttl_secs

    @staticmethod
    def key(channel: str, thread_ts: str | None = None) -> str:
        """Derive a session key from channel and optional thread_ts."""
        if thread_ts:
            return f"{channel}-{thread_ts}"
        if channel.startswith("D"):
            return f"dm-{channel}"
        return f"ch-{channel}"

    def get_or_create(self, key: str) -> Session:
        """Get existing session or create a new one."""
        if key in self._sessions:
            # Move to end (most recently used)
            self._sessions.move_to_end(key)
            return self._sessions[key]

        # Evict LRU if at capacity
        if len(self._sessions) >= self._max_sessions:
            self._sessions.popitem(last=False)

        session = Session()
        self._sessions[key] = session
        return session

    def get(self, key: str) -> Session | None:
        """Get an existing session, or None."""
        if key in self._sessions:
            self._sessions.move_to_end(key)
            return self._sessions[key]
        return None

    def cleanup(self) -> None:
        """Remove sessions idle longer than TTL."""
        now = datetime.now(UTC)
        stale = [
            k
            for k, s in self._sessions.items()
            if (now - s.last_activity).total_seconds() > self._ttl_secs
        ]
        for k in stale:
            del self._sessions[k]

    def __len__(self) -> int:
        return len(self._sessions)

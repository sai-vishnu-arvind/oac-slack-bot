"""In-memory metrics for the bot."""

from __future__ import annotations

import asyncio
import time
from collections import OrderedDict
from dataclasses import dataclass, field

import structlog

logger = structlog.get_logger()

MAX_BOT_MESSAGES = 2000


@dataclass
class BotMessageInfo:
    channel: str
    thread_ts: str
    plugin: str | None
    user: str | None
    question: str
    created_at: float = field(default_factory=time.monotonic)


class Metrics:
    """Thread-safe in-memory metrics store."""

    def __init__(self) -> None:
        self._lock = asyncio.Lock()

        # Counters
        self.total_mentions = 0
        self.total_errors = 0
        self.thumbs_up = 0
        self.thumbs_down = 0
        self.total_input_tokens = 0
        self.total_output_tokens = 0

        # Maps
        self._plugin_calls: dict[str, int] = {}
        self._user_calls: dict[str, int] = {}
        self._channel_calls: dict[str, int] = {}

        # Response times (ms)
        self._response_times_ms: list[int] = []
        self._first_token_times_ms: list[int] = []

        # Bot message tracking (LRU)
        self._bot_messages: OrderedDict[str, BotMessageInfo] = OrderedDict()

    async def record_mention(self, user: str | None, channel: str) -> None:
        async with self._lock:
            self.total_mentions += 1
            if user:
                self._user_calls[user] = self._user_calls.get(user, 0) + 1
            self._channel_calls[channel] = self._channel_calls.get(channel, 0) + 1

    def record_error(self) -> None:
        self.total_errors += 1

    async def record_plugin_call(self, plugin_fqn: str) -> None:
        async with self._lock:
            self._plugin_calls[plugin_fqn] = self._plugin_calls.get(plugin_fqn, 0) + 1

    async def record_response_time(self, duration_secs: float) -> None:
        async with self._lock:
            self._response_times_ms.append(int(duration_secs * 1000))

    async def record_first_token_time(self, duration_secs: float) -> None:
        async with self._lock:
            self._first_token_times_ms.append(int(duration_secs * 1000))

    def record_tokens(self, input_tokens: int, output_tokens: int) -> None:
        self.total_input_tokens += input_tokens
        self.total_output_tokens += output_tokens

    async def register_bot_message(
        self,
        channel: str,
        ts: str,
        thread_ts: str,
        plugin: str | None,
        user: str | None,
        question: str,
    ) -> None:
        async with self._lock:
            key = f"{channel}:{ts}"
            self._bot_messages[key] = BotMessageInfo(
                channel=channel,
                thread_ts=thread_ts,
                plugin=plugin,
                user=user,
                question=question,
            )
            # Evict LRU if over limit
            while len(self._bot_messages) > MAX_BOT_MESSAGES:
                self._bot_messages.popitem(last=False)

    async def record_reaction(
        self,
        channel: str,
        message_ts: str,
        reaction: str,
        user: str,
    ) -> BotMessageInfo | None:
        positive = reaction in ("+1", "thumbsup", "white_check_mark", "heavy_check_mark")
        negative = reaction in ("-1", "thumbsdown", "x")

        if not positive and not negative:
            return None

        key = f"{channel}:{message_ts}"
        async with self._lock:
            info = self._bot_messages.get(key)
            if info is None:
                return None

            if positive:
                self.thumbs_up += 1
            else:
                self.thumbs_down += 1

        logger.info(
            "feedback_received",
            reaction=reaction,
            sentiment="positive" if positive else "negative",
            user=user,
            channel=info.channel,
            thread_ts=info.thread_ts,
            plugin=info.plugin,
            response_age_secs=int(time.monotonic() - info.created_at),
        )
        return info

    async def format_stats(self) -> str:
        async with self._lock:
            mentions = self.total_mentions
            errors = self.total_errors
            up = self.thumbs_up
            down = self.thumbs_down
            input_tok = self.total_input_tokens
            output_tok = self.total_output_tokens
            error_pct = (errors / mentions * 100) if mentions > 0 else 0.0
            feedback_pct = (up / (up + down) * 100) if (up + down) > 0 else 0.0

            avg_rt, p95_rt = _percentiles(self._response_times_ms)
            avg_ft, _ = _percentiles(self._first_token_times_ms)
            top_users = _top_n(self._user_calls, 5)
            top_channels = _top_n(self._channel_calls, 5)
            top_plugins = _top_n(self._plugin_calls, 5)

        s = "📊 *Bot Stats*\n\n"
        s += "*Usage*\n"
        s += f"  Mentions: {mentions}\n"
        s += f"  Errors: {errors} ({error_pct:.1f}%)\n"
        s += f"  Avg response: {_format_ms(avg_rt)}\n"
        s += f"  P95 response: {_format_ms(p95_rt)}\n"
        s += f"  Avg first token: {_format_ms(avg_ft)}\n\n"

        s += "*Tokens*\n"
        s += f"  Input: {_format_number(input_tok)}\n"
        s += f"  Output: {_format_number(output_tok)}\n\n"

        s += "*Feedback*\n"
        s += f"  👍 {up}  👎 {down}  ({feedback_pct:.0f}% positive)\n\n"

        if top_users:
            s += "*Top Users*\n"
            for i, (name, count) in enumerate(top_users, 1):
                s += f"  {i}. <@{name}> — {count} requests\n"
            s += "\n"

        if top_channels:
            s += "*Top Channels*\n"
            for i, (name, count) in enumerate(top_channels, 1):
                s += f"  {i}. <#{name}> — {count} requests\n"
            s += "\n"

        if top_plugins:
            s += "*Top Plugins*\n"
            for i, (name, count) in enumerate(top_plugins, 1):
                s += f"  {i}. `{name}` — {count} calls\n"
            s += "\n"

        return s

    async def log_summary(self) -> None:
        async with self._lock:
            avg_rt, p95_rt = _percentiles(self._response_times_ms)
            top_plugins = _top_n(self._plugin_calls, 5)
            top_users = _top_n(self._user_calls, 3)

        logger.info(
            "metrics_summary",
            mentions=self.total_mentions,
            errors=self.total_errors,
            thumbs_up=self.thumbs_up,
            thumbs_down=self.thumbs_down,
            input_tokens=self.total_input_tokens,
            output_tokens=self.total_output_tokens,
            avg_response_ms=avg_rt,
            p95_response_ms=p95_rt,
            top_plugins=top_plugins,
            top_users=top_users,
        )


# ── Helpers ──


def _percentiles(values: list[int]) -> tuple[int, int]:
    if not values:
        return (0, 0)
    sorted_v = sorted(values)
    avg = sum(sorted_v) // len(sorted_v)
    p95_idx = int(len(sorted_v) * 0.95)
    p95 = sorted_v[min(p95_idx, len(sorted_v) - 1)]
    return (avg, p95)


def _top_n(mapping: dict[str, int], n: int) -> list[tuple[str, int]]:
    entries = sorted(mapping.items(), key=lambda x: x[1], reverse=True)
    return entries[:n]


def _format_ms(ms: int) -> str:
    if ms == 0:
        return "—"
    if ms < 1000:
        return f"{ms}ms"
    return f"{ms / 1000:.1f}s"


def _format_number(n: int) -> str:
    s = str(n)
    result: list[str] = []
    for i, c in enumerate(reversed(s)):
        if i > 0 and i % 3 == 0:
            result.append(",")
        result.append(c)
    return "".join(reversed(result))

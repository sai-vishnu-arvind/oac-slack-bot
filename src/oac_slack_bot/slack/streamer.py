"""Stream Claude SSE events into Slack with periodic message updates."""

from __future__ import annotations

import asyncio
import time

import structlog

from oac_slack_bot.claude.types import (
    DoneEvent,
    ErrorEvent,
    StreamEvent,
    TextEvent,
    ToolUseEvent,
    UsageEvent,
)
from oac_slack_bot.metrics import Metrics
from oac_slack_bot.slack.client import SlackClient
from oac_slack_bot.slack.format import md_to_slack

logger = structlog.get_logger()

UPDATE_INTERVAL_SECS = 0.8
MAX_SLACK_MSG_LEN = 3900


async def stream_to_slack(
    queue: asyncio.Queue[StreamEvent],
    client: SlackClient,
    channel: str,
    thread_ts: str,
    metrics: Metrics,
    user: str | None,
    question: str,
) -> str:
    """Stream events from queue into a Slack thread. Returns final text."""
    stream_start = time.monotonic()
    first_token_recorded = False

    # Post initial placeholder
    initial_resp = await client.post_message(channel, "⏳ Thinking...", thread_ts)
    if not initial_resp.ts:
        raise RuntimeError("post_message returned no ts")

    current_ts = initial_resp.ts

    await metrics.register_bot_message(
        channel, current_ts, thread_ts, None, user, question
    )

    full_text = ""
    last_sent_text = "⏳ Thinking..."
    done = False
    last_update_time = time.monotonic()

    while not done:
        # Try to drain events with a timeout for periodic updates
        try:
            event = await asyncio.wait_for(queue.get(), timeout=UPDATE_INTERVAL_SECS)

            if isinstance(event, TextEvent):
                if not first_token_recorded:
                    first_token_recorded = True
                    elapsed = time.monotonic() - stream_start
                    await metrics.record_first_token_time(elapsed)
                full_text += event.text

            elif isinstance(event, DoneEvent):
                done = True

            elif isinstance(event, ErrorEvent):
                error_text = f"❌ Error: {event.message}"
                await client.update_message(channel, current_ts, error_text)
                metrics.record_error()
                raise RuntimeError(event.message)

            elif isinstance(event, UsageEvent):
                metrics.record_tokens(event.input_tokens, event.output_tokens)

            elif isinstance(event, ToolUseEvent):
                pass  # Not expected in streaming path

        except TimeoutError:
            pass  # Tick — update Slack if needed

        # Periodic update check
        now = time.monotonic()
        should_update = (
            now - last_update_time >= UPDATE_INTERVAL_SECS
            and full_text
            and full_text != last_sent_text
        )
        if should_update:
            current_ts = await _flush_text(
                full_text, last_sent_text, client, channel, thread_ts,
                current_ts, metrics, user, question,
            )
            last_sent_text = full_text
            last_update_time = now

    # Final update
    final_text = full_text if full_text else "(no response)"
    if final_text != last_sent_text:
        await _flush_text(
            final_text, last_sent_text, client, channel, thread_ts,
            current_ts, metrics, user, question,
        )

    return full_text


async def _flush_text(
    text: str,
    last_sent: str,
    client: SlackClient,
    channel: str,
    thread_ts: str,
    current_ts: str,
    metrics: Metrics,
    user: str | None,
    question: str,
) -> str:
    """Update current Slack message; split if over limit. Returns current ts."""
    if text == last_sent:
        return current_ts

    slack_text = md_to_slack(text)

    if len(slack_text) <= MAX_SLACK_MSG_LEN:
        await client.update_message(channel, current_ts, slack_text)
        return current_ts

    # Split at whitespace boundary
    split_at = _find_split_point(slack_text, MAX_SLACK_MSG_LEN)
    first_part = slack_text[:split_at]
    remainder = slack_text[split_at:].lstrip()

    await client.update_message(channel, current_ts, first_part)

    resp = await client.post_message(channel, remainder, thread_ts)
    new_ts = resp.ts or current_ts

    await metrics.register_bot_message(
        channel, new_ts, thread_ts, None, user, question
    )

    return new_ts


def _find_split_point(text: str, max_len: int) -> int:
    """Find a good split point at or before max_len, preferring whitespace."""
    if max_len >= len(text):
        return len(text)

    # Find last whitespace at or before max_len
    search_region = text[:max_len]
    for i in range(len(search_region) - 1, 0, -1):
        if search_region[i].isspace():
            return i

    return max_len

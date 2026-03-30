"""Slack event handlers — mention and reaction processing."""

from __future__ import annotations

import asyncio
import os
import re
import time
from typing import TYPE_CHECKING

import httpx
import structlog

from oac_slack_bot.claude.session import SessionStore
from oac_slack_bot.claude.types import Message
from oac_slack_bot.metrics import Metrics
from oac_slack_bot.plugins.executor import execute_plugin
from oac_slack_bot.plugins.registry import PluginRegistry
from oac_slack_bot.plugins.router import route
from oac_slack_bot.slack.format import md_to_slack
from oac_slack_bot.slack.streamer import stream_to_slack
from oac_slack_bot.slack.types import SlackEvent, ThreadMessage

if TYPE_CHECKING:
    from oac_slack_bot.claude.client import ClaudeClient
    from oac_slack_bot.guardrails import GuardrailsPipeline
    from oac_slack_bot.slack.client import SlackClient

logger = structlog.get_logger()


async def handle_mention(
    event: SlackEvent,
    slack: SlackClient,
    claude: ClaudeClient,
    sessions: SessionStore,
    sessions_lock: asyncio.Lock,
    registry: PluginRegistry,
    metrics: Metrics,
    default_plugin: str | None,
    guardrails: GuardrailsPipeline | None = None,
) -> None:
    """Handle an app_mention event."""
    start = time.monotonic()

    channel = event.channel
    if not channel:
        return

    thread_ts = event.thread_ts or event.ts or ""
    user = event.user
    user_text = strip_mentions(event.text or "").strip()

    if not user_text:
        return

    await metrics.record_mention(user, channel)

    # Built-in commands
    if user_text.lower() == "stop":
        await slack.post_message(channel, "🛑 Stopped.", thread_ts)
        return

    if user_text.lower() == "stats":
        stats = await metrics.format_stats()
        await slack.post_message(channel, stats, thread_ts)
        return

    logger.info(
        "handling_app_mention",
        channel=channel,
        thread_ts=thread_ts,
        user=user,
        text=user_text[:200],
    )

    # Guardrails checkpoint
    if guardrails:
        result = await guardrails.check_input(user_text, user, channel)
        if not result.allowed:
            await slack.post_message(
                channel, f"⚠️ {result.reason}", thread_ts
            )
            return

    # Concurrent external resolution
    slack_resolved, devrev_resolved = await asyncio.gather(
        resolve_slack_urls(user_text, slack),
        resolve_devrev_tickets(user_text),
    )
    user_text = merge_resolved(user_text, slack_resolved, devrev_resolved)

    # Fetch thread history
    try:
        reply_resp = await slack.thread_history(channel, thread_ts)
        thread_msgs = reply_resp.messages or []
    except Exception as e:
        logger.warning("thread_history_failed", error=str(e))
        thread_msgs = []

    thread_context = build_thread_context(thread_msgs, exclude_ts=event.ts)

    # Session
    session_key = SessionStore.key(channel, event.thread_ts)
    async with sessions_lock:
        session = sessions.get_or_create(session_key)
        session.push(Message.user(user_text))

    # Route to plugin
    matched_fqn: str | None = default_plugin if default_plugin else None
    if not matched_fqn:
        matched = route(user_text, registry)
        matched_fqn = matched.fqn if matched else None

    if matched_fqn:
        # ── Plugin path ──
        plugin = registry.get(matched_fqn)
        if not plugin:
            logger.warning("plugin_not_found", plugin=matched_fqn)
            metrics.record_error()
            return

        await metrics.record_plugin_call(plugin.fqn)

        await slack.post_message(
            channel, f"🔍 Running plugin: `{plugin.fqn}`...", thread_ts
        )

        enriched_query = user_text
        if thread_context:
            enriched_query = (
                f"<thread_history>\n{thread_context}\n</thread_history>\n\n{user_text}"
            )

        try:
            reply_text = await execute_plugin(plugin, enriched_query)
            if not reply_text:
                reply_text = "(no response from plugin)"
            else:
                logger.info("plugin_response", len=len(reply_text))
        except Exception as e:
            logger.warning("plugin_execution_failed", error=str(e))
            metrics.record_error()
            reply_text = f"❌ Plugin error: {e}"

        # Filter output if guardrails enabled
        if guardrails:
            reply_text, warnings = guardrails.filter_output(reply_text)
            for w in warnings:
                logger.warning("output_filter_warning", warning=w)

        slack_text = md_to_slack(reply_text)
        post_resp = await slack.post_message(channel, slack_text, thread_ts)

        if post_resp.ts:
            await metrics.register_bot_message(
                channel, post_resp.ts, thread_ts,
                plugin.fqn, user, user_text,
            )

        elapsed = time.monotonic() - start
        await metrics.record_response_time(elapsed)

        async with sessions_lock:
            sessions.get_or_create(session_key).push(Message.assistant(reply_text))

    else:
        # ── Streaming path (no plugin) ──
        system = None
        if thread_context:
            system = (
                "Below is the conversation history from this Slack thread. "
                "Use it as context to understand the ongoing discussion and "
                "provide a relevant response to the latest message.\n\n"
                f"<thread_history>\n{thread_context}\n</thread_history>"
            )

        messages = [Message.user(user_text)]

        try:
            queue = await claude.stream(messages, system)
        except Exception as e:
            logger.warning("claude_stream_failed", error=str(e))
            metrics.record_error()
            await slack.post_message(channel, f"❌ Error: {e}", thread_ts)
            return

        try:
            final_text = await stream_to_slack(
                queue, slack, channel, thread_ts, metrics, user, user_text
            )
        except Exception as e:
            logger.warning("stream_to_slack_failed", error=str(e))
            metrics.record_error()
            return

        elapsed = time.monotonic() - start
        await metrics.record_response_time(elapsed)

        if final_text:
            async with sessions_lock:
                sessions.get_or_create(session_key).push(Message.assistant(final_text))


async def handle_reaction(event: SlackEvent, metrics: Metrics) -> None:
    """Handle a reaction_added event."""
    reaction = event.reaction
    item = event.item
    if not reaction or not item:
        return

    item_channel = item.channel
    item_ts = item.ts
    if not item_channel or not item_ts:
        return

    user = event.user or "unknown"
    await metrics.record_reaction(item_channel, item_ts, reaction, user)


# ── Helpers ──


def strip_mentions(text: str) -> str:
    """Remove <@BOT_ID> mention tokens from text."""
    result: list[str] = []
    chars = list(text)
    i = 0

    while i < len(chars):
        if chars[i] == "<" and i + 1 < len(chars) and chars[i + 1] == "@":
            # Consume until >
            while i < len(chars) and chars[i] != ">":
                i += 1
            i += 1  # skip >
            # Skip one trailing space
            if i < len(chars) and chars[i] == " ":
                i += 1
        else:
            result.append(chars[i])
            i += 1

    return "".join(result)


async def resolve_slack_urls(message: str, slack: SlackClient) -> str:
    """Fetch content from Slack thread URLs found in the message."""
    urls = extract_slack_urls(message)
    if not urls:
        return message

    async def fetch_thread(channel_id: str, thread_ts: str) -> str | None:
        logger.info("fetching_linked_thread", channel=channel_id, ts=thread_ts)
        try:
            resp = await slack.thread_history(channel_id, thread_ts)
            if not resp.messages:
                return None
            parts: list[str] = []
            for msg in resp.messages:
                user = msg.user or "unknown"
                text = msg.text or ""
                if text:
                    parts.append(f"[{user}]: {text}")
            if parts:
                return (
                    f"--- Slack thread (#{channel_id}, ts: {thread_ts}) ---\n"
                    + "\n".join(parts)
                    + "\n---"
                )
        except Exception as e:
            logger.warning("fetch_linked_thread_failed", channel=channel_id, error=str(e))
        return None

    results = await asyncio.gather(
        *(fetch_thread(ch, ts) for ch, ts in urls)
    )

    context_parts = [r for r in results if r]
    if not context_parts:
        return message

    return f"{chr(10).join(context_parts)}\n\nUser's question: {message}"


def extract_slack_urls(message: str) -> list[tuple[str, str]]:
    """Parse Slack thread URLs from a message."""
    results: list[tuple[str, str]] = []
    cleaned = message.replace("<", " ").replace(">", " ")

    for word in cleaned.split():
        if ".slack.com/archives/" not in word:
            continue

        parts = word.split("/")
        try:
            archives_idx = parts.index("archives")
        except ValueError:
            continue

        if archives_idx + 2 >= len(parts):
            continue

        channel_id = parts[archives_idx + 1]
        raw_ts = parts[archives_idx + 2].split("?")[0]

        if raw_ts.startswith("p") and len(raw_ts) >= 8:
            ts_digits = raw_ts[1:]
            dot_pos = len(ts_digits) - 6
            thread_ts = f"{ts_digits[:dot_pos]}.{ts_digits[dot_pos:]}"
            results.append((channel_id, thread_ts))

    return results


async def resolve_devrev_tickets(message: str) -> str:
    """Fetch DevRev ticket details for ticket IDs found in the message."""
    token = os.environ.get("DEVREV_TOKEN", "")
    if not token:
        return message

    ticket_ids = extract_devrev_ticket_ids(message)
    if not ticket_ids:
        return message

    http = httpx.AsyncClient(timeout=15)

    async def fetch_ticket(ticket_id: str) -> str | None:
        logger.info("fetching_devrev_ticket", ticket=ticket_id)
        try:
            resp = await http.get(
                "https://api.devrev.ai/works.get",
                headers={"Authorization": token},
                params={"id": ticket_id},
            )
            if resp.status_code != 200:
                return None
            body = resp.json()
            work = body.get("work", {})
            title = work.get("title", "No title")
            description = work.get("body", "No description")
            stage = (work.get("stage") or {}).get("name", "unknown")
            priority = work.get("priority", "unknown")
            created_by = (work.get("created_by") or {}).get("display_name", "unknown")
            return (
                f"--- DevRev Ticket {ticket_id} ---\n"
                f"Title: {title}\nStage: {stage}\nPriority: {priority}\n"
                f"Created by: {created_by}\nDescription:\n{description}\n---"
            )
        except Exception as e:
            logger.warning("devrev_fetch_failed", ticket=ticket_id, error=str(e))
            return None

    results = await asyncio.gather(*(fetch_ticket(tid) for tid in ticket_ids))
    await http.aclose()

    context_parts = [r for r in results if r]
    if not context_parts:
        return message

    return f"{chr(10).join(context_parts)}\n\nUser's question: {message}"


def extract_devrev_ticket_ids(message: str) -> list[str]:
    """Extract DevRev ticket IDs (TKT-xxx, ISS-xxx) from a message."""
    results: list[str] = []
    cleaned = message.replace("<", " ").replace(">", " ")

    for word in cleaned.split():
        upper = word.upper()
        stripped = upper.strip("".join(c for c in upper if not c.isalnum() and c != "-"))

        if (stripped.startswith("TKT-") or stripped.startswith("ISS-")) and len(stripped) > 4:
            results.append(stripped)
            continue

        if "devrev.ai" in word:
            for prefix in ("/works/", "/issue/", "/ticket/"):
                pos = word.find(prefix)
                if pos >= 0:
                    after = word[pos + len(prefix) :]
                    id_match = re.match(r"[A-Za-z0-9-]+", after)
                    if id_match:
                        tid = id_match.group().upper()
                        if (tid.startswith("TKT-") or tid.startswith("ISS-")) and len(tid) > 4:
                            results.append(tid)
                    break

    results = sorted(set(results))
    return results


def build_thread_context(
    messages: list[ThreadMessage], exclude_ts: str | None = None
) -> str:
    """Build context string from thread messages."""
    parts: list[str] = []

    for msg in messages:
        if exclude_ts and msg.ts == exclude_ts:
            continue

        text = (msg.text or "").strip()
        if not text:
            continue

        if _is_bot_indicator(text):
            continue

        sender = "assistant" if msg.bot_id else (msg.user or "unknown")
        parts.append(f"[{sender}]: {text}")

    return "\n".join(parts)


def merge_resolved(original: str, slack_resolved: str, devrev_resolved: str) -> str:
    """Merge results from concurrent URL and ticket resolution."""
    slack_changed = slack_resolved != original
    devrev_changed = devrev_resolved != original

    if not slack_changed and not devrev_changed:
        return original
    if slack_changed and not devrev_changed:
        return slack_resolved
    if not slack_changed and devrev_changed:
        return devrev_resolved

    # Both changed — combine contexts
    slack_context = slack_resolved.rsplit("\n\nUser's question: ", 1)[0]
    devrev_context = devrev_resolved.rsplit("\n\nUser's question: ", 1)[0]
    return f"{slack_context}\n\n{devrev_context}\n\nUser's question: {original}"


def _is_bot_indicator(text: str) -> bool:
    return (
        text.startswith("⏳")
        or text.startswith("🔍 Running plugin")
        or text == "🛑 Stopped."
        or text.startswith("❌ Error:")
        or text.startswith("❌ Plugin error:")
    )

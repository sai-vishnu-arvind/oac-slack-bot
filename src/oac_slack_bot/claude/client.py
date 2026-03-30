"""Claude API client with dual auth, SSE streaming, and concurrency control."""

from __future__ import annotations

import asyncio
import json
from typing import Any

import httpx
import structlog

from oac_slack_bot.claude.auth import GcpAuth, resolve_credentials_path
from oac_slack_bot.claude.types import (
    DoneEvent,
    ErrorEvent,
    Message,
    StreamEvent,
    TextEvent,
    Tool,
    ToolCall,
    ToolUseEvent,
    UsageEvent,
)
from oac_slack_bot.config import Config

logger = structlog.get_logger()

STREAM_QUEUE_SIZE = 64
MAX_CONCURRENT_REQUESTS = 10


class ClaudeClient:
    """Async Claude API client supporting both Vertex AI and Anthropic API."""

    def __init__(self, config: Config) -> None:
        self._http = httpx.AsyncClient(timeout=300)
        self._config = config
        credentials_path = config.google_application_credentials or resolve_credentials_path()
        self._auth = GcpAuth(credentials_path)
        self._semaphore = asyncio.Semaphore(MAX_CONCURRENT_REQUESTS)

    @property
    def _is_anthropic_mode(self) -> bool:
        return self._config.anthropic_base_url is not None

    async def stream(
        self,
        messages: list[Message],
        system_prompt: str | None = None,
        tools: list[Tool] | None = None,
    ) -> asyncio.Queue[StreamEvent]:
        """Start a streaming Claude request. Returns a queue of StreamEvents."""
        tool_count = len(tools) if tools else 0

        if self._is_anthropic_mode:
            base = self._config.anthropic_base_url or ""
            url = f"{base.rstrip('/')}/v1/messages"
            body = self._build_anthropic_body(messages, system_prompt, tools, stream=True)
        else:
            url = self._config.vertex_endpoint
            body = self._build_vertex_body(messages, system_prompt, tools, stream=True)

        last_msg_preview = ""
        if messages:
            last = messages[-1]
            if isinstance(last.content, str):
                last_msg_preview = last.content[:200]

        logger.info(
            "claude_request",
            url=url,
            msg_count=len(messages),
            system_prompt_len=len(system_prompt) if system_prompt else 0,
            tool_count=tool_count,
            last_msg_preview=last_msg_preview,
        )

        await self._semaphore.acquire()

        try:
            headers: dict[str, str] = {"Content-Type": "application/json"}

            if self._is_anthropic_mode:
                api_key = self._config.anthropic_api_key or ""
                headers["x-api-key"] = api_key
                headers["anthropic-version"] = "2023-06-01"
            else:
                token = await self._auth.token()
                headers["Authorization"] = f"Bearer {token}"

            response = await self._http.send(
                self._http.build_request("POST", url, json=body, headers=headers),
                stream=True,
            )

            if response.status_code >= 400:
                body_text = await response.aread()
                await response.aclose()
                self._semaphore.release()
                raise RuntimeError(
                    f"Claude returned {response.status_code}: {body_text.decode(errors='replace')}"
                )

        except Exception:
            self._semaphore.release()
            raise

        queue: asyncio.Queue[StreamEvent] = asyncio.Queue(maxsize=STREAM_QUEUE_SIZE)

        async def _drive_and_release() -> None:
            try:
                await _drive_sse(response, queue)
            finally:
                await response.aclose()
                self._semaphore.release()

        asyncio.create_task(_drive_and_release())
        return queue

    async def complete(
        self,
        messages: list[Message],
        system_prompt: str | None = None,
        tools: list[Tool] | None = None,
    ) -> tuple[str, list[ToolCall]]:
        """Non-streaming call. Returns (text, tool_calls)."""
        queue = await self.stream(messages, system_prompt, tools)

        text_buf: list[str] = []
        tool_calls: list[ToolCall] = []

        while True:
            event = await queue.get()
            if isinstance(event, TextEvent):
                text_buf.append(event.text)
            elif isinstance(event, ToolUseEvent):
                tool_calls.append(event.tool_call)
            elif isinstance(event, DoneEvent):
                break
            elif isinstance(event, ErrorEvent):
                raise RuntimeError(event.message)

        text = "".join(text_buf)
        tool_names = [tc.name for tc in tool_calls]
        logger.info(
            "claude_response",
            response_len=len(text),
            tool_calls=len(tool_names),
            tool_names=tool_names,
            response_preview=text[:500],
        )
        return text, tool_calls

    def _build_anthropic_body(
        self,
        messages: list[Message],
        system_prompt: str | None,
        tools: list[Tool] | None,
        stream: bool,
    ) -> dict[str, Any]:
        body: dict[str, Any] = {
            "model": self._config.vertex_model,
            "max_tokens": 8192,
            "messages": [m.to_api_dict() for m in messages],
            "stream": stream,
        }
        if system_prompt:
            body["system"] = system_prompt
        if tools:
            body["tools"] = [t.to_dict() for t in tools]
        return body

    def _build_vertex_body(
        self,
        messages: list[Message],
        system_prompt: str | None,
        tools: list[Tool] | None,
        stream: bool,
    ) -> dict[str, Any]:
        body: dict[str, Any] = {
            "anthropic_version": "vertex-2023-10-16",
            "max_tokens": 8192,
            "messages": [m.to_api_dict() for m in messages],
            "stream": stream,
        }
        if system_prompt:
            body["system"] = system_prompt
        if tools:
            body["tools"] = [t.to_dict() for t in tools]
        return body


# ── SSE processing ──


class _ToolUseAccumulator:
    __slots__ = ("id", "name", "json_buf")

    def __init__(self, id: str, name: str) -> None:
        self.id = id
        self.name = name
        self.json_buf = ""


async def _drive_sse(response: httpx.Response, queue: asyncio.Queue[StreamEvent]) -> None:
    """Parse SSE byte stream and emit StreamEvents to the queue."""
    raw = ""
    tool_acc: _ToolUseAccumulator | None = None

    async for chunk in response.aiter_text():
        raw += chunk

        while True:
            frame_end = raw.find("\n\n")
            if frame_end < 0:
                break

            frame = raw[:frame_end]
            raw = raw[frame_end + 2 :]

            for line in frame.splitlines():
                data = line.removeprefix("data: ").strip() if line.startswith("data: ") else None
                if data is None:
                    continue

                if data == "[DONE]":
                    await queue.put(DoneEvent())
                    return

                try:
                    val = json.loads(data)
                except json.JSONDecodeError:
                    logger.warning("sse_json_parse_error", data=data[:200])
                    continue

                event_type = val.get("type", "")

                if event_type == "content_block_delta":
                    delta = val.get("delta", {})
                    delta_type = delta.get("type", "")

                    if delta_type == "text_delta":
                        text = delta.get("text", "")
                        if text:
                            await queue.put(TextEvent(text=text))

                    elif delta_type == "input_json_delta":
                        fragment = delta.get("partial_json", "")
                        if fragment and tool_acc:
                            tool_acc.json_buf += fragment

                elif event_type == "content_block_start":
                    block = val.get("content_block", {})
                    if block.get("type") == "tool_use":
                        tool_acc = _ToolUseAccumulator(
                            id=block.get("id", ""),
                            name=block.get("name", ""),
                        )

                elif event_type == "content_block_stop":
                    if tool_acc:
                        try:
                            input_data = json.loads(tool_acc.json_buf) if tool_acc.json_buf else {}
                        except json.JSONDecodeError:
                            input_data = {"_raw": tool_acc.json_buf}
                        tc = ToolCall(id=tool_acc.id, name=tool_acc.name, input=input_data)
                        await queue.put(ToolUseEvent(tool_call=tc))
                        tool_acc = None

                elif event_type == "message_start":
                    input_tokens = _deep_get(val, "message", "usage", "input_tokens") or 0
                    if input_tokens:
                        await queue.put(UsageEvent(input_tokens=input_tokens))

                elif event_type == "message_delta":
                    output_tokens = _deep_get(val, "usage", "output_tokens") or 0
                    if output_tokens:
                        await queue.put(UsageEvent(output_tokens=output_tokens))

                elif event_type == "message_stop":
                    await queue.put(DoneEvent())
                    return

                elif event_type == "error":
                    msg = _deep_get(val, "error", "message") or "unknown Claude error"
                    await queue.put(ErrorEvent(message=f"Claude error: {msg}"))
                    return

    await queue.put(DoneEvent())


def _deep_get(d: dict[str, Any], *keys: str) -> Any:
    """Safely traverse nested dicts."""
    for key in keys:
        if not isinstance(d, dict):
            return None
        d = d.get(key)  # type: ignore[assignment]
        if d is None:
            return None
    return d

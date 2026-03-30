"""Slack API client wrapper."""

from __future__ import annotations

import httpx
import structlog

from oac_slack_bot.slack.types import ConversationsRepliesResponse, PostMessageResponse

logger = structlog.get_logger()

SLACK_API = "https://slack.com/api"


class SlackClient:
    """Async Slack Web API client."""

    def __init__(self, bot_token: str) -> None:
        self._bot_token = bot_token
        self._http = httpx.AsyncClient(timeout=30)

    def _auth_headers(self) -> dict[str, str]:
        return {"Authorization": f"Bearer {self._bot_token}"}

    async def connections_open(self, app_token: str) -> str:
        """Open a Socket Mode connection. Returns the wss:// URL."""
        resp = await self._http.post(
            f"{SLACK_API}/apps.connections.open",
            headers={"Authorization": f"Bearer {app_token}"},
        )
        body = resp.json()
        if not body.get("ok"):
            raise RuntimeError(
                f"connections.open error: {body.get('error', 'unknown')}"
            )
        url = body.get("url")
        if not url:
            raise RuntimeError("connections.open: missing url")
        return url

    async def thread_history(
        self, channel: str, thread_ts: str
    ) -> ConversationsRepliesResponse:
        """Fetch thread messages."""
        resp = await self._http.get(
            f"{SLACK_API}/conversations.replies",
            headers=self._auth_headers(),
            params={"channel": channel, "ts": thread_ts, "limit": "50"},
        )
        result = ConversationsRepliesResponse.model_validate(resp.json())
        if not result.ok:
            logger.warning("conversations_replies_error", error=result.error)
        return result

    async def post_message(
        self,
        channel: str,
        text: str,
        thread_ts: str | None = None,
    ) -> PostMessageResponse:
        """Post a message to a channel/thread."""
        payload: dict[str, str] = {"channel": channel, "text": text}
        if thread_ts:
            payload["thread_ts"] = thread_ts

        resp = await self._http.post(
            f"{SLACK_API}/chat.postMessage",
            headers={**self._auth_headers(), "Content-Type": "application/json"},
            json=payload,
        )
        result = PostMessageResponse.model_validate(resp.json())
        if not result.ok:
            logger.warning("post_message_error", error=result.error)
        return result

    async def update_message(
        self, channel: str, ts: str, text: str
    ) -> PostMessageResponse:
        """Update an existing message."""
        resp = await self._http.post(
            f"{SLACK_API}/chat.update",
            headers={**self._auth_headers(), "Content-Type": "application/json"},
            json={"channel": channel, "ts": ts, "text": text},
        )
        result = PostMessageResponse.model_validate(resp.json())
        if not result.ok:
            logger.debug("update_message_error", error=result.error)
        return result

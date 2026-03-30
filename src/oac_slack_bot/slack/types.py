"""Pydantic models for Slack API payloads."""

from __future__ import annotations

from typing import Any

from pydantic import BaseModel, ConfigDict, Field


class ReactionItem(BaseModel):
    model_config = ConfigDict(extra="ignore")

    type: str | None = None
    channel: str | None = None
    ts: str | None = None


class SlackEvent(BaseModel):
    model_config = ConfigDict(extra="ignore")

    type: str = Field(alias="type", default="")
    user: str | None = None
    text: str | None = None
    channel: str | None = None
    ts: str | None = None
    thread_ts: str | None = None
    bot_id: str | None = None
    reaction: str | None = None
    item: ReactionItem | None = None


class EventsPayload(BaseModel):
    model_config = ConfigDict(extra="ignore")

    event: SlackEvent | None = None


class Envelope(BaseModel):
    model_config = ConfigDict(extra="ignore")

    envelope_id: str = ""
    type: str = ""
    payload: dict[str, Any] | None = None


class ThreadMessage(BaseModel):
    model_config = ConfigDict(extra="ignore")

    user: str | None = None
    text: str | None = None
    ts: str | None = None
    bot_id: str | None = None


class ConversationsRepliesResponse(BaseModel):
    model_config = ConfigDict(extra="ignore")

    ok: bool = False
    messages: list[ThreadMessage] | None = None
    error: str | None = None


class PostMessageResponse(BaseModel):
    model_config = ConfigDict(extra="ignore")

    ok: bool = False
    ts: str | None = None
    error: str | None = None

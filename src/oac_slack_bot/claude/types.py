"""Types for Claude API interactions."""

from __future__ import annotations

from dataclasses import dataclass
from enum import StrEnum
from typing import Any


class Role(StrEnum):
    USER = "user"
    ASSISTANT = "assistant"


@dataclass
class Message:
    role: Role
    content: str | list[ContentBlock]

    @staticmethod
    def user(text: str) -> Message:
        return Message(role=Role.USER, content=text)

    @staticmethod
    def assistant(text: str) -> Message:
        return Message(role=Role.ASSISTANT, content=text)

    @staticmethod
    def tool_result(tool_use_id: str, content: str) -> Message:
        return Message(
            role=Role.USER,
            content=[ToolResultBlock(tool_use_id=tool_use_id, content=content)],
        )

    def to_api_dict(self) -> dict[str, Any]:
        """Serialize for the Anthropic API."""
        if isinstance(self.content, str):
            return {"role": self.role.value, "content": self.content}
        return {
            "role": self.role.value,
            "content": [block.to_dict() for block in self.content],
        }


@dataclass
class TextBlock:
    text: str
    type: str = "text"

    def to_dict(self) -> dict[str, Any]:
        return {"type": self.type, "text": self.text}


@dataclass
class ToolUseBlock:
    id: str
    name: str
    input: dict[str, Any]
    type: str = "tool_use"

    def to_dict(self) -> dict[str, Any]:
        return {"type": self.type, "id": self.id, "name": self.name, "input": self.input}


@dataclass
class ToolResultBlock:
    tool_use_id: str
    content: str
    type: str = "tool_result"

    def to_dict(self) -> dict[str, Any]:
        return {"type": self.type, "tool_use_id": self.tool_use_id, "content": self.content}


ContentBlock = TextBlock | ToolUseBlock | ToolResultBlock


@dataclass
class ToolCall:
    id: str
    name: str
    input: dict[str, Any]


@dataclass
class Tool:
    name: str
    description: str
    input_schema: dict[str, Any]

    def to_dict(self) -> dict[str, Any]:
        return {
            "name": self.name,
            "description": self.description,
            "input_schema": self.input_schema,
        }


# ── Stream events ──


@dataclass
class TextEvent:
    text: str


@dataclass
class ToolUseEvent:
    tool_call: ToolCall


@dataclass
class UsageEvent:
    input_tokens: int = 0
    output_tokens: int = 0


@dataclass
class DoneEvent:
    pass


@dataclass
class ErrorEvent:
    message: str


StreamEvent = TextEvent | ToolUseEvent | UsageEvent | DoneEvent | ErrorEvent

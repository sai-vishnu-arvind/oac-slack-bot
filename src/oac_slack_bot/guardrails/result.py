"""Guardrail result type."""

from __future__ import annotations


class GuardrailResult:
    """Result of a guardrails check."""

    __slots__ = ("allowed", "reason")

    def __init__(self, allowed: bool, reason: str | None = None) -> None:
        self.allowed = allowed
        self.reason = reason

    @staticmethod
    def ok() -> GuardrailResult:
        return GuardrailResult(allowed=True)

    @staticmethod
    def blocked(reason: str) -> GuardrailResult:
        return GuardrailResult(allowed=False, reason=reason)

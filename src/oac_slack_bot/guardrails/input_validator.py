"""Input validation guardrail."""

from __future__ import annotations

from oac_slack_bot.guardrails.result import GuardrailResult


class InputValidator:
    """Validates message content before processing."""

    def __init__(self, max_length: int = 10_000) -> None:
        self._max_length = max_length

    def validate(self, text: str) -> GuardrailResult:
        # Strip null bytes
        if "\x00" in text:
            return GuardrailResult.blocked("Message contains null bytes")

        if len(text) > self._max_length:
            return GuardrailResult.blocked(
                f"Message too long ({len(text)} chars, max {self._max_length})"
            )

        # Check for excessive whitespace (likely spam)
        if text.strip() and len(text) > 100:
            non_space = sum(1 for c in text if not c.isspace())
            if non_space / len(text) < 0.1:
                return GuardrailResult.blocked("Message is mostly whitespace")

        return GuardrailResult.ok()

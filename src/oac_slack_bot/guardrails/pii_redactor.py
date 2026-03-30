"""PII redaction for audit logs."""

from __future__ import annotations

import re

# Patterns for common PII
_PII_PATTERNS: list[tuple[re.Pattern[str], str]] = [
    # Email addresses
    (re.compile(r"[a-zA-Z0-9._%+-]+@[a-zA-Z0-9.-]+\.[a-zA-Z]{2,}"), "[REDACTED_EMAIL]"),
    # API keys / tokens (must come before phone to avoid partial matches)
    (re.compile(r"sk-[a-zA-Z0-9]{20,}"), "[REDACTED_API_KEY]"),
    (re.compile(r"xoxb-[a-zA-Z0-9-]+"), "[REDACTED_SLACK_TOKEN]"),
    (re.compile(r"xoxp-[a-zA-Z0-9-]+"), "[REDACTED_SLACK_TOKEN]"),
    (re.compile(r"xapp-[a-zA-Z0-9-]+"), "[REDACTED_SLACK_TOKEN]"),
    (re.compile(r"Bearer\s+[a-zA-Z0-9._-]{20,}"), "[REDACTED_BEARER]"),
    # AWS keys
    (re.compile(r"AKIA[0-9A-Z]{16}"), "[REDACTED_AWS_KEY]"),
    # Credit card numbers (basic pattern)
    (re.compile(r"\b\d{4}[-\s]?\d{4}[-\s]?\d{4}[-\s]?\d{4}\b"), "[REDACTED_CC]"),
    # Phone numbers (last — most greedy pattern)
    (re.compile(r"\+?\d{1,3}[-.\s]?\(?\d{1,4}\)?[-.\s]?\d{1,4}[-.\s]?\d{1,9}"), "[REDACTED_PHONE]"),
]


class PIIRedactor:
    """Redacts personally identifiable information from text."""

    def __init__(self, enabled: bool = True) -> None:
        self._enabled = enabled

    def redact(self, text: str) -> str:
        """Redact PII from text for safe logging."""
        if not self._enabled:
            return text

        result = text
        for pattern, replacement in _PII_PATTERNS:
            result = pattern.sub(replacement, result)
        return result

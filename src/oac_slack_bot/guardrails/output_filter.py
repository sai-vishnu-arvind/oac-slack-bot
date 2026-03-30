"""Output content filtering guardrail."""

from __future__ import annotations

import re

# Patterns to detect in output
_SENSITIVE_PATTERNS: list[tuple[re.Pattern[str], str]] = [
    # Internal IP addresses
    (re.compile(r"\b10\.\d{1,3}\.\d{1,3}\.\d{1,3}\b"), "internal IP address"),
    (re.compile(r"\b172\.(1[6-9]|2\d|3[01])\.\d{1,3}\.\d{1,3}\b"), "internal IP address"),
    (re.compile(r"\b192\.168\.\d{1,3}\.\d{1,3}\b"), "internal IP address"),
    # API keys in output
    (re.compile(r"sk-[a-zA-Z0-9]{20,}"), "API key"),
    (re.compile(r"xoxb-[a-zA-Z0-9-]+"), "Slack bot token"),
    (re.compile(r"xoxp-[a-zA-Z0-9-]+"), "Slack user token"),
    # AWS keys
    (re.compile(r"AKIA[0-9A-Z]{16}"), "AWS access key"),
    # Password-like patterns in structured output
    (re.compile(r'"password"\s*:\s*"[^"]{4,}"', re.IGNORECASE), "password in output"),
]


class OutputFilter:
    """Scans Claude responses for potentially sensitive content."""

    def filter(self, text: str) -> tuple[str, list[str]]:
        """Filter output text. Returns (text, warnings).

        Currently only warns, does not modify text — to avoid breaking
        legitimate responses that reference internal infrastructure.
        """
        warnings: list[str] = []

        for pattern, description in _SENSITIVE_PATTERNS:
            if pattern.search(text):
                warnings.append(f"Output contains {description}")

        return text, warnings

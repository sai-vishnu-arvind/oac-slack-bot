"""Prompt injection detection guardrail."""

from __future__ import annotations

import re

# Patterns that suggest prompt injection attempts
_INJECTION_PATTERNS: list[tuple[str, str]] = [
    (r"ignore\s+(all\s+)?previous\s+instructions", "instruction override attempt"),
    (r"ignore\s+(all\s+)?above", "instruction override attempt"),
    (r"disregard\s+(all\s+)?previous", "instruction override attempt"),
    (r"you\s+are\s+now\s+a", "role reassignment attempt"),
    (r"pretend\s+you\s+are", "role reassignment attempt"),
    (r"act\s+as\s+if\s+you", "role reassignment attempt"),
    (r"^system:\s", "system prompt injection"),
    (r"\[system\]", "system prompt injection"),
    (r"<\|im_start\|>system", "chat ML injection"),
    (r"ADMIN\s*MODE", "privilege escalation attempt"),
    (r"developer\s+mode", "mode override attempt"),
    (r"DAN\s+mode", "jailbreak attempt"),
    (r"bypass\s+(all\s+)?safety", "safety bypass attempt"),
    (r"bypass\s+(all\s+)?restrictions", "safety bypass attempt"),
]

_COMPILED_PATTERNS = [(re.compile(p, re.IGNORECASE), reason) for p, reason in _INJECTION_PATTERNS]


class InjectionDetector:
    """Detects common prompt injection patterns."""

    def __init__(self, enabled: bool = True) -> None:
        self._enabled = enabled

    def check(self, text: str) -> tuple[bool, str | None]:
        """Check text for injection patterns. Returns (is_suspicious, reason)."""
        if not self._enabled:
            return False, None

        for pattern, reason in _COMPILED_PATTERNS:
            if pattern.search(text):
                return True, reason

        # Check for excessive repetition (potential token stuffing)
        words = text.split()
        if len(words) > 20:
            unique_ratio = len(set(words)) / len(words)
            if unique_ratio < 0.1:
                return True, "excessive repetition detected"

        # Check for base64-encoded payloads (suspiciously long alphanumeric strings)
        if re.search(r"[A-Za-z0-9+/=]{200,}", text):
            return True, "suspicious encoded payload"

        return False, None

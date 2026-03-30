"""Structured audit logging for all bot interactions."""

from __future__ import annotations

import structlog

from oac_slack_bot.guardrails.pii_redactor import PIIRedactor

logger = structlog.get_logger("audit")


class AuditLogger:
    """Logs all bot interactions with PII redaction."""

    def __init__(self, pii_redactor: PIIRedactor) -> None:
        self._redactor = pii_redactor

    def log_request(
        self,
        user: str | None,
        channel: str,
        thread_ts: str,
        message: str,
        plugin_fqn: str | None = None,
    ) -> None:
        logger.info(
            "audit_request",
            user=user,
            channel=channel,
            thread_ts=thread_ts,
            message=self._redactor.redact(message[:500]),
            plugin=plugin_fqn,
        )

    def log_response(
        self,
        user: str | None,
        channel: str,
        thread_ts: str,
        response_len: int,
        input_tokens: int = 0,
        output_tokens: int = 0,
        duration_ms: int = 0,
        plugin_fqn: str | None = None,
    ) -> None:
        logger.info(
            "audit_response",
            user=user,
            channel=channel,
            thread_ts=thread_ts,
            response_len=response_len,
            input_tokens=input_tokens,
            output_tokens=output_tokens,
            duration_ms=duration_ms,
            plugin=plugin_fqn,
        )

    def log_guardrail_block(
        self,
        user: str | None,
        channel: str,
        reason: str,
        message: str,
    ) -> None:
        logger.warning(
            "audit_guardrail_block",
            user=user,
            channel=channel,
            reason=reason,
            message=self._redactor.redact(message[:200]),
        )

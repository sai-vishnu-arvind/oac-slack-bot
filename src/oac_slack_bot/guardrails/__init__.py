"""Guardrails pipeline for input validation, rate limiting, and safety checks."""

from __future__ import annotations

import structlog

from oac_slack_bot.guardrails.audit_logger import AuditLogger
from oac_slack_bot.guardrails.channel_whitelist import ChannelWhitelist
from oac_slack_bot.guardrails.cost_tracker import CostTracker
from oac_slack_bot.guardrails.injection_detector import InjectionDetector
from oac_slack_bot.guardrails.input_validator import InputValidator
from oac_slack_bot.guardrails.output_filter import OutputFilter
from oac_slack_bot.guardrails.pii_redactor import PIIRedactor
from oac_slack_bot.guardrails.rate_limiter import RateLimiter
from oac_slack_bot.guardrails.result import GuardrailResult

logger = structlog.get_logger()


class GuardrailsPipeline:
    """Orchestrates all guardrails checks in sequence."""

    def __init__(
        self,
        input_validator: InputValidator,
        rate_limiter: RateLimiter,
        channel_whitelist: ChannelWhitelist,
        injection_detector: InjectionDetector,
        cost_tracker: CostTracker,
        pii_redactor: PIIRedactor,
        output_filter: OutputFilter,
        audit_logger: AuditLogger,
    ) -> None:
        self.input_validator = input_validator
        self.rate_limiter = rate_limiter
        self.channel_whitelist = channel_whitelist
        self.injection_detector = injection_detector
        self.cost_tracker = cost_tracker
        self.pii_redactor = pii_redactor
        self.output_filter = output_filter
        self.audit_logger = audit_logger

    async def check_input(
        self,
        text: str,
        user_id: str | None,
        channel_id: str,
    ) -> GuardrailResult:
        """Run all input guardrails. Returns blocked result on first failure."""
        # 1. Channel whitelist
        if not self.channel_whitelist.is_allowed(channel_id):
            return GuardrailResult.blocked("Channel not in whitelist")

        # 2. Input validation
        result = self.input_validator.validate(text)
        if not result.allowed:
            return result

        # 3. Rate limiting
        if user_id:
            user_ok = self.rate_limiter.check_user(user_id)
            if not user_ok:
                return GuardrailResult.blocked("Rate limit exceeded for user")

        channel_ok = self.rate_limiter.check_channel(channel_id)
        if not channel_ok:
            return GuardrailResult.blocked("Rate limit exceeded for channel")

        # 4. Injection detection
        is_suspicious, reason = self.injection_detector.check(text)
        if is_suspicious:
            logger.warning("injection_detected", user=user_id, reason=reason)
            return GuardrailResult.blocked(f"Message flagged: {reason}")

        # 5. Cost budget
        if user_id:
            within_budget, remaining = self.cost_tracker.check_budget(user_id)
            if not within_budget:
                return GuardrailResult.blocked(
                    f"Daily token budget exhausted (remaining: {remaining})"
                )

        return GuardrailResult.ok()

    def filter_output(self, text: str) -> tuple[str, list[str]]:
        """Filter output text. Returns (filtered_text, warnings)."""
        return self.output_filter.filter(text)

    def redact_for_logging(self, text: str) -> str:
        """Redact PII from text before logging."""
        return self.pii_redactor.redact(text)

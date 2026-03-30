"""Application wiring and startup."""

from __future__ import annotations

import asyncio
import re

import structlog
from dotenv import load_dotenv
from slack_bolt.adapter.socket_mode.async_handler import AsyncSocketModeHandler
from slack_bolt.async_app import AsyncApp

from oac_slack_bot.claude.client import ClaudeClient
from oac_slack_bot.claude.session import SessionStore
from oac_slack_bot.config import Config
from oac_slack_bot.guardrails import GuardrailsPipeline
from oac_slack_bot.guardrails.audit_logger import AuditLogger
from oac_slack_bot.guardrails.channel_whitelist import ChannelWhitelist
from oac_slack_bot.guardrails.cost_tracker import CostTracker
from oac_slack_bot.guardrails.injection_detector import InjectionDetector
from oac_slack_bot.guardrails.input_validator import InputValidator
from oac_slack_bot.guardrails.output_filter import OutputFilter
from oac_slack_bot.guardrails.pii_redactor import PIIRedactor
from oac_slack_bot.guardrails.rate_limiter import RateLimiter
from oac_slack_bot.metrics import Metrics
from oac_slack_bot.plugins.registry import PluginRegistry
from oac_slack_bot.slack.client import SlackClient
from oac_slack_bot.slack.events import handle_mention, handle_reaction
from oac_slack_bot.slack.types import SlackEvent

logger = structlog.get_logger()


async def main() -> None:
    """Start the OAC Slack Bot."""
    load_dotenv()

    # Configure structlog
    structlog.configure(
        processors=[
            structlog.contextvars.merge_contextvars,
            structlog.processors.add_log_level,
            structlog.processors.TimeStamper(fmt="iso"),
            structlog.dev.ConsoleRenderer(),
        ],
        wrapper_class=structlog.make_filtering_bound_logger(20),  # INFO
    )

    # Load config
    config = Config()  # type: ignore[call-arg]

    logger.info(
        "oac_slack_bot_starting",
        vertex_project=config.vertex_project_id,
        vertex_region=config.vertex_region,
        vertex_model=config.vertex_model,
        plugin_dirs=config.get_plugin_dirs(),
        default_plugin=config.default_plugin,
    )

    # Build shared state
    registry = PluginRegistry.load(config.get_plugin_dirs())
    claude = ClaudeClient(config)
    sessions = SessionStore(500, config.session_ttl_secs)
    sessions_lock = asyncio.Lock()
    slack_client = SlackClient(config.slack_bot_token)
    metrics = Metrics()

    # Build guardrails pipeline
    pii_redactor = PIIRedactor(enabled=config.enable_pii_redaction)
    guardrails = GuardrailsPipeline(
        input_validator=InputValidator(max_length=config.max_message_length),
        rate_limiter=RateLimiter(
            user_limit=config.rate_limit_per_user,
            channel_limit=config.rate_limit_per_channel,
        ),
        channel_whitelist=ChannelWhitelist(config.get_allowed_channels()),
        injection_detector=InjectionDetector(enabled=config.enable_injection_detection),
        cost_tracker=CostTracker(budget_per_user=config.cost_budget_per_user),
        pii_redactor=pii_redactor,
        output_filter=OutputFilter(),
        audit_logger=AuditLogger(pii_redactor),
    )

    # Create slack-bolt app
    app = AsyncApp(token=config.slack_bot_token)

    @app.event("app_mention")
    async def on_mention(event: dict, say: object) -> None:  # noqa: ARG001
        slack_event = SlackEvent.model_validate(event)
        if slack_event.bot_id:
            return
        asyncio.create_task(
            handle_mention(
                event=slack_event,
                slack=slack_client,
                claude=claude,
                sessions=sessions,
                sessions_lock=sessions_lock,
                registry=registry,
                metrics=metrics,
                default_plugin=config.default_plugin,
                guardrails=guardrails,
            )
        )

    @app.event("reaction_added")
    async def on_reaction(event: dict, say: object) -> None:  # noqa: ARG001
        slack_event = SlackEvent.model_validate(event)
        asyncio.create_task(handle_reaction(slack_event, metrics))

    # Catch-all for unhandled events
    @app.event({"type": re.compile(".*")})  # type: ignore[arg-type]
    async def catch_all(event: dict) -> None:
        pass

    # Periodic tasks
    async def session_cleanup_loop() -> None:
        while True:
            await asyncio.sleep(300)
            async with sessions_lock:
                sessions.cleanup()

    async def metrics_summary_loop() -> None:
        while True:
            await asyncio.sleep(300)
            await metrics.log_summary()

    asyncio.create_task(session_cleanup_loop())
    asyncio.create_task(metrics_summary_loop())

    # Start Socket Mode
    logger.info("connecting_to_slack_socket_mode")
    handler = AsyncSocketModeHandler(app, config.slack_app_token)
    await handler.start_async()


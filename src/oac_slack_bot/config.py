"""Application configuration from environment variables."""

from __future__ import annotations

import os
from pathlib import Path

from pydantic import field_validator
from pydantic_settings import BaseSettings


class Config(BaseSettings):
    """Bot configuration loaded from environment variables."""

    model_config = {"env_file": ".env", "env_file_encoding": "utf-8", "extra": "ignore"}

    # Slack (required)
    slack_bot_token: str
    slack_app_token: str

    # Vertex AI
    vertex_project_id: str = ""
    vertex_region: str = "us-east5"
    vertex_model: str = ""

    # GCP auth (local dev; pod uses Workload Identity)
    google_application_credentials: str | None = None

    # Anthropic API / LiteLLM mode (alternative to Vertex AI)
    anthropic_base_url: str | None = None
    anthropic_api_key: str | None = None

    # Plugin dirs (colon-separated)
    plugin_dirs: str = ""

    # Default plugin FQN
    default_plugin: str | None = None

    # Session TTL in seconds
    session_ttl_secs: int = 1800

    # DevRev
    devrev_token: str | None = None

    # ── Guardrails ──
    max_message_length: int = 10_000
    rate_limit_per_user: int = 20  # requests per minute
    rate_limit_per_channel: int = 50
    allowed_channels: str = ""  # comma-separated, empty = no whitelist
    cost_budget_per_user: int = 100_000  # tokens per day
    enable_pii_redaction: bool = True
    enable_injection_detection: bool = True

    @field_validator(
        "anthropic_base_url", "anthropic_api_key", "default_plugin", "devrev_token",
        mode="before",
    )
    @classmethod
    def empty_str_to_none(cls, v: str | None) -> str | None:
        if isinstance(v, str) and not v.strip():
            return None
        return v

    @field_validator("vertex_model", mode="before")
    @classmethod
    def resolve_model(cls, v: str | None) -> str:
        if v:
            return v
        return os.environ.get("ANTHROPIC_DEFAULT_SONNET_MODEL", "claude-sonnet-4-6")

    def get_plugin_dirs(self) -> list[str]:
        """Return expanded plugin directory paths."""
        raw = self.plugin_dirs
        if not raw:
            home = Path.home()
            raw = f"{home}/.agents/skills:{home}/claude-plugins/plugins"
        return [str(Path(p).expanduser()) for p in raw.split(":") if p.strip()]

    def get_allowed_channels(self) -> set[str] | None:
        """Return set of allowed channel IDs, or None for no whitelist."""
        if not self.allowed_channels.strip():
            return None
        return {c.strip() for c in self.allowed_channels.split(",") if c.strip()}

    @property
    def vertex_endpoint(self) -> str:
        return (
            f"https://{self.vertex_region}-aiplatform.googleapis.com/v1/"
            f"projects/{self.vertex_project_id}/locations/{self.vertex_region}/"
            f"publishers/anthropic/models/{self.vertex_model}:streamRawPredict"
        )

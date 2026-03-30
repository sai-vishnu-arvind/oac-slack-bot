"""GCP OAuth2 authentication for Vertex AI."""

from __future__ import annotations

import asyncio
import json
import os
import time
from pathlib import Path

import httpx
import structlog

logger = structlog.get_logger()

TOKEN_REFRESH_BUFFER_SECS = 60
OAUTH_TOKEN_URL = "https://oauth2.googleapis.com/token"
METADATA_TOKEN_URL = (
    "http://metadata.google.internal/computeMetadata/v1/"
    "instance/service-accounts/default/token"
)
SCOPES = "https://www.googleapis.com/auth/cloud-platform"


def resolve_credentials_path() -> str | None:
    """Find GCP credentials, checking env var then default ADC location."""
    env_path = os.environ.get("GOOGLE_APPLICATION_CREDENTIALS")
    if env_path and Path(env_path).exists():
        return env_path

    default_path = Path.home() / ".config" / "gcloud" / "application_default_credentials.json"
    if default_path.exists():
        return str(default_path)

    return None


class GcpAuth:
    """GCP OAuth2 token provider with caching.

    Supports:
    - ADC file (authorized_user type with refresh_token)
    - GCE metadata server (Workload Identity / service account)
    """

    def __init__(self, credentials_path: str | None = None) -> None:
        self._credentials_path = credentials_path
        self._http = httpx.AsyncClient(timeout=10)
        self._cached_token: tuple[str, float] | None = None  # (token, expires_at)
        self._lock = asyncio.Lock()

    async def token(self) -> str:
        """Get a valid access token, refreshing if needed."""
        async with self._lock:
            now = time.time()
            if self._cached_token:
                token, expires_at = self._cached_token
                if now < expires_at - TOKEN_REFRESH_BUFFER_SECS:
                    return token

            token, expires_in = await self._fetch_token()
            self._cached_token = (token, now + expires_in)
            return token

    async def _fetch_token(self) -> tuple[str, float]:
        """Fetch a fresh token. Returns (access_token, expires_in_secs)."""
        if self._credentials_path:
            return await self._fetch_from_file(self._credentials_path)
        return await self._fetch_from_metadata_server()

    async def _fetch_from_file(self, path: str) -> tuple[str, float]:
        """Read ADC file and refresh the OAuth2 token."""
        try:
            with open(path) as f:
                creds = json.load(f)
        except (OSError, json.JSONDecodeError) as e:
            raise RuntimeError(f"Failed to read credentials file {path}: {e}") from e

        cred_type = creds.get("type", "")

        if cred_type == "authorized_user":
            client_id = creds.get("client_id", "")
            client_secret = creds.get("client_secret", "")
            refresh_token = creds.get("refresh_token", "")

            if not refresh_token:
                logger.info("no_refresh_token_in_adc", path=path)
                return await self._fetch_from_metadata_server()

            return await self._refresh_oauth2_token(client_id, client_secret, refresh_token)

        if cred_type == "service_account":
            raise RuntimeError(
                "Service account JSON not supported for direct token refresh. "
                "Use Workload Identity or set GOOGLE_APPLICATION_CREDENTIALS and use google-auth."
            )

        logger.info("unknown_credential_type", type=cred_type, path=path)
        return await self._fetch_from_metadata_server()

    async def _refresh_oauth2_token(
        self, client_id: str, client_secret: str, refresh_token: str
    ) -> tuple[str, float]:
        """Refresh an OAuth2 token using client credentials."""
        resp = await self._http.post(
            OAUTH_TOKEN_URL,
            data={
                "grant_type": "refresh_token",
                "client_id": client_id,
                "client_secret": client_secret,
                "refresh_token": refresh_token,
            },
        )

        if resp.status_code != 200:
            raise RuntimeError(f"OAuth2 token refresh failed ({resp.status_code}): {resp.text}")

        body = resp.json()
        access_token = body.get("access_token", "")
        expires_in = body.get("expires_in", 3600)

        if not access_token:
            raise RuntimeError("OAuth2 response missing access_token")

        logger.debug("oauth2_token_refreshed", expires_in=expires_in)
        return access_token, float(expires_in)

    async def _fetch_from_metadata_server(self) -> tuple[str, float]:
        """Fetch token from GCE metadata server (Workload Identity)."""
        try:
            resp = await self._http.get(
                METADATA_TOKEN_URL,
                headers={"Metadata-Flavor": "Google"},
            )
        except httpx.ConnectError as e:
            raise RuntimeError(
                f"Cannot reach metadata server (not running on GCP?): {e}"
            ) from e

        if resp.status_code != 200:
            raise RuntimeError(f"Metadata server returned {resp.status_code}: {resp.text}")

        body = resp.json()
        access_token = body.get("access_token", "")
        expires_in = body.get("expires_in", 3600)

        if not access_token:
            raise RuntimeError("Metadata server response missing access_token")

        logger.debug("metadata_token_fetched", expires_in=expires_in)
        return access_token, float(expires_in)

use reqwest::Client;
use serde::Deserialize;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;
use tracing::{debug, info};

/// Sixty seconds of buffer before we consider a token expired and refresh it.
const EXPIRY_BUFFER_SECS: u64 = 60;

/// Default lifetime assumed when the response does not include `expires_in`.
const DEFAULT_TOKEN_LIFETIME_SECS: u64 = 3600;

// ── Wire types ───────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct TokenResponse {
    access_token: String,
    expires_in: Option<u64>,
}

/// Contents of an Application Default Credentials JSON file that has a
/// `refresh_token` (produced by `gcloud auth application-default login`).
#[derive(Debug, Deserialize)]
struct AdcFile {
    /// May be "authorized_user" (ADC) or "service_account".
    #[serde(rename = "type")]
    credential_type: Option<String>,
    // ADC / authorized_user fields
    client_id: Option<String>,
    client_secret: Option<String>,
    refresh_token: Option<String>,
    /// Some ADC files already carry a (possibly stale) access_token.
    access_token: Option<String>,
}

// ── Cached token ─────────────────────────────────────────────────────────────

struct CachedToken {
    token: String,
    /// Instant at which this token should be considered expired (with buffer).
    expires_at: Instant,
}

impl CachedToken {
    fn is_valid(&self) -> bool {
        Instant::now() < self.expires_at
    }
}

// ── GcpAuth ──────────────────────────────────────────────────────────────────

/// Provides GCP Bearer tokens, handling both in-pod (Workload Identity) and
/// local-dev (ADC / `GOOGLE_APPLICATION_CREDENTIALS`) scenarios.
pub struct GcpAuth {
    http: Client,
    /// Path to an ADC / service-account JSON file, or `None` to use the
    /// metadata server.
    credentials_path: Option<String>,
    cached: Mutex<Option<CachedToken>>,
}

impl GcpAuth {
    pub fn new(credentials_path: Option<String>) -> Self {
        Self {
            http: Client::new(),
            credentials_path,
            cached: Mutex::new(None),
        }
    }

    /// Return a valid Bearer token, refreshing automatically when needed.
    pub async fn token(&self) -> Result<String, String> {
        let mut guard = self.cached.lock().await;

        // Fast path: reuse cached token if still valid.
        if let Some(ref cached) = *guard {
            if cached.is_valid() {
                debug!("Reusing cached GCP token");
                return Ok(cached.token.clone());
            }
        }

        info!("Refreshing GCP auth token");
        let (token, lifetime) = self.fetch_token().await?;

        let expires_at =
            Instant::now() + Duration::from_secs(lifetime.saturating_sub(EXPIRY_BUFFER_SECS));

        *guard = Some(CachedToken {
            token: token.clone(),
            expires_at,
        });

        Ok(token)
    }

    // ── Internal fetch logic ─────────────────────────────────────────────────

    async fn fetch_token(&self) -> Result<(String, u64), String> {
        match &self.credentials_path {
            Some(path) => self.fetch_from_file(path).await,
            None => self.fetch_from_metadata_server().await,
        }
    }

    /// Try to read the file at `path` as an ADC JSON.  If it contains a
    /// `refresh_token` field we exchange it for a fresh access token.
    async fn fetch_from_file(&self, path: &str) -> Result<(String, u64), String> {
        let raw = std::fs::read_to_string(path)
            .map_err(|e| format!("Cannot read credentials file {path}: {e}"))?;

        let adc: AdcFile = serde_json::from_str(&raw)
            .map_err(|e| format!("Cannot parse credentials file {path}: {e}"))?;

        match adc.credential_type.as_deref() {
            // ── authorized_user (ADC from `gcloud auth application-default login`) ──
            Some("authorized_user") | None => {
                if let (Some(client_id), Some(client_secret), Some(refresh_token)) =
                    (adc.client_id, adc.client_secret, adc.refresh_token)
                {
                    return self
                        .refresh_oauth2_token(&client_id, &client_secret, &refresh_token)
                        .await;
                }

                // If there is no refresh_token fall through to the metadata server.
                info!("ADC file has no refresh_token — falling back to metadata server");
                self.fetch_from_metadata_server().await
            }

            Some("service_account") => {
                // Service account key files require JWT signing (RS256).
                // Rather than bundle that complexity here, we fall back to
                // the metadata server if running in GCP, or surface a clear
                // error for local dev.
                Err(
                    "Service account key files are not supported directly. \
                     Use `gcloud auth application-default login` for local dev \
                     or Workload Identity in GCP."
                        .into(),
                )
            }

            Some(t) => Err(format!("Unsupported credential type in {path}: {t}")),
        }
    }

    /// Exchange a refresh_token for an access_token via the OAuth2 token endpoint.
    async fn refresh_oauth2_token(
        &self,
        client_id: &str,
        client_secret: &str,
        refresh_token: &str,
    ) -> Result<(String, u64), String> {
        debug!("Refreshing OAuth2 token with refresh_token");

        let params = [
            ("client_id", client_id),
            ("client_secret", client_secret),
            ("refresh_token", refresh_token),
            ("grant_type", "refresh_token"),
        ];

        let resp = self
            .http
            .post("https://oauth2.googleapis.com/token")
            .form(&params)
            .send()
            .await
            .map_err(|e| format!("OAuth2 token request failed: {e}"))?;

        let status = resp.status();
        let body = resp
            .text()
            .await
            .map_err(|e| format!("OAuth2 token response read failed: {e}"))?;

        if !status.is_success() {
            return Err(format!("OAuth2 token endpoint returned {status}: {body}"));
        }

        let token_resp: TokenResponse = serde_json::from_str(&body)
            .map_err(|e| format!("Cannot parse OAuth2 token response: {e} — body: {body}"))?;

        let lifetime = token_resp
            .expires_in
            .unwrap_or(DEFAULT_TOKEN_LIFETIME_SECS);

        info!(expires_in = lifetime, "OAuth2 token refreshed successfully");
        Ok((token_resp.access_token, lifetime))
    }

    /// Fetch a token from the GCP instance metadata server (Workload Identity).
    async fn fetch_from_metadata_server(&self) -> Result<(String, u64), String> {
        debug!("Fetching GCP token from metadata server");

        let resp = self
            .http
            .get(
                "http://metadata.google.internal/computeMetadata/v1/\
                 instance/service-accounts/default/token",
            )
            .header("Metadata-Flavor", "Google")
            .send()
            .await
            .map_err(|e| format!("Metadata server request failed: {e}"))?;

        let status = resp.status();
        let body = resp
            .text()
            .await
            .map_err(|e| format!("Metadata server response read failed: {e}"))?;

        if !status.is_success() {
            return Err(format!(
                "Metadata server returned {status}: {body}"
            ));
        }

        let token_resp: TokenResponse = serde_json::from_str(&body)
            .map_err(|e| format!("Cannot parse metadata token response: {e} — body: {body}"))?;

        let lifetime = token_resp
            .expires_in
            .unwrap_or(DEFAULT_TOKEN_LIFETIME_SECS);

        info!(expires_in = lifetime, "Workload Identity token fetched successfully");
        Ok((token_resp.access_token, lifetime))
    }
}

// ── Helper: resolve the best available credentials path ─────────────────────

/// Resolve the credential source to use:
/// 1. `GOOGLE_APPLICATION_CREDENTIALS` env var (if set).
/// 2. `~/.config/gcloud/application_default_credentials.json` (ADC default).
/// 3. `None` → use the metadata server (in-pod / Workload Identity).
pub fn resolve_credentials_path() -> Option<String> {
    // 1. Explicit env var.
    if let Ok(path) = std::env::var("GOOGLE_APPLICATION_CREDENTIALS") {
        if std::path::Path::new(&path).exists() {
            return Some(path);
        }
    }

    // 2. Well-known ADC location.
    if let Ok(home) = std::env::var("HOME") {
        let adc_path = format!("{home}/.config/gcloud/application_default_credentials.json");
        if std::path::Path::new(&adc_path).exists() {
            return Some(adc_path);
        }
    }

    // 3. Fall back to metadata server.
    None
}

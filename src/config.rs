use std::env;

#[derive(Clone, Debug)]
pub struct Config {
    // Slack
    pub slack_bot_token: String,
    pub slack_app_token: String,

    // Vertex AI
    pub vertex_project_id: String,
    pub vertex_region: String,
    pub vertex_model: String,

    // GCP auth (local dev; pod uses Workload Identity)
    pub google_application_credentials: Option<String>,

    // Anthropic API / LiteLLM mode (alternative to Vertex AI)
    pub anthropic_base_url: Option<String>,
    pub anthropic_api_key: Option<String>,

    // Plugin dirs to scan for SKILL.md files
    pub plugin_dirs: Vec<String>,

    // Default plugin FQN — if set, every message routes here when no router match.
    pub default_plugin: Option<String>,

    // Session TTL
    pub session_ttl_secs: u64,
}

impl Config {
    pub fn from_env() -> Result<Self, String> {
        Ok(Self {
            slack_bot_token: required("SLACK_BOT_TOKEN")?,
            slack_app_token: required("SLACK_APP_TOKEN")?,
            // Vertex AI fields are optional when using ANTHROPIC_BASE_URL
            vertex_project_id: env::var("VERTEX_PROJECT_ID").unwrap_or_default(),
            vertex_region: env::var("VERTEX_REGION").unwrap_or_else(|_| "us-east5".into()),
            vertex_model: env::var("VERTEX_MODEL")
                .or_else(|_| env::var("ANTHROPIC_DEFAULT_SONNET_MODEL"))
                .unwrap_or_else(|_| "claude-sonnet-4-6".into()),
            google_application_credentials: env::var("GOOGLE_APPLICATION_CREDENTIALS").ok(),
            anthropic_base_url: env::var("ANTHROPIC_BASE_URL").ok().filter(|s| !s.is_empty()),
            anthropic_api_key: env::var("ANTHROPIC_API_KEY").ok().filter(|s| !s.is_empty()),
            plugin_dirs: env::var("PLUGIN_DIRS")
                .unwrap_or_else(|_| {
                    let home = env::var("HOME").unwrap_or_else(|_| "~".into());
                    format!("{home}/.agents/skills:{home}/claude-plugins/plugins")
                })
                .split(':')
                .map(|s| shellexpand::tilde(s).into_owned())
                .collect(),
            default_plugin: env::var("DEFAULT_PLUGIN").ok().filter(|s| !s.is_empty()),
            session_ttl_secs: env::var("SESSION_TTL_SECS")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(1800),
        })
    }

    pub fn vertex_endpoint(&self) -> String {
        format!(
            "https://{region}-aiplatform.googleapis.com/v1/projects/{project}/locations/{region}\
             /publishers/anthropic/models/{model}:streamRawPredict",
            region = self.vertex_region,
            project = self.vertex_project_id,
            model = self.vertex_model,
        )
    }
}

fn required(key: &str) -> Result<String, String> {
    env::var(key).map_err(|_| format!("Missing required env var: {key}"))
}

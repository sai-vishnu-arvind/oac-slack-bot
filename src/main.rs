mod config;
mod metrics;
mod slack;
mod claude;
mod plugins;

use std::sync::Arc;
use tokio::sync::Mutex;

use config::Config;
use claude::client::ClaudeClient;
use claude::session::SessionStore;
use metrics::Metrics;
use plugins::registry::PluginRegistry;
use slack::client::SlackClient;
use tracing::info;

#[tokio::main]
async fn main() {
    // Logging
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "oac_slack_bot=debug,info".into()),
        )
        .init();

    // Load .env (ignore if not present — prod uses real env vars)
    let _ = dotenvy::dotenv();

    // Config
    let config = match Config::from_env() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Config error: {e}");
            std::process::exit(1);
        }
    };

    info!(
        vertex_project = %config.vertex_project_id,
        vertex_region = %config.vertex_region,
        vertex_model = %config.vertex_model,
        plugin_dirs = ?config.plugin_dirs,
        default_plugin = ?config.default_plugin,
        "OAC Slack Bot starting"
    );

    // Build shared state
    let registry = Arc::new(PluginRegistry::load(&config.plugin_dirs));
    let claude = Arc::new(ClaudeClient::new(config.clone()));
    let sessions = Arc::new(Mutex::new(SessionStore::new(500, config.session_ttl_secs)));
    let slack = Arc::new(SlackClient::new(&config.slack_bot_token));
    let metrics = Arc::new(Metrics::new());

    // Periodic session cleanup
    let sessions_cleanup = Arc::clone(&sessions);
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(tokio::time::Duration::from_secs(300));
        loop {
            tick.tick().await;
            sessions_cleanup.lock().await.cleanup();
        }
    });

    // Periodic metrics summary (every 5 minutes)
    let metrics_log = Arc::clone(&metrics);
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(tokio::time::Duration::from_secs(300));
        loop {
            tick.tick().await;
            metrics_log.log_summary().await;
        }
    });

    let app_token = config.slack_app_token.clone();
    let default_plugin = config.default_plugin.clone();

    info!("Connecting to Slack Socket Mode...");
    slack::socket::run(
        slack,
        claude,
        sessions,
        registry,
        metrics,
        app_token,
        default_plugin,
    )
    .await;
}

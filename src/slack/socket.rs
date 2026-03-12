use std::sync::Arc;
use tokio::sync::Mutex;

use futures_util::{SinkExt, StreamExt};
use tokio_tungstenite::{connect_async, tungstenite::Message};
use tracing::{debug, error, info, warn};

use super::client::SlackClient;
use super::events::{handle_mention, handle_reaction};
use super::types::{Envelope, EventsPayload};
use crate::claude::client::ClaudeClient;
use crate::claude::session::SessionStore;
use crate::metrics::Metrics;
use crate::plugins::registry::PluginRegistry;

/// Connect to Slack Socket Mode and handle events. Auto-reconnects on failure.
pub async fn run(
    slack: Arc<SlackClient>,
    claude: Arc<ClaudeClient>,
    sessions: Arc<Mutex<SessionStore>>,
    registry: Arc<PluginRegistry>,
    metrics: Arc<Metrics>,
    app_token: String,
    default_plugin: Option<String>,
) {
    loop {
        match connect_once(
            &slack,
            &claude,
            &sessions,
            &registry,
            &metrics,
            &app_token,
            &default_plugin,
        )
        .await
        {
            Ok(()) => info!("Socket Mode connection closed — reconnecting"),
            Err(e) => error!(error = %e, "Socket Mode error — reconnecting in 5s"),
        }
        tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;
    }
}

async fn connect_once(
    slack: &Arc<SlackClient>,
    claude: &Arc<ClaudeClient>,
    sessions: &Arc<Mutex<SessionStore>>,
    registry: &Arc<PluginRegistry>,
    metrics: &Arc<Metrics>,
    app_token: &str,
    default_plugin: &Option<String>,
) -> Result<(), String> {
    let ws_url = slack.connections_open(app_token).await?;
    info!(url = %ws_url, "Connecting to Socket Mode");

    let (ws_stream, _) = connect_async(&ws_url)
        .await
        .map_err(|e| format!("WebSocket connect failed: {e}"))?;

    info!("Socket Mode connected");

    let (mut write, mut read) = ws_stream.split();

    while let Some(msg_result) = read.next().await {
        let msg = match msg_result {
            Ok(m) => m,
            Err(e) => {
                warn!(error = %e, "WebSocket read error");
                break;
            }
        };

        match msg {
            Message::Text(text) => {
                let envelope: Envelope = match serde_json::from_str(&text) {
                    Ok(e) => e,
                    Err(e) => {
                        debug!(error = %e, raw = %text, "Failed to parse envelope");
                        continue;
                    }
                };

                // Always ACK immediately before doing any work.
                let ack = serde_json::json!({ "envelope_id": envelope.envelope_id });
                if let Err(e) = write.send(Message::Text(ack.to_string().into())).await {
                    warn!(error = %e, "Failed to send ACK");
                }

                match envelope.envelope_type.as_str() {
                    "hello" => {
                        info!("Socket Mode handshake complete");
                    }
                    "disconnect" => {
                        info!("Server requested disconnect — reconnecting");
                        break;
                    }
                    "events_api" => {
                        if let Some(ref p) = envelope.payload {
                            let event_type = p.pointer("/event/type")
                                .and_then(|v| v.as_str())
                                .unwrap_or("unknown");
                            debug!(event_type = %event_type, "Received events_api envelope");
                        }
                        if let Some(payload) = envelope.payload {
                            let slack = Arc::clone(slack);
                            let claude = Arc::clone(claude);
                            let sessions = Arc::clone(sessions);
                            let registry = Arc::clone(registry);
                            let metrics = Arc::clone(metrics);
                            let default_plugin = default_plugin.clone();
                            tokio::spawn(async move {
                                dispatch_event(
                                    payload,
                                    slack,
                                    claude,
                                    sessions,
                                    registry,
                                    metrics,
                                    default_plugin,
                                )
                                .await;
                            });
                        }
                    }
                    other => {
                        debug!(envelope_type = %other, "Unhandled envelope type");
                    }
                }
            }
            Message::Ping(data) => {
                let _ = write.send(Message::Pong(data)).await;
            }
            Message::Close(_) => {
                info!("WebSocket closed by server");
                break;
            }
            _ => {}
        }
    }

    Ok(())
}

async fn dispatch_event(
    payload: serde_json::Value,
    slack: Arc<SlackClient>,
    claude: Arc<ClaudeClient>,
    sessions: Arc<Mutex<SessionStore>>,
    registry: Arc<PluginRegistry>,
    metrics: Arc<Metrics>,
    default_plugin: Option<String>,
) {
    let events_payload: EventsPayload = match serde_json::from_value(payload) {
        Ok(p) => p,
        Err(e) => {
            debug!(error = %e, "Failed to parse events payload");
            return;
        }
    };

    let event = match events_payload.event {
        Some(e) => e,
        None => return,
    };

    match event.event_type.as_str() {
        "app_mention" => {
            // Ignore bot's own messages (but NOT for other event types like reactions).
            if event.bot_id.is_some() {
                return;
            }
            handle_mention(
                event,
                slack,
                claude,
                sessions,
                registry,
                metrics,
                default_plugin,
            )
            .await;
        }
        "reaction_added" => {
            handle_reaction(event, metrics).await;
        }
        other => {
            debug!(event_type = %other, "Ignoring event");
        }
    }
}

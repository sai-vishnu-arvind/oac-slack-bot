use std::sync::Arc;
use std::time::Instant;
use tokio::sync::Mutex;
use tracing::{info, warn};

use super::client::SlackClient;
use super::format::md_to_slack;
use super::streamer::stream_to_slack;
use super::types::{SlackEvent, ThreadMessage};
use crate::claude::client::ClaudeClient;
use crate::claude::session::SessionStore;
use crate::claude::types::Message;
use crate::metrics::Metrics;
use crate::plugins::executor::execute_plugin;
use crate::plugins::registry::PluginRegistry;
use crate::plugins::router::route;

// ── Mention handler ─────────────────────────────────────────────────────────

/// Called for every `app_mention` event.
/// Runs the full plugin-routing + Claude-streaming flow.
pub async fn handle_mention(
    event: SlackEvent,
    slack: Arc<SlackClient>,
    claude: Arc<ClaudeClient>,
    sessions: Arc<Mutex<SessionStore>>,
    registry: Arc<PluginRegistry>,
    metrics: Arc<Metrics>,
    default_plugin: Option<String>,
) {
    let start = Instant::now();

    // 1. Extract channel.
    let channel = match &event.channel {
        Some(c) => c.clone(),
        None => return,
    };

    // 2. thread_ts: use existing thread, or fall back to the event's own ts.
    let thread_ts = event
        .thread_ts
        .clone()
        .or_else(|| event.ts.clone())
        .unwrap_or_default();

    let user = event.user.clone();

    // 3. Strip bot mention tokens and trim whitespace.
    let user_text = strip_mentions(event.text.as_deref().unwrap_or(""))
        .trim()
        .to_string();

    // Ignore empty messages.
    if user_text.is_empty() {
        return;
    }

    // Record mention in metrics.
    metrics.record_mention(user.as_deref(), &channel).await;

    // ── Built-in commands ────────────────────────────────────────────────

    // "stop" command
    if user_text.eq_ignore_ascii_case("stop") {
        let _ = slack
            .post_message(&channel, "🛑 Stopped.", Some(&thread_ts))
            .await;
        return;
    }

    // "stats" command
    if user_text.eq_ignore_ascii_case("stats") {
        let stats = metrics.format_stats().await;
        let _ = slack
            .post_message(&channel, &stats, Some(&thread_ts))
            .await;
        return;
    }

    info!(
        channel = %channel,
        thread_ts = %thread_ts,
        user = ?event.user,
        text = %user_text,
        "Handling app_mention"
    );

    // 4. Resolve external references in the message.
    let user_text = resolve_slack_urls(&user_text, &slack).await;
    let user_text = resolve_devrev_tickets(&user_text).await;

    // 5. Fetch thread history from Slack (source of truth) and build context.
    let thread_msgs = match slack.thread_history(&channel, &thread_ts).await {
        Ok(r) => r.messages.unwrap_or_default(),
        Err(e) => {
            warn!(error = %e, "Failed to fetch thread history");
            vec![]
        }
    };

    let thread_context = build_thread_context(&thread_msgs, event.ts.as_deref());

    // 6. Build session key and record user message (keeps session store in sync).
    let session_key = SessionStore::key(&channel, event.thread_ts.as_deref());
    {
        let mut store = sessions.lock().await;
        store
            .get_or_create(&session_key)
            .push(Message::user(&user_text));
    }

    // 7. Route to a plugin.
    let matched_plugin_fqn: Option<String> = if default_plugin.is_some() {
        default_plugin
    } else {
        route(&user_text, &registry).map(|p| p.fqn.clone())
    };

    if let Some(plugin_fqn) = matched_plugin_fqn {
        // ── Plugin path ─────────────────────────────────────────────────
        let plugin = match registry.get(&plugin_fqn) {
            Some(p) => p,
            None => {
                warn!(plugin = %plugin_fqn, "Plugin not found in registry");
                metrics.record_error();
                return;
            }
        };

        metrics.record_plugin_call(&plugin.fqn).await;

        // Post a "running plugin" indicator.
        if let Err(e) = slack
            .post_message(
                &channel,
                &format!("🔍 Running plugin: `{}`...", plugin.fqn),
                Some(&thread_ts),
            )
            .await
        {
            warn!(error = %e, "Failed to post plugin indicator");
        }

        // Enrich the query with thread context.
        let enriched_query = if thread_context.is_empty() {
            user_text.clone()
        } else {
            format!(
                "<thread_history>\n{}\n</thread_history>\n\n{}",
                thread_context, user_text
            )
        };

        // Execute the plugin with a single-turn call.
        let result =
            execute_plugin(plugin, &enriched_query, &[], &claude, &registry, 0).await;

        let reply_text = match &result {
            Ok(text) if text.is_empty() => "(no response from plugin)".to_string(),
            Ok(text) => {
                info!(len = text.len(), "Plugin returned response");
                text.clone()
            }
            Err(e) => {
                warn!(error = %e, "Plugin execution failed");
                metrics.record_error();
                format!("❌ Plugin error: {e}")
            }
        };

        let slack_text = md_to_slack(&reply_text);
        let post_resp = slack
            .post_message(&channel, &slack_text, Some(&thread_ts))
            .await;

        // Register bot message for reaction tracking.
        if let Ok(resp) = &post_resp {
            if let Some(ts) = &resp.ts {
                metrics
                    .register_bot_message(
                        &channel,
                        ts,
                        &thread_ts,
                        Some(&plugin.fqn),
                        user.as_deref(),
                        &user_text,
                    )
                    .await;
            }
        }
        if let Err(e) = post_resp {
            warn!(error = %e, "Failed to post plugin result");
        }

        // Record timing.
        metrics.record_response_time(start.elapsed()).await;

        // Record assistant response in session store.
        let mut store = sessions.lock().await;
        store
            .get_or_create(&session_key)
            .push(Message::assistant(&reply_text));
    } else {
        // ── Streaming path (no plugin) ──────────────────────────────────

        // Build system prompt with thread context.
        let system = if thread_context.is_empty() {
            None
        } else {
            Some(format!(
                "Below is the conversation history from this Slack thread. \
                 Use it as context to understand the ongoing discussion and \
                 provide a relevant response to the latest message.\n\n\
                 <thread_history>\n{}\n</thread_history>",
                thread_context
            ))
        };

        let messages = vec![Message::user(&user_text)];

        let rx = match claude.stream(messages, system.as_deref(), None).await {
            Ok(rx) => rx,
            Err(e) => {
                warn!(error = %e, "Claude stream failed to start");
                metrics.record_error();
                let _ = slack
                    .post_message(&channel, &format!("❌ Error: {e}"), Some(&thread_ts))
                    .await;
                return;
            }
        };

        let final_text = match stream_to_slack(
            rx,
            Arc::clone(&slack),
            channel.clone(),
            thread_ts.clone(),
            Arc::clone(&metrics),
            user.clone(),
            user_text.clone(),
        )
        .await
        {
            Ok(t) => t,
            Err(e) => {
                warn!(error = %e, "stream_to_slack failed");
                metrics.record_error();
                return;
            }
        };

        // Record timing.
        metrics.record_response_time(start.elapsed()).await;

        // Record assistant response in session store.
        if !final_text.is_empty() {
            let mut store = sessions.lock().await;
            store
                .get_or_create(&session_key)
                .push(Message::assistant(&final_text));
        }
    }
}

// ── Reaction handler ────────────────────────────────────────────────────────

/// Called for every `reaction_added` event.
/// Handles 👍/👎 feedback tracking.
pub async fn handle_reaction(event: SlackEvent, metrics: Arc<Metrics>) {
    let reaction = match &event.reaction {
        Some(r) => r.as_str(),
        None => return,
    };
    let item = match &event.item {
        Some(i) => i,
        None => return,
    };
    let item_channel = match &item.channel {
        Some(c) => c.as_str(),
        None => return,
    };
    let item_ts = match &item.ts {
        Some(t) => t.as_str(),
        None => return,
    };
    let user = event.user.as_deref().unwrap_or("unknown");

    metrics
        .record_reaction(item_channel, item_ts, reaction, user)
        .await;
}

// ── Helpers ─────────────────────────────────────────────────────────────────

/// Parse Slack thread URLs from a message and fetch their content.
async fn resolve_slack_urls(message: &str, slack: &SlackClient) -> String {
    let urls = extract_slack_urls(message);
    if urls.is_empty() {
        return message.to_string();
    }

    let mut context_parts: Vec<String> = Vec::new();

    for (channel_id, thread_ts) in &urls {
        info!(channel = %channel_id, ts = %thread_ts, "Fetching linked Slack thread");
        match slack.thread_history(channel_id, thread_ts).await {
            Ok(resp) => {
                if let Some(messages) = resp.messages {
                    let mut thread_text = String::new();
                    for msg in &messages {
                        let user = msg.user.as_deref().unwrap_or("unknown");
                        let text = msg.text.as_deref().unwrap_or("");
                        if !text.is_empty() {
                            thread_text.push_str(&format!("[{}]: {}\n", user, text));
                        }
                    }
                    if !thread_text.is_empty() {
                        context_parts.push(format!(
                            "--- Slack thread (#{}, ts: {}) ---\n{}---",
                            channel_id, thread_ts, thread_text
                        ));
                    }
                }
            }
            Err(e) => {
                warn!(channel = %channel_id, ts = %thread_ts, error = %e, "Failed to fetch linked thread");
            }
        }
    }

    if context_parts.is_empty() {
        return message.to_string();
    }

    format!(
        "{}\n\nUser's question: {}",
        context_parts.join("\n\n"),
        message
    )
}

fn extract_slack_urls(message: &str) -> Vec<(String, String)> {
    let mut results = Vec::new();
    let message = message.replace(['<', '>'], " ");

    for word in message.split_whitespace() {
        if !word.contains(".slack.com/archives/") {
            continue;
        }

        let parts: Vec<&str> = word.split('/').collect();
        if let Some(archives_idx) = parts.iter().position(|&p| p == "archives") {
            if archives_idx + 2 < parts.len() {
                let channel_id = parts[archives_idx + 1].to_string();
                let raw_ts = parts[archives_idx + 2];
                let raw_ts = raw_ts.split('?').next().unwrap_or(raw_ts);

                if let Some(ts_digits) = raw_ts.strip_prefix('p') {
                    if ts_digits.len() >= 7 {
                        let dot_pos = ts_digits.len() - 6;
                        let thread_ts =
                            format!("{}.{}", &ts_digits[..dot_pos], &ts_digits[dot_pos..]);
                        results.push((channel_id, thread_ts));
                    }
                }
            }
        }
    }

    results
}

async fn resolve_devrev_tickets(message: &str) -> String {
    let token = match std::env::var("DEVREV_TOKEN") {
        Ok(t) if !t.is_empty() => t,
        _ => return message.to_string(),
    };

    let ticket_ids = extract_devrev_ticket_ids(message);
    if ticket_ids.is_empty() {
        return message.to_string();
    }

    let http = reqwest::Client::new();
    let mut context_parts: Vec<String> = Vec::new();

    for ticket_id in &ticket_ids {
        info!(ticket = %ticket_id, "Fetching DevRev ticket");
        match fetch_devrev_ticket(&http, &token, ticket_id).await {
            Ok(content) => context_parts.push(content),
            Err(e) => {
                warn!(ticket = %ticket_id, error = %e, "Failed to fetch DevRev ticket");
            }
        }
    }

    if context_parts.is_empty() {
        return message.to_string();
    }

    format!(
        "{}\n\nUser's question: {}",
        context_parts.join("\n\n"),
        message
    )
}

fn extract_devrev_ticket_ids(message: &str) -> Vec<String> {
    let mut results = Vec::new();
    let message = message.replace(['<', '>'], " ");

    for word in message.split_whitespace() {
        let upper = word.to_uppercase();
        let cleaned = upper.trim_matches(|c: char| !c.is_alphanumeric() && c != '-');

        if (cleaned.starts_with("TKT-") || cleaned.starts_with("ISS-")) && cleaned.len() > 4 {
            results.push(cleaned.to_string());
            continue;
        }

        if word.contains("devrev.ai") {
            for prefix in &["/works/", "/issue/", "/ticket/"] {
                if let Some(pos) = word.find(prefix) {
                    let after = &word[pos + prefix.len()..];
                    let id = after
                        .split(|c: char| !c.is_alphanumeric() && c != '-')
                        .next()
                        .unwrap_or("")
                        .to_uppercase();
                    if (id.starts_with("TKT-") || id.starts_with("ISS-")) && id.len() > 4 {
                        results.push(id);
                    }
                    break;
                }
            }
        }
    }

    results.sort();
    results.dedup();
    results
}

async fn fetch_devrev_ticket(
    http: &reqwest::Client,
    token: &str,
    ticket_id: &str,
) -> Result<String, String> {
    let resp = http
        .get("https://api.devrev.ai/works.get")
        .header("Authorization", token)
        .query(&[("id", ticket_id)])
        .send()
        .await
        .map_err(|e| format!("DevRev request failed: {e}"))?;

    let status = resp.status();
    let body: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| format!("DevRev response parse failed: {e}"))?;

    if !status.is_success() {
        return Err(format!(
            "DevRev returned {}: {}",
            status,
            body.get("message")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown error")
        ));
    }

    let work = &body["work"];
    let title = work.get("title").and_then(|v| v.as_str()).unwrap_or("No title");
    let description = work
        .get("body")
        .and_then(|v| v.as_str())
        .unwrap_or("No description");
    let stage = work
        .pointer("/stage/name")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown");
    let priority = work
        .get("priority")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown");
    let created_by = work
        .pointer("/created_by/display_name")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown");

    Ok(format!(
        "--- DevRev Ticket {} ---\nTitle: {}\nStage: {}\nPriority: {}\nCreated by: {}\nDescription:\n{}\n---",
        ticket_id, title, stage, priority, created_by, description
    ))
}

fn build_thread_context(messages: &[ThreadMessage], exclude_ts: Option<&str>) -> String {
    let mut parts: Vec<String> = Vec::new();

    for msg in messages {
        if let (Some(ts), Some(exc)) = (msg.ts.as_deref(), exclude_ts) {
            if ts == exc {
                continue;
            }
        }

        let text = msg.text.as_deref().unwrap_or("").trim();
        if text.is_empty() {
            continue;
        }

        if is_bot_indicator(text) {
            continue;
        }

        let sender = if msg.bot_id.is_some() {
            "assistant"
        } else {
            msg.user.as_deref().unwrap_or("unknown")
        };

        parts.push(format!("[{}]: {}", sender, text));
    }

    parts.join("\n")
}

fn is_bot_indicator(text: &str) -> bool {
    text.starts_with("⏳")
        || text.starts_with("🔍 Running plugin")
        || text == "🛑 Stopped."
        || text.starts_with("❌ Error:")
        || text.starts_with("❌ Plugin error:")
}

fn strip_mentions(text: &str) -> String {
    let mut result = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();

    while let Some(c) = chars.next() {
        if c == '<' && chars.peek() == Some(&'@') {
            while let Some(inner) = chars.next() {
                if inner == '>' {
                    break;
                }
            }
            if chars.peek() == Some(&' ') {
                chars.next();
            }
        } else {
            result.push(c);
        }
    }

    result
}

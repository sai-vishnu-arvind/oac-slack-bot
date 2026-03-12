use std::sync::Arc;
use std::time::Instant;
use tokio::sync::mpsc;
use tokio::time::{interval, Duration};
use tracing::warn;

use super::client::SlackClient;
use super::format::md_to_slack;
use crate::claude::types::StreamEvent;
use crate::metrics::Metrics;

const UPDATE_INTERVAL_MS: u64 = 800;
const MAX_SLACK_MSG_LEN: usize = 3900; // Slack limit is 4000, leave headroom

/// Stream StreamEvents into a Slack thread, updating a single message in place.
/// Returns the final complete text.
pub async fn stream_to_slack(
    mut rx: mpsc::Receiver<StreamEvent>,
    client: Arc<SlackClient>,
    channel: String,
    thread_ts: String,
    metrics: Arc<Metrics>,
    user: Option<String>,
    question: String,
) -> Result<String, String> {
    let stream_start = Instant::now();
    let mut first_token_recorded = false;

    // Post initial placeholder message and capture its ts.
    let initial_resp = client
        .post_message(&channel, "⏳ Thinking...", Some(&thread_ts))
        .await
        .map_err(|e| format!("Failed to post initial message: {e}"))?;

    let mut current_ts = match initial_resp.ts {
        Some(ts) => ts,
        None => return Err("post_message returned no ts".into()),
    };

    // Register the initial bot message for reaction attribution.
    metrics
        .register_bot_message(
            &channel,
            &current_ts,
            &thread_ts,
            None,
            user.as_deref(),
            &question,
        )
        .await;

    let mut full_text = String::new();
    let mut last_sent_text = "⏳ Thinking...".to_string();
    let mut done = false;

    // Ticker that fires every UPDATE_INTERVAL_MS milliseconds.
    let mut tick = interval(Duration::from_millis(UPDATE_INTERVAL_MS));
    // Consume the immediate first tick so we don't update before any text arrives.
    tick.tick().await;

    loop {
        tokio::select! {
            // Receive next StreamEvent (biased so events drain before tick fires).
            biased;
            msg = rx.recv() => {
                match msg {
                    None => {
                        // Channel closed without Done — treat as done.
                        done = true;
                    }
                    Some(StreamEvent::Text(chunk)) => {
                        // Record time-to-first-token.
                        if !first_token_recorded {
                            first_token_recorded = true;
                            metrics
                                .record_first_token_time(stream_start.elapsed())
                                .await;
                        }
                        full_text.push_str(&chunk);
                    }
                    Some(StreamEvent::Done) => {
                        done = true;
                    }
                    Some(StreamEvent::Error(e)) => {
                        let error_text = format!("❌ Error: {e}");
                        let _ = client
                            .update_message(&channel, &current_ts, &error_text)
                            .await;
                        metrics.record_error();
                        return Err(e);
                    }
                    Some(StreamEvent::Usage { input_tokens, output_tokens }) => {
                        metrics.record_tokens(input_tokens, output_tokens);
                    }
                    Some(StreamEvent::ToolUse(_)) => {
                        // Tool use events are not expected in this streaming path;
                        // ignore them silently.
                    }
                }

                if done {
                    // Final update: use the full accumulated text (remove placeholder if empty).
                    let final_text = if full_text.is_empty() {
                        "(no response)".to_string()
                    } else {
                        full_text.clone()
                    };

                    // Handle the case where we need to split into multiple messages.
                    let _ = flush_text(
                        &final_text,
                        &mut last_sent_text,
                        &client,
                        &channel,
                        &thread_ts,
                        current_ts,
                        &metrics,
                        user.as_deref(),
                        &question,
                    )
                    .await;

                    return Ok(full_text);
                }
            }

            // Ticker fires — push an incremental update.
            _ = tick.tick() => {
                if full_text != last_sent_text && !full_text.is_empty() {
                    current_ts = flush_text(
                        &full_text,
                        &mut last_sent_text,
                        &client,
                        &channel,
                        &thread_ts,
                        current_ts,
                        &metrics,
                        user.as_deref(),
                        &question,
                    )
                    .await;
                }

                if done {
                    return Ok(full_text);
                }
            }
        }
    }
}

/// Update the current Slack message with `text`.
/// If `text` exceeds MAX_SLACK_MSG_LEN, split: update current message with the first
/// chunk and post a new continuation message, returning the new message's ts.
async fn flush_text(
    text: &str,
    last_sent: &mut String,
    client: &Arc<SlackClient>,
    channel: &str,
    thread_ts: &str,
    current_ts: String,
    metrics: &Arc<Metrics>,
    user: Option<&str>,
    question: &str,
) -> String {
    if text == last_sent {
        return current_ts;
    }

    // Convert Markdown → Slack mrkdwn before sending.
    let slack_text = md_to_slack(text);

    if slack_text.len() <= MAX_SLACK_MSG_LEN {
        // Simple in-place update.
        if let Err(e) = client.update_message(channel, &current_ts, &slack_text).await {
            warn!(error = %e, "Failed to update Slack message");
        }
        *last_sent = text.to_string();
        return current_ts;
    }

    // Text is too long — find a safe split point (on a character boundary, preferably whitespace).
    let split_at = find_split_point(&slack_text, MAX_SLACK_MSG_LEN);
    let first_part = &slack_text[..split_at];
    let remainder = slack_text[split_at..].trim_start();

    // Update the current message with the first part.
    if let Err(e) = client.update_message(channel, &current_ts, first_part).await {
        warn!(error = %e, "Failed to update Slack message (split part 1)");
    }

    // Post the remainder as a new thread reply and use its ts going forward.
    match client.post_message(channel, remainder, Some(thread_ts)).await {
        Ok(resp) => {
            *last_sent = text.to_string();
            let new_ts = resp.ts.unwrap_or(current_ts);
            // Register the continuation message for reaction tracking.
            metrics
                .register_bot_message(channel, &new_ts, thread_ts, None, user, question)
                .await;
            new_ts
        }
        Err(e) => {
            warn!(error = %e, "Failed to post continuation Slack message");
            *last_sent = text.to_string();
            current_ts
        }
    }
}

/// Find a good split point at or before `max_len` bytes, preferring whitespace.
fn find_split_point(text: &str, max_len: usize) -> usize {
    // Ensure we don't split in the middle of a multi-byte character.
    let safe_end = text
        .char_indices()
        .map(|(i, _)| i)
        .filter(|&i| i <= max_len)
        .last()
        .unwrap_or(0);

    // Walk backwards from safe_end to find whitespace.
    let slice = &text[..safe_end];
    if let Some(ws_pos) = slice.rfind(|c: char| c.is_whitespace()) {
        if ws_pos > 0 {
            return ws_pos;
        }
    }

    // No whitespace found — hard split at the safe char boundary.
    safe_end
}

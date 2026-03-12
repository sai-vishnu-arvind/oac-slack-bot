use reqwest::Client;
use tracing::{debug, warn};

use super::types::{ConversationsRepliesResponse, PostMessage, PostMessageResponse, UpdateMessage};

#[derive(Clone)]
pub struct SlackClient {
    http: Client,
    bot_token: String,
}

impl SlackClient {
    pub fn new(bot_token: impl Into<String>) -> Self {
        Self {
            http: Client::new(),
            bot_token: bot_token.into(),
        }
    }

    /// Open a Socket Mode WebSocket connection and return the wss:// URL.
    pub async fn connections_open(&self, app_token: &str) -> Result<String, String> {
        let resp: serde_json::Value = self
            .http
            .post("https://slack.com/api/apps.connections.open")
            .bearer_auth(app_token)
            .send()
            .await
            .map_err(|e| format!("connections.open failed: {e}"))?
            .json()
            .await
            .map_err(|e| format!("connections.open parse failed: {e}"))?;

        if resp["ok"].as_bool() != Some(true) {
            return Err(format!(
                "connections.open error: {}",
                resp["error"].as_str().unwrap_or("unknown")
            ));
        }

        resp["url"]
            .as_str()
            .map(|s| s.to_string())
            .ok_or_else(|| "connections.open: missing url".into())
    }

    /// Fetch all messages in a thread. `thread_ts` is the parent message ts.
    pub async fn thread_history(
        &self,
        channel: &str,
        thread_ts: &str,
    ) -> Result<ConversationsRepliesResponse, String> {
        let resp = self
            .http
            .get("https://slack.com/api/conversations.replies")
            .bearer_auth(&self.bot_token)
            .query(&[("channel", channel), ("ts", thread_ts), ("limit", "50")])
            .send()
            .await
            .map_err(|e| format!("conversations.replies failed: {e}"))?
            .json::<ConversationsRepliesResponse>()
            .await
            .map_err(|e| format!("conversations.replies parse failed: {e}"))?;

        if !resp.ok {
            warn!(error = ?resp.error, "conversations.replies error");
        }

        Ok(resp)
    }

    /// Post a message into a thread (or channel root if thread_ts is None).
    pub async fn post_message(
        &self,
        channel: &str,
        text: &str,
        thread_ts: Option<&str>,
    ) -> Result<PostMessageResponse, String> {
        let body = PostMessage {
            channel,
            text,
            thread_ts,
        };

        let resp: PostMessageResponse = self
            .http
            .post("https://slack.com/api/chat.postMessage")
            .bearer_auth(&self.bot_token)
            .json(&body)
            .send()
            .await
            .map_err(|e| format!("chat.postMessage failed: {e}"))?
            .json()
            .await
            .map_err(|e| format!("chat.postMessage parse failed: {e}"))?;

        if !resp.ok {
            warn!(error = ?resp.error, "chat.postMessage error");
        }

        Ok(resp)
    }

    /// Update an existing message in place (used for streaming edits).
    pub async fn update_message(
        &self,
        channel: &str,
        ts: &str,
        text: &str,
    ) -> Result<PostMessageResponse, String> {
        let body = UpdateMessage { channel, ts, text };

        let resp: PostMessageResponse = self
            .http
            .post("https://slack.com/api/chat.update")
            .bearer_auth(&self.bot_token)
            .json(&body)
            .send()
            .await
            .map_err(|e| format!("chat.update failed: {e}"))?
            .json()
            .await
            .map_err(|e| format!("chat.update parse failed: {e}"))?;

        if !resp.ok {
            debug!(error = ?resp.error, "chat.update error");
        }

        Ok(resp)
    }
}

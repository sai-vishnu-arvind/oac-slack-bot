use serde::{Deserialize, Serialize};

// ── Socket Mode envelope ──────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct Envelope {
    pub envelope_id: String,
    #[serde(rename = "type")]
    pub envelope_type: String,
    pub payload: Option<serde_json::Value>,
}

// ── Events API payload ────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct EventsPayload {
    pub event: Option<SlackEvent>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct SlackEvent {
    #[serde(rename = "type")]
    pub event_type: String,
    pub channel: Option<String>,
    pub user: Option<String>,
    pub text: Option<String>,
    pub ts: Option<String>,
    pub thread_ts: Option<String>,
    pub bot_id: Option<String>,
    // Reaction events (reaction_added / reaction_removed)
    pub reaction: Option<String>,
    pub item: Option<ReactionItem>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct ReactionItem {
    #[serde(rename = "type")]
    pub item_type: Option<String>,
    pub channel: Option<String>,
    pub ts: Option<String>,
}

// ── conversations.replies ─────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct ConversationsRepliesResponse {
    pub ok: bool,
    pub messages: Option<Vec<ThreadMessage>>,
    pub error: Option<String>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct ThreadMessage {
    pub user: Option<String>,
    pub text: Option<String>,
    pub ts: Option<String>,
    pub bot_id: Option<String>,
}

// ── chat.postMessage / chat.update ───────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct PostMessageResponse {
    pub ok: bool,
    pub ts: Option<String>,
    pub error: Option<String>,
}

// ── Outgoing message to Slack ─────────────────────────────────────────────────

#[derive(Debug, Serialize)]
pub struct PostMessage<'a> {
    pub channel: &'a str,
    pub text: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thread_ts: Option<&'a str>,
}

#[derive(Debug, Serialize)]
pub struct UpdateMessage<'a> {
    pub channel: &'a str,
    pub ts: &'a str,
    pub text: &'a str,
}

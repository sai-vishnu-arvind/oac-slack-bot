use futures_util::StreamExt;
use reqwest::Client;
use serde_json::{json, Value};
use tokio::sync::mpsc;
use tracing::{debug, info, warn};

use super::auth::GcpAuth;
use super::types::{Message, StreamEvent, Tool, ToolCall};
use crate::config::Config;

/// Channel buffer size for StreamEvents.
const STREAM_CHANNEL_BUF: usize = 64;

// ── ClaudeClient ─────────────────────────────────────────────────────────────

pub struct ClaudeClient {
    http: Client,
    auth: GcpAuth,
    config: Config,
}

impl ClaudeClient {
    pub fn new(config: Config) -> Self {
        let credentials_path = config.google_application_credentials.clone();
        let resolved = credentials_path.or_else(super::auth::resolve_credentials_path);

        Self {
            http: Client::new(),
            auth: GcpAuth::new(resolved),
            config,
        }
    }

    /// Returns true if using Anthropic API / LiteLLM mode (vs Vertex AI).
    fn is_anthropic_mode(&self) -> bool {
        self.config.anthropic_base_url.is_some()
    }

    // ── Public API ───────────────────────────────────────────────────────────

    /// Stream a Claude response.
    pub async fn stream(
        &self,
        messages: Vec<Message>,
        system_prompt: Option<&str>,
        tools: Option<Vec<Tool>>,
    ) -> Result<mpsc::Receiver<StreamEvent>, String> {
        let tool_count = tools.as_ref().map(|t| t.len()).unwrap_or(0);
        let (url, body) = if self.is_anthropic_mode() {
            let base = self.config.anthropic_base_url.as_ref().unwrap();
            let url = format!("{}/v1/messages", base.trim_end_matches('/'));
            let body = self.build_anthropic_body(&messages, system_prompt, tools, true);
            (url, body)
        } else {
            let url = self.config.vertex_endpoint();
            let body = self.build_vertex_body(&messages, system_prompt, tools, true);
            (url, body)
        };

        let msg_count = messages.len();
        let sys_len = system_prompt.map(|s| s.len()).unwrap_or(0);
        let user_msg_preview: String = messages.last()
            .and_then(|m| match &m.content {
                super::types::MessageContent::Text(t) => Some(t.chars().take(200).collect()),
                _ => Some("[blocks]".to_string()),
            })
            .unwrap_or_default();

        info!(
            url = %url,
            msg_count,
            system_prompt_len = sys_len,
            tool_count,
            last_msg_preview = %user_msg_preview,
            "Claude request"
        );

        let mut req = self
            .http
            .post(&url)
            .header("Content-Type", "application/json");

        if self.is_anthropic_mode() {
            let api_key = self.config.anthropic_api_key.as_deref().unwrap_or("");
            req = req
                .header("x-api-key", api_key)
                .header("anthropic-version", "2023-06-01");
        } else {
            let token = self.auth.token().await?;
            req = req.bearer_auth(&token);
        }

        let response = req
            .json(&body)
            .send()
            .await
            .map_err(|e| format!("Claude request failed: {e}"))?;

        let status = response.status();
        if !status.is_success() {
            let body_text = response
                .text()
                .await
                .unwrap_or_else(|_| "<unreadable>".into());
            return Err(format!("Claude returned {status}: {body_text}"));
        }

        let (tx, rx) = mpsc::channel::<StreamEvent>(STREAM_CHANNEL_BUF);

        tokio::spawn(drive_sse(response, tx));

        Ok(rx)
    }

    /// Non-streaming call — collects all text and tool calls then returns them.
    pub async fn complete(
        &self,
        messages: Vec<Message>,
        system_prompt: Option<&str>,
        tools: Option<Vec<Tool>>,
    ) -> Result<(String, Vec<ToolCall>), String> {
        let req_tool_count = tools.as_ref().map(|t| t.len()).unwrap_or(0);
        let mut rx = self.stream(messages, system_prompt, tools).await?;

        let mut text_buf = String::new();
        let mut tool_calls: Vec<ToolCall> = Vec::new();

        while let Some(event) = rx.recv().await {
            match event {
                StreamEvent::Text(chunk) => text_buf.push_str(&chunk),
                StreamEvent::ToolUse(tc) => tool_calls.push(tc),
                StreamEvent::Usage { .. } => {} // consumed by callers that need it
                StreamEvent::Done => break,
                StreamEvent::Error(e) => return Err(e),
            }
        }

        // Log response summary
        let response_preview: String = text_buf.chars().take(500).collect();
        let tool_names: Vec<&str> = tool_calls.iter().map(|tc| tc.name.as_str()).collect();
        info!(
            response_len = text_buf.len(),
            tool_calls = tool_names.len(),
            tool_names = ?tool_names,
            response_preview = %response_preview,
            "Claude response"
        );

        Ok((text_buf, tool_calls))
    }

    // ── Private helpers ──────────────────────────────────────────────────────

    /// Build request body for Anthropic API / LiteLLM.
    fn build_anthropic_body(
        &self,
        messages: &[Message],
        system_prompt: Option<&str>,
        tools: Option<Vec<Tool>>,
        stream: bool,
    ) -> Value {
        let mut body = json!({
            "model": self.config.vertex_model,
            "max_tokens": 8192,
            "messages": messages,
            "stream": stream,
        });

        if let Some(sys) = system_prompt {
            body["system"] = json!(sys);
        }
        if let Some(t) = tools {
            body["tools"] = json!(t);
        }

        body
    }

    /// Build request body for Vertex AI.
    fn build_vertex_body(
        &self,
        messages: &[Message],
        system_prompt: Option<&str>,
        tools: Option<Vec<Tool>>,
        stream: bool,
    ) -> Value {
        let mut body = json!({
            "anthropic_version": "vertex-2023-10-16",
            "max_tokens": 8192,
            "messages": messages,
            "stream": stream,
        });

        if let Some(sys) = system_prompt {
            body["system"] = json!(sys);
        }
        if let Some(t) = tools {
            body["tools"] = json!(t);
        }

        body
    }
}

// ── SSE processing ────────────────────────────────────────────────────────────

/// State accumulated while a `tool_use` content block is being streamed.
#[derive(Default)]
struct ToolUseAccumulator {
    id: String,
    name: String,
    /// Raw JSON fragments from `input_json_delta` events, concatenated.
    json_buf: String,
}

/// Drives the SSE byte stream from a [`reqwest::Response`], parsing events and
/// forwarding [`StreamEvent`]s to `tx`.  Runs inside a spawned Tokio task.
async fn drive_sse(response: reqwest::Response, tx: mpsc::Sender<StreamEvent>) {
    let mut stream = response.bytes_stream();
    let mut raw = String::new();
    let mut tool_acc: Option<ToolUseAccumulator> = None;

    while let Some(chunk_result) = stream.next().await {
        let bytes = match chunk_result {
            Ok(b) => b,
            Err(e) => {
                let _ = tx
                    .send(StreamEvent::Error(format!("Stream read error: {e}")))
                    .await;
                return;
            }
        };

        let text = match std::str::from_utf8(&bytes) {
            Ok(s) => s,
            Err(e) => {
                let _ = tx
                    .send(StreamEvent::Error(format!("UTF-8 decode error: {e}")))
                    .await;
                return;
            }
        };
        raw.push_str(text);

        loop {
            let frame_end = match raw.find("\n\n") {
                Some(pos) => pos,
                None => break,
            };

            let frame = raw[..frame_end].to_string();
            raw.drain(..frame_end + 2);

            for line in frame.lines() {
                let data = match line.strip_prefix("data: ") {
                    Some(d) => d.trim(),
                    None => continue,
                };

                if data == "[DONE]" {
                    let _ = tx.send(StreamEvent::Done).await;
                    return;
                }

                let val: Value = match serde_json::from_str(data) {
                    Ok(v) => v,
                    Err(e) => {
                        warn!("SSE JSON parse error: {e} — skipping: {data}");
                        continue;
                    }
                };

                let event_type = match val.get("type").and_then(|v| v.as_str()) {
                    Some(t) => t,
                    None => continue,
                };

                match event_type {
                    "content_block_delta" => {
                        let delta = &val["delta"];
                        match delta.get("type").and_then(|v| v.as_str()) {
                            Some("text_delta") => {
                                if let Some(text) =
                                    delta.get("text").and_then(|v| v.as_str())
                                {
                                    if tx
                                        .send(StreamEvent::Text(text.to_string()))
                                        .await
                                        .is_err()
                                    {
                                        return;
                                    }
                                }
                            }
                            Some("input_json_delta") => {
                                if let Some(fragment) =
                                    delta.get("partial_json").and_then(|v| v.as_str())
                                {
                                    if let Some(acc) = tool_acc.as_mut() {
                                        acc.json_buf.push_str(fragment);
                                    }
                                }
                            }
                            _ => {}
                        }
                    }

                    "content_block_start" => {
                        let block = &val["content_block"];
                        if block.get("type").and_then(|v| v.as_str()) == Some("tool_use") {
                            tool_acc = Some(ToolUseAccumulator {
                                id: block
                                    .get("id")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or_default()
                                    .to_string(),
                                name: block
                                    .get("name")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or_default()
                                    .to_string(),
                                json_buf: String::new(),
                            });
                        }
                    }

                    "content_block_stop" => {
                        if let Some(acc) = tool_acc.take() {
                            let input: Value =
                                serde_json::from_str(&acc.json_buf).unwrap_or_else(|_| {
                                    if acc.json_buf.is_empty() {
                                        json!({})
                                    } else {
                                        json!({"_raw": acc.json_buf})
                                    }
                                });
                            let tc = ToolCall {
                                id: acc.id,
                                name: acc.name,
                                input,
                            };
                            if tx.send(StreamEvent::ToolUse(tc)).await.is_err() {
                                return;
                            }
                        }
                    }

                    // Token usage: message_start contains input_tokens,
                    // message_delta contains output_tokens.
                    "message_start" => {
                        let input = val
                            .pointer("/message/usage/input_tokens")
                            .and_then(|v| v.as_u64())
                            .unwrap_or(0);
                        if input > 0 {
                            let _ = tx
                                .send(StreamEvent::Usage {
                                    input_tokens: input,
                                    output_tokens: 0,
                                })
                                .await;
                        }
                    }

                    "message_delta" => {
                        let output = val
                            .pointer("/usage/output_tokens")
                            .and_then(|v| v.as_u64())
                            .unwrap_or(0);
                        if output > 0 {
                            let _ = tx
                                .send(StreamEvent::Usage {
                                    input_tokens: 0,
                                    output_tokens: output,
                                })
                                .await;
                        }
                    }

                    "message_stop" => {
                        let _ = tx.send(StreamEvent::Done).await;
                        return;
                    }

                    "error" => {
                        let msg = val
                            .pointer("/error/message")
                            .and_then(|v| v.as_str())
                            .unwrap_or("unknown Claude error");
                        let _ = tx
                            .send(StreamEvent::Error(format!("Claude error: {msg}")))
                            .await;
                        return;
                    }

                    _ => {
                        debug!(event_type, "Ignoring SSE event");
                    }
                }
            }
        }
    }

    let _ = tx.send(StreamEvent::Done).await;
}

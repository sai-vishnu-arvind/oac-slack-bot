use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::process::Stdio;

use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;

use crate::plugins::registry::Plugin;

/// Result from a CLI execution: the response text and optional session ID for resume.
pub struct CliResult {
    pub text: String,
    pub session_id: Option<String>,
}

// ── CLI-based executor ───────────────────────────────────────────────────────

/// Execute a plugin by spawning the `claude` CLI as a subprocess.
///
/// The CLI has full access to: Bash, Read, Grep, Glob, Agent, MCP servers
/// (Coralogix, Trino, Splitz, etc.), and all native Claude Code tools.
///
/// This replaces the old Vertex AI API call + 4 custom tools approach.
pub async fn execute_plugin_via_cli(
    plugin: &Plugin,
    query: &str,
    session_id: Option<&str>,
    mcp_config_path: Option<&str>,
) -> Result<CliResult, String> {
    let mut cmd = Command::new("claude");

    // Core flags — use stream-json to capture actual response text
    cmd.arg("--print")
        .arg("--dangerously-skip-permissions")
        .arg("--output-format").arg("stream-json")
        .arg("--verbose")
;

    // System prompt = the plugin's SKILL.md content
    cmd.arg("--system-prompt").arg(&plugin.system_prompt);

    // MCP server configuration
    if let Some(mcp_path) = mcp_config_path {
        let path = PathBuf::from(mcp_path);
        if path.exists() {
            cmd.arg("--mcp-config").arg(mcp_path);
        }
    }

    // Resume existing session for multi-turn conversations
    if let Some(sid) = session_id {
        cmd.arg("--resume").arg(sid);
    }

    // Pass query via stdin to avoid shell escaping issues
    // (enriched queries contain ---, DevRev content, URLs, etc.)
    cmd.arg("--print");

    cmd.stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    tracing::info!(
        fqn = %plugin.fqn,
        query_len = query.len(),
        has_session = session_id.is_some(),
        "Spawning claude CLI for plugin"
    );

    let mut child = cmd.spawn().map_err(|e| {
        format!("Failed to spawn claude CLI: {}. Is claude installed and in PATH?", e)
    })?;

    // Write query to stdin
    if let Some(mut stdin) = child.stdin.take() {
        use tokio::io::AsyncWriteExt;
        stdin.write_all(query.as_bytes()).await.map_err(|e| {
            format!("Failed to write query to claude CLI stdin: {}", e)
        })?;
        drop(stdin); // Close stdin to signal EOF
    }

    let output = child.wait_with_output().await.map_err(|e| {
        format!("Claude CLI process failed: {}", e)
    })?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    if !output.status.success() && stdout.is_empty() {
        tracing::error!(
            fqn = %plugin.fqn,
            exit_code = ?output.status.code(),
            stderr = %stderr,
            "Claude CLI exited with error"
        );
        return Err(format!("Claude CLI error (exit {}): {}",
            output.status.code().unwrap_or(-1), stderr));
    }

    // Parse stream-json output — each line is a JSON object
    // Collect text from "assistant" messages and metadata from "result" message
    let mut text = String::new();
    let mut cli_session_id: Option<String> = None;
    let mut cost: f64 = 0.0;
    let mut duration_ms: u64 = 0;

    for line in stdout.lines() {
        if line.is_empty() { continue; }
        let parsed: serde_json::Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(_) => continue,
        };

        let msg_type = parsed.get("type").and_then(|v| v.as_str()).unwrap_or("");

        match msg_type {
            "assistant" => {
                // Extract text from assistant message content blocks
                if let Some(content) = parsed.pointer("/message/content").and_then(|c| c.as_array()) {
                    for block in content {
                        if let Some(t) = block.get("text").and_then(|t| t.as_str()) {
                            text.push_str(t);
                        }
                    }
                }
            }
            "result" => {
                // Capture metadata from the final result line
                if let Some(r) = parsed.get("result").and_then(|v| v.as_str()) {
                    if !r.is_empty() && text.is_empty() {
                        text = r.to_string();
                    }
                }
                cli_session_id = parsed.get("session_id")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());
                cost = parsed.get("total_cost_usd")
                    .and_then(|v| v.as_f64())
                    .unwrap_or(0.0);
                duration_ms = parsed.get("duration_ms")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0);
            }
            _ => {}
        }
    }

    tracing::info!(
        fqn = %plugin.fqn,
        text_len = text.len(),
        session_id = ?cli_session_id,
        cost_usd = cost,
        duration_ms = duration_ms,
        "Claude CLI execution complete"
    );

    Ok(CliResult {
        text,
        session_id: cli_session_id,
    })
}

/// Execute a plugin by spawning the `claude` CLI with streaming JSON output.
///
/// Streams text chunks as they arrive, calling `on_text` for each chunk.
/// Returns the final result text and session ID.
pub async fn execute_plugin_via_cli_streaming<F>(
    plugin: &Plugin,
    query: &str,
    session_id: Option<&str>,
    mcp_config_path: Option<&str>,
    mut on_text: F,
) -> Result<CliResult, String>
where
    F: FnMut(&str) + Send,
{
    let mut cmd = Command::new("claude");

    // Core flags — use stream-json for streaming
    cmd.arg("--print")
        .arg("--dangerously-skip-permissions")
        .arg("--output-format").arg("stream-json")
        .arg("--verbose")
;

    // System prompt = the plugin's SKILL.md content
    cmd.arg("--system-prompt").arg(&plugin.system_prompt);

    // MCP server configuration
    if let Some(mcp_path) = mcp_config_path {
        let path = PathBuf::from(mcp_path);
        if path.exists() {
            cmd.arg("--mcp-config").arg(mcp_path);
        }
    }

    // Resume existing session
    if let Some(sid) = session_id {
        cmd.arg("--resume").arg(sid);
    }

    // Pass query via stdin (avoid shell escaping issues with enriched content)
    cmd.stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    tracing::info!(
        fqn = %plugin.fqn,
        query_len = query.len(),
        has_session = session_id.is_some(),
        "Spawning claude CLI (streaming) for plugin"
    );

    let mut child = cmd.spawn().map_err(|e| {
        format!("Failed to spawn claude CLI: {}. Is claude installed and in PATH?", e)
    })?;

    // Write query to stdin
    if let Some(mut stdin) = child.stdin.take() {
        use tokio::io::AsyncWriteExt;
        stdin.write_all(query.as_bytes()).await.map_err(|e| {
            format!("Failed to write query to claude CLI stdin: {}", e)
        })?;
        drop(stdin); // Close stdin to signal EOF
    }

    let stdout = child.stdout.take()
        .ok_or("Failed to capture claude CLI stdout")?;

    let mut reader = BufReader::new(stdout).lines();
    let mut full_text = String::new();
    let mut cli_session_id: Option<String> = None;

    // Read streaming JSON lines
    while let Some(line) = reader.next_line().await.map_err(|e| {
        format!("Error reading claude CLI output: {}", e)
    })? {
        if line.is_empty() {
            continue;
        }

        // Parse each JSON line
        let parsed: serde_json::Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(_) => continue, // Skip unparseable lines
        };

        let msg_type = parsed.get("type").and_then(|v| v.as_str()).unwrap_or("");

        match msg_type {
            "assistant" => {
                // Extract text from assistant message content blocks
                if let Some(message) = parsed.get("message") {
                    if let Some(content) = message.get("content").and_then(|c| c.as_array()) {
                        for block in content {
                            if let Some(text) = block.get("text").and_then(|t| t.as_str()) {
                                full_text.push_str(text);
                                on_text(text);
                            }
                        }
                    }
                }
            }
            "result" => {
                // Final result — capture session ID and result text
                if let Some(result_text) = parsed.get("result").and_then(|v| v.as_str()) {
                    if !result_text.is_empty() {
                        full_text = result_text.to_string();
                    }
                }
                cli_session_id = parsed.get("session_id")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());

                let cost = parsed.get("total_cost_usd")
                    .and_then(|v| v.as_f64())
                    .unwrap_or(0.0);

                tracing::info!(
                    fqn = %plugin.fqn,
                    text_len = full_text.len(),
                    session_id = ?cli_session_id,
                    cost_usd = cost,
                    "Claude CLI streaming complete"
                );
            }
            "system" => {
                // Capture session ID from init message
                let subtype = parsed.get("subtype").and_then(|v| v.as_str()).unwrap_or("");
                if subtype == "init" {
                    cli_session_id = parsed.get("session_id")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string());
                }
            }
            _ => {
                // Ignore other message types (tool_use, system hooks, etc.)
            }
        }
    }

    // Wait for the process to finish
    let status = child.wait().await.map_err(|e| {
        format!("Failed to wait for claude CLI: {}", e)
    })?;

    if !status.success() {
        tracing::warn!(
            fqn = %plugin.fqn,
            exit_code = ?status.code(),
            "Claude CLI exited with non-zero status (may still have output)"
        );
    }

    Ok(CliResult {
        text: full_text,
        session_id: cli_session_id,
    })
}

// ── Backward-compatible wrapper ──────────────────────────────────────────────

/// Execute a plugin — backward-compatible wrapper that matches the old signature.
///
/// This is called from events.rs. It delegates to `execute_plugin_via_cli`.
/// The `client` and `registry` params are kept for API compatibility but unused.
pub fn execute_plugin<'a>(
    plugin: &'a Plugin,
    query: &'a str,
    _history: &'a [crate::claude::types::Message],
    _client: &'a crate::claude::client::ClaudeClient,
    _registry: &'a crate::plugins::registry::PluginRegistry,
    _depth: u32,
) -> Pin<Box<dyn Future<Output = Result<String, String>> + Send + 'a>> {
    Box::pin(async move {
        // Resolve MCP config path relative to the executable
        let mcp_config = std::env::current_dir()
            .ok()
            .map(|d| d.join("mcp-servers.json"))
            .and_then(|p| if p.exists() { Some(p.to_string_lossy().to_string()) } else { None });

        let result = execute_plugin_via_cli(
            plugin,
            query,
            None, // No session resume in the backward-compatible path
            mcp_config.as_deref(),
        )
        .await?;

        Ok(result.text)
    })
}

// ── Unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cli_result_struct() {
        let result = CliResult {
            text: "Hello".to_string(),
            session_id: Some("abc-123".to_string()),
        };
        assert_eq!(result.text, "Hello");
        assert_eq!(result.session_id.unwrap(), "abc-123");
    }

    #[test]
    fn test_cli_result_no_session() {
        let result = CliResult {
            text: "Response".to_string(),
            session_id: None,
        };
        assert!(result.session_id.is_none());
    }
}

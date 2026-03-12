use std::future::Future;
use std::pin::Pin;

use futures_util::future::join_all;
use serde_json::json;

use crate::claude::client::ClaudeClient;
use crate::claude::types::{ContentBlock, Message, MessageContent, Role, Tool, ToolCall};
use crate::plugins::registry::{GetError, Plugin, PluginRegistry};

const MAX_DEPTH: u32 = 2;

// ── Tool definitions ──────────────────────────────────────────────────────────

pub fn invoke_plugin_tool() -> Tool {
    Tool {
        name: "invoke_plugin".into(),
        description: "Invoke a named plugin to handle a specialized task. \
                      Use when a subtask requires domain expertise from another plugin. \
                      Plugins can chain into other plugins."
            .into(),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "plugin_name": {
                    "type": "string",
                    "description": "Fully-qualified plugin name like 'second-brain:capture' \
                                   or 'backend-engineer:brainstorming'. Use bare name only \
                                   for ungrouped plugins like 'oncall-debugger'. \
                                   Use list_plugins or list_plugin_commands to discover names."
                },
                "query": {
                    "type": "string",
                    "description": "The question or task to pass to the plugin"
                }
            },
            "required": ["plugin_name", "query"]
        }),
    }
}

pub fn list_plugins_tool() -> Tool {
    Tool {
        name: "list_plugins".into(),
        description: "List all available plugins with their fully-qualified names, \
                      descriptions, and groups. Use when unsure which plugin handles a task."
            .into(),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {},
            "required": []
        }),
    }
}

pub fn list_plugin_commands_tool() -> Tool {
    Tool {
        name: "list_plugin_commands".into(),
        description: "List all sub-commands of a specific plugin group. \
                      Use when you know the group (e.g. 'second-brain') but need to \
                      discover which commands are available within it."
            .into(),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "group": {
                    "type": "string",
                    "description": "The plugin group name (e.g. 'second-brain', 'agent-ready', 'backend-engineer')"
                }
            },
            "required": ["group"]
        }),
    }
}

pub fn spawn_agents_tool() -> Tool {
    Tool {
        name: "spawn_agents".into(),
        description: "Spawn multiple plugins in parallel and wait for all results. \
                      Use this instead of sequential invoke_plugin calls when you need \
                      data from several sources simultaneously (e.g. logs + metrics + \
                      deployments + architecture all at once). \
                      Returns a JSON object mapping each plugin_name to its result."
            .into(),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "agents": {
                    "type": "array",
                    "description": "List of plugins to run in parallel",
                    "items": {
                        "type": "object",
                        "properties": {
                            "plugin_name": {
                                "type": "string",
                                "description": "Fully-qualified plugin name (e.g. 'second-brain:capture')"
                            },
                            "query": {
                                "type": "string",
                                "description": "The question or task for this plugin"
                            }
                        },
                        "required": ["plugin_name", "query"]
                    },
                    "minItems": 2
                }
            },
            "required": ["agents"]
        }),
    }
}

// ── Executor ──────────────────────────────────────────────────────────────────

/// Execute a plugin with the given query and conversation history.
/// Returns the plugin's final text response.
///
/// `depth` prevents infinite recursion — starts at 0, max is MAX_DEPTH (3).
pub fn execute_plugin<'a>(
    plugin: &'a Plugin,
    query: &'a str,
    history: &'a [Message],
    client: &'a ClaudeClient,
    registry: &'a PluginRegistry,
    depth: u32,
) -> Pin<Box<dyn Future<Output = Result<String, String>> + Send + 'a>> {
    Box::pin(async move {
        // 1. Build messages: history + user query
        let mut messages: Vec<Message> = history.to_vec();
        messages.push(Message::user(query));

        // 2. Build system prompt with runtime context about available tools
        let system_prompt = format!(
            "{}\n\n\
            ---\n\
            # RUNTIME CONTEXT (from bot runtime — takes priority over instructions above)\n\n\
            You are running inside a Slack bot, NOT inside Claude Code CLI.\n\
            You do NOT have access to: Bash, Read, Glob, Grep, file system, MCP servers \
            (Coralogix, Watchtower, Trino, Kubernetes, Google Drive, Slack MCP, GitHub MCP).\n\n\
            You ONLY have these 4 tools: invoke_plugin, list_plugins, list_plugin_commands, spawn_agents.\n\
            These tools call other SKILL.md-based plugins — NOT MCP servers.\n\n\
            If the user's message already contains fetched data (DevRev ticket details, Slack thread \
            content), analyze that data directly. Do NOT try to fetch it again.\n\n\
            Focus on providing the best analysis you can with the information already in the message. \
            Use invoke_plugin/spawn_agents ONLY if another SKILL.md plugin would genuinely add value \
            (use list_plugins first to see what's available). If no plugin helps, just analyze directly.\n\
            ---",
            plugin.system_prompt
        );

        // 3. For top-level plugin (depth 0), skip tools entirely.
        //    The enriched message already has DevRev/Slack data — just analyze it.
        //    Sub-plugins (depth > 0) get tools for chaining.
        if depth == 0 {
            let (text, _) = client
                .complete(messages, Some(&system_prompt), None)
                .await?;

            tracing::info!(
                fqn = %plugin.fqn,
                text_len = text.len(),
                "Top-level plugin response (no tools)"
            );

            return Ok(text);
        }

        // 4. Sub-plugin: define tools and run multi-turn loop
        let tools = vec![
            invoke_plugin_tool(),
            list_plugins_tool(),
            list_plugin_commands_tool(),
            spawn_agents_tool(),
        ];
        let sub_max_rounds: usize = 2;
        let mut last_text = String::new();

        for round in 0..sub_max_rounds {
            let round_tools = if round == sub_max_rounds - 1 {
                None
            } else {
                Some(tools.clone())
            };

            let (text, tool_calls) = client
                .complete(
                    messages.clone(),
                    Some(&system_prompt),
                    round_tools,
                )
                .await?;

            tracing::info!(
                fqn = %plugin.fqn,
                round = round,
                text_len = text.len(),
                tool_call_count = tool_calls.len(),
                tool_names = ?tool_calls.iter().map(|tc| tc.name.as_str()).collect::<Vec<_>>(),
                "Claude call result"
            );

            if !text.is_empty() {
                last_text = text.clone();
            }

            // No tool calls — we're done. Return this round's text,
            // or fall back to the best text from earlier rounds.
            if tool_calls.is_empty() {
                if text.is_empty() {
                    return Ok(last_text);
                }
                return Ok(text);
            }

            // Dispatch each tool call
            let mut results: Vec<(String, String)> = Vec::new();
            for tc in &tool_calls {
                tracing::debug!(tool = %tc.name, input = %tc.input, "Dispatching tool call");
                let tool_result = dispatch_tool_call(tc, client, registry, depth).await;
                tracing::debug!(tool = %tc.name, result_len = tool_result.len(), "Tool call result");
                results.push((tc.id.clone(), tool_result));
            }

            // Append assistant message with tool-use blocks + any text
            let mut assistant_blocks: Vec<ContentBlock> = Vec::new();
            if !text.is_empty() {
                assistant_blocks.push(ContentBlock::Text { text });
            }
            for tc in &tool_calls {
                assistant_blocks.push(ContentBlock::ToolUse {
                    id: tc.id.clone(),
                    name: tc.name.clone(),
                    input: tc.input.clone(),
                });
            }
            messages.push(Message {
                role: Role::Assistant,
                content: MessageContent::Blocks(assistant_blocks),
            });

            // Append user message with tool results
            let result_blocks: Vec<ContentBlock> = results
                .iter()
                .map(|(id, content)| ContentBlock::ToolResult {
                    tool_use_id: id.clone(),
                    content: content.clone(),
                })
                .collect();
            messages.push(Message {
                role: Role::User,
                content: MessageContent::Blocks(result_blocks),
            });
        }

        // Hit the round limit — return whatever text we have
        tracing::warn!(fqn = %plugin.fqn, "Hit max tool rounds ({})", sub_max_rounds);
        Ok(last_text)
    })
}

// ── Tool dispatch helper ──────────────────────────────────────────────────────

/// Resolve a single tool call to a result string.
async fn dispatch_tool_call(
    tc: &ToolCall,
    client: &ClaudeClient,
    registry: &PluginRegistry,
    depth: u32,
) -> String {
    match tc.name.as_str() {
        "invoke_plugin" => {
            let plugin_name = match tc.input.get("plugin_name").and_then(|v| v.as_str()) {
                Some(n) => n,
                None => return "invoke_plugin: missing 'plugin_name' field".to_string(),
            };
            let sub_query = match tc.input.get("query").and_then(|v| v.as_str()) {
                Some(q) => q,
                None => return "invoke_plugin: missing 'query' field".to_string(),
            };

            if depth >= MAX_DEPTH {
                return "[Plugin depth limit reached]".to_string();
            }

            match registry.get_or_ambiguous(plugin_name) {
                Ok(sub_plugin) => {
                    match execute_plugin(sub_plugin, sub_query, &[], client, registry, depth + 1)
                        .await
                    {
                        Ok(result) => result,
                        Err(e) => format!("Plugin '{}' error: {}", plugin_name, e),
                    }
                }
                Err(GetError::NotFound) => {
                    let available: Vec<String> =
                        registry.list().into_iter().map(|(fqn, _, _)| fqn).collect();
                    format!(
                        "Plugin '{}' not found. Available: {:?}",
                        plugin_name, available
                    )
                }
                Err(GetError::Ambiguous(fqns)) => {
                    format!(
                        "Ambiguous plugin name '{}'. Use a fully-qualified name: {}",
                        plugin_name,
                        fqns.join(", ")
                    )
                }
            }
        }

        "list_plugins" => {
            let entries: Vec<serde_json::Value> = registry
                .list()
                .into_iter()
                .map(|(fqn, description, group)| {
                    json!({
                        "fqn": fqn,
                        "description": description,
                        "group": group
                    })
                })
                .collect();
            serde_json::to_string(&entries)
                .unwrap_or_else(|_| "[]".to_string())
        }

        "list_plugin_commands" => {
            let group = match tc.input.get("group").and_then(|v| v.as_str()) {
                Some(g) => g,
                None => return "list_plugin_commands: missing 'group' field".to_string(),
            };

            let commands = registry.get_group_commands(group);
            if commands.is_empty() {
                let groups = registry.groups();
                return format!(
                    "No commands found for group '{}'. Available groups: {:?}",
                    group, groups
                );
            }

            let entries: Vec<serde_json::Value> = commands
                .into_iter()
                .map(|p| {
                    json!({
                        "fqn": p.fqn,
                        "name": p.name,
                        "description": p.description
                    })
                })
                .collect();
            serde_json::to_string_pretty(&entries)
                .unwrap_or_else(|_| "[]".to_string())
        }

        "spawn_agents" => {
            spawn_agents_parallel(tc, client, registry, depth).await
        }

        unknown => format!("Unknown tool: {}", unknown),
    }
}

// ── Parallel agent spawning ───────────────────────────────────────────────────

/// Execute multiple plugins concurrently and return all results as a JSON object.
async fn spawn_agents_parallel(
    tc: &ToolCall,
    client: &ClaudeClient,
    registry: &PluginRegistry,
    depth: u32,
) -> String {
    if depth >= MAX_DEPTH {
        return "[Agent depth limit reached — cannot spawn further sub-agents]".to_string();
    }

    let agents_val = match tc.input.get("agents").and_then(|v| v.as_array()) {
        Some(a) => a.clone(),
        None => return "spawn_agents: missing or invalid 'agents' array".to_string(),
    };

    let mut tasks: Vec<(String, String, &Plugin)> = Vec::new();
    let mut errors: Vec<String> = Vec::new();

    for agent in &agents_val {
        let plugin_name = match agent.get("plugin_name").and_then(|v| v.as_str()) {
            Some(n) => n.to_string(),
            None => {
                errors.push("spawn_agents: agent missing 'plugin_name'".to_string());
                continue;
            }
        };
        let query = match agent.get("query").and_then(|v| v.as_str()) {
            Some(q) => q.to_string(),
            None => {
                errors.push(format!("spawn_agents: agent '{}' missing 'query'", plugin_name));
                continue;
            }
        };
        match registry.get_or_ambiguous(&plugin_name) {
            Ok(plugin) => tasks.push((plugin_name, query, plugin)),
            Err(GetError::NotFound) => {
                errors.push(format!("spawn_agents: plugin '{}' not found", plugin_name))
            }
            Err(GetError::Ambiguous(fqns)) => {
                errors.push(format!(
                    "spawn_agents: '{}' is ambiguous, use FQN: {}",
                    plugin_name,
                    fqns.join(", ")
                ))
            }
        }
    }

    // Run all valid plugins concurrently
    let futures = tasks.iter().map(|(name, query, plugin)| {
        let name = name.clone();
        async move {
            let result = execute_plugin(plugin, query, &[], client, registry, depth + 1).await;
            (name, result)
        }
    });

    let results = join_all(futures).await;

    let mut map = serde_json::Map::new();

    for err in errors {
        map.insert(
            format!("_error_{}", map.len()),
            serde_json::Value::String(err),
        );
    }

    for (name, result) in results {
        let value = match result {
            Ok(text) => serde_json::Value::String(text),
            Err(e) => serde_json::Value::String(format!("[error: {}]", e)),
        };
        map.insert(name, value);
    }

    serde_json::to_string_pretty(&serde_json::Value::Object(map))
        .unwrap_or_else(|_| "{}".to_string())
}

// ── Unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_invoke_plugin_tool_schema() {
        let tool = invoke_plugin_tool();
        assert_eq!(tool.name, "invoke_plugin");
        assert!(tool.input_schema["properties"]["plugin_name"].is_object());
        assert!(tool.input_schema["properties"]["query"].is_object());
        // Schema mentions FQN
        let plugin_name_desc = tool.input_schema["properties"]["plugin_name"]["description"]
            .as_str()
            .unwrap();
        assert!(plugin_name_desc.contains("Fully-qualified"));
    }

    #[test]
    fn test_list_plugins_tool_schema() {
        let tool = list_plugins_tool();
        assert_eq!(tool.name, "list_plugins");
        assert!(tool.description.contains("fully-qualified"));
    }

    #[test]
    fn test_list_plugin_commands_tool_schema() {
        let tool = list_plugin_commands_tool();
        assert_eq!(tool.name, "list_plugin_commands");
        assert!(tool.input_schema["properties"]["group"].is_object());
        assert!(tool.description.contains("sub-commands"));
    }

    #[test]
    fn test_depth_limit_message() {
        assert_eq!(MAX_DEPTH, 3);
    }

    #[test]
    fn test_spawn_agents_tool_schema() {
        let tool = spawn_agents_tool();
        assert_eq!(tool.name, "spawn_agents");
        let agents = &tool.input_schema["properties"]["agents"];
        assert!(agents.is_object());
        assert_eq!(agents["type"], "array");
        assert_eq!(agents["minItems"], 2);
    }

    #[test]
    fn test_all_four_tools_distinct_names() {
        let names: Vec<_> = [
            invoke_plugin_tool(),
            list_plugins_tool(),
            list_plugin_commands_tool(),
            spawn_agents_tool(),
        ]
        .iter()
        .map(|t| t.name.clone())
        .collect();
        let unique: std::collections::HashSet<_> = names.iter().collect();
        assert_eq!(names.len(), unique.len(), "all tool names must be unique");
    }
}

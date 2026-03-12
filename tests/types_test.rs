// NOTE: These tests require `pub mod claude;` to be present in src/lib.rs.
// The wiring agent must add that export before this file will compile.
use oac_slack_bot::claude::types::{ContentBlock, Message, MessageContent, Role};

#[test]
fn test_message_user_constructor() {
    let msg = Message::user("hello world");
    assert_eq!(msg.role, Role::User);
    match &msg.content {
        MessageContent::Text(t) => assert_eq!(t, "hello world"),
        _ => panic!("expected text content"),
    }
}

#[test]
fn test_message_assistant_constructor() {
    let msg = Message::assistant("I can help");
    assert_eq!(msg.role, Role::Assistant);
}

#[test]
fn test_message_tool_result_constructor() {
    let msg = Message::tool_result("tool-123", "result data");
    assert_eq!(msg.role, Role::User);
    match &msg.content {
        MessageContent::Blocks(blocks) => {
            assert_eq!(blocks.len(), 1);
            match &blocks[0] {
                ContentBlock::ToolResult {
                    tool_use_id,
                    content,
                } => {
                    assert_eq!(tool_use_id, "tool-123");
                    assert_eq!(content, "result data");
                }
                _ => panic!("expected tool result block"),
            }
        }
        _ => panic!("expected blocks content"),
    }
}

#[test]
fn test_message_serializes_correctly() {
    let msg = Message::user("test");
    let json = serde_json::to_string(&msg).unwrap();
    assert!(json.contains("\"role\":\"user\""));
    assert!(json.contains("test"));
}

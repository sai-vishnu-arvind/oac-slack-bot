// NOTE: These tests require `pub mod claude;` to be present in src/lib.rs.
// The wiring agent must add that export before this file will compile.
use oac_slack_bot::claude::session::{Session, SessionStore};
use oac_slack_bot::claude::types::Message;

#[test]
fn test_session_stores_and_retrieves_messages() {
    let mut store = SessionStore::new(10, 3600);
    let key = SessionStore::key("C123", Some("1234567890.123456"));

    let session = store.get_or_create(&key);
    session.push(Message::user("hello"));
    session.push(Message::assistant("hi there"));

    let session = store.get(&key).expect("session should exist");
    assert_eq!(session.messages.len(), 2);
}

#[test]
fn test_session_plugin_name() {
    let mut store = SessionStore::new(10, 3600);
    let key = "test-key";

    let session = store.get_or_create(key);
    assert!(session.plugin_name.is_none());
    session.plugin_name = Some("oncall-debugger".to_string());

    let session = store.get(key).unwrap();
    assert_eq!(session.plugin_name.as_deref(), Some("oncall-debugger"));
}

#[test]
fn test_session_key_formats() {
    // Thread reply
    assert_eq!(
        SessionStore::key("C01ABC", Some("1234.5678")),
        "C01ABC-1234.5678"
    );
    // DM no thread
    assert_eq!(SessionStore::key("D01ABC", None), "dm-D01ABC");
    // Channel no thread
    assert_eq!(SessionStore::key("C01ABC", None), "ch-C01ABC");
}

#[test]
fn test_session_messages_vec() {
    let mut session = Session::new();
    session.push(Message::user("q1"));
    session.push(Message::assistant("a1"));

    let vec = session.messages_vec();
    assert_eq!(vec.len(), 2);
}

#[test]
fn test_session_cleanup() {
    let mut store = SessionStore::new(10, 1);
    let key = "old-session";

    {
        let s = store.get_or_create(key);
        // Backdate by 10 seconds so it is older than the 1-second TTL
        s.last_activity = chrono::Utc::now() - chrono::Duration::seconds(10);
    }

    store.get_or_create("fresh-session").push(Message::user("hi"));
    store.cleanup();

    assert!(store.get(key).is_none(), "old session should be cleaned up");
    assert!(
        store.get("fresh-session").is_some(),
        "fresh session should remain"
    );
}

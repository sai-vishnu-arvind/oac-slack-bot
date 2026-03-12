use chrono::{DateTime, Utc};
use lru::LruCache;
use std::collections::VecDeque;
use std::num::NonZeroUsize;

use super::types::Message;

/// Maximum number of messages kept per session.
const MAX_SESSION_MESSAGES: usize = 50;

// ── Session ───────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct Session {
    /// Conversation history for this thread / DM.
    pub messages: VecDeque<Message>,
    /// The plugin currently active for this conversation thread (if any).
    pub plugin_name: Option<String>,
    /// Wall-clock time of the last user interaction; used for TTL eviction.
    pub last_activity: DateTime<Utc>,
}

impl Session {
    pub fn new() -> Self {
        Self {
            messages: VecDeque::new(),
            plugin_name: None,
            last_activity: Utc::now(),
        }
    }

    /// Append a message, evicting the oldest if we are at capacity.
    pub fn push(&mut self, msg: Message) {
        if self.messages.len() >= MAX_SESSION_MESSAGES {
            self.messages.pop_front();
        }
        self.messages.push_back(msg);
        self.last_activity = Utc::now();
    }

    /// Clone the conversation history into an owned `Vec`.
    pub fn messages_vec(&self) -> Vec<Message> {
        self.messages.iter().cloned().collect()
    }
}

impl Default for Session {
    fn default() -> Self {
        Self::new()
    }
}

// ── SessionStore ──────────────────────────────────────────────────────────────

/// Stores active [`Session`]s keyed by a string derived from the Slack channel
/// and thread timestamp.  Backed by an LRU cache so that the least-recently-
/// used sessions are evicted automatically when the cache is full.
pub struct SessionStore {
    sessions: LruCache<String, Session>,
    ttl_secs: u64,
}

impl SessionStore {
    /// Create a new store that holds at most `max_sessions` sessions and
    /// evicts sessions idle for longer than `ttl_secs` seconds on [`cleanup`].
    ///
    /// # Panics
    /// Panics if `max_sessions` is zero.
    pub fn new(max_sessions: usize, ttl_secs: u64) -> Self {
        let cap = NonZeroUsize::new(max_sessions)
            .expect("max_sessions must be non-zero");
        Self {
            sessions: LruCache::new(cap),
            ttl_secs,
        }
    }

    // ── Key helpers ──────────────────────────────────────────────────────────

    /// Derive a unique session key from a Slack `channel` and an optional
    /// `thread_ts`.
    ///
    /// Rules:
    /// - Thread reply → `{channel}-{thread_ts}`
    /// - DM (channel starts with `'D'`) with no thread → `dm-{channel}`
    /// - Channel message with no thread → `ch-{channel}`
    pub fn key(channel: &str, thread_ts: Option<&str>) -> String {
        match thread_ts {
            Some(ts) => format!("{channel}-{ts}"),
            None if channel.starts_with('D') => format!("dm-{channel}"),
            None => format!("ch-{channel}"),
        }
    }

    // ── Session access ────────────────────────────────────────────────────────

    /// Return a mutable reference to the session for `key`, creating a fresh
    /// one if it does not exist.
    pub fn get_or_create(&mut self, key: &str) -> &mut Session {
        if !self.sessions.contains(key) {
            self.sessions.put(key.to_string(), Session::new());
        }
        // `get_mut` promotes the entry to most-recently-used.
        self.sessions
            .get_mut(key)
            .expect("session was just inserted")
    }

    /// Return a mutable reference to an existing session, or `None` if the
    /// key is not present.
    pub fn get(&mut self, key: &str) -> Option<&mut Session> {
        self.sessions.get_mut(key)
    }

    // ── Maintenance ───────────────────────────────────────────────────────────

    /// Remove all sessions whose `last_activity` is older than `ttl_secs`.
    ///
    /// [`LruCache`] does not support mutating-iteration, so we collect stale
    /// keys first, then remove them one by one.
    pub fn cleanup(&mut self) {
        let deadline = Utc::now()
            - chrono::Duration::seconds(self.ttl_secs as i64);

        let stale_keys: Vec<String> = self
            .sessions
            .iter()
            .filter(|(_, s)| s.last_activity < deadline)
            .map(|(k, _)| k.clone())
            .collect();

        for key in stale_keys {
            self.sessions.pop(&key);
        }
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::claude::types::{Message, Role};

    fn make_user_msg(text: &str) -> Message {
        Message::user(text)
    }

    // ── SessionStore::key ─────────────────────────────────────────────────────

    #[test]
    fn test_session_key_dm() {
        // A channel ID starting with 'D' and no thread_ts should use the
        // "dm-" prefix.
        let key = SessionStore::key("D01ABC123", None);
        assert_eq!(key, "dm-D01ABC123");
    }

    #[test]
    fn test_session_key_thread() {
        // When a thread_ts is provided the key is "{channel}-{ts}".
        let key = SessionStore::key("C01XYZ999", Some("1700000000.123456"));
        assert_eq!(key, "C01XYZ999-1700000000.123456");
    }

    #[test]
    fn test_session_key_channel_no_thread() {
        // A public/private channel without a thread gets the "ch-" prefix.
        let key = SessionStore::key("C01XYZ999", None);
        assert_eq!(key, "ch-C01XYZ999");
    }

    // ── Session::push — max-50 eviction ───────────────────────────────────────

    #[test]
    fn test_session_push_max_50() {
        let mut session = Session::new();

        // Push 60 messages; the first 10 should be evicted.
        for i in 0..60_usize {
            session.push(make_user_msg(&format!("message {i}")));
        }

        assert_eq!(
            session.messages.len(),
            50,
            "session should cap at 50 messages"
        );

        // The first surviving message should be message 10.
        let msgs = session.messages_vec();
        let first_text = match &msgs[0].content {
            crate::claude::types::MessageContent::Text(t) => t.clone(),
            _ => panic!("expected text content"),
        };
        assert_eq!(first_text, "message 10");

        // The last message should be message 59.
        let last_text = match &msgs[49].content {
            crate::claude::types::MessageContent::Text(t) => t.clone(),
            _ => panic!("expected text content"),
        };
        assert_eq!(last_text, "message 59");
    }

    // ── SessionStore::get_or_create ───────────────────────────────────────────

    #[test]
    fn test_get_or_create_returns_fresh_session() {
        let mut store = SessionStore::new(10, 1800);
        let session = store.get_or_create("test-key");
        assert!(session.messages.is_empty());
        assert!(session.plugin_name.is_none());
    }

    #[test]
    fn test_get_or_create_returns_same_session() {
        let mut store = SessionStore::new(10, 1800);
        store.get_or_create("key1").push(make_user_msg("hello"));
        let session = store.get_or_create("key1");
        assert_eq!(session.messages.len(), 1);
    }

    // ── SessionStore::cleanup ─────────────────────────────────────────────────

    #[test]
    fn test_cleanup_removes_stale_sessions() {
        let mut store = SessionStore::new(10, 1); // 1-second TTL

        // Create a session and manually backdate its last_activity.
        {
            let s = store.get_or_create("old-key");
            s.last_activity = Utc::now() - chrono::Duration::seconds(10);
        }

        // Create a fresh session.
        store.get_or_create("new-key").push(make_user_msg("hi"));

        store.cleanup();

        assert!(
            store.get("old-key").is_none(),
            "stale session should have been removed"
        );
        assert!(
            store.get("new-key").is_some(),
            "fresh session should remain"
        );
    }

    // ── Role ──────────────────────────────────────────────────────────────────

    #[test]
    fn test_role_serde() {
        let msg = Message::assistant("hello");
        assert_eq!(msg.role, Role::Assistant);
    }
}

use std::collections::HashMap;
use std::num::NonZeroUsize;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use lru::LruCache;
use tokio::sync::Mutex;
use tracing::info;

// ── Bot message metadata (for reaction attribution) ─────────────────────────

/// Metadata stashed when the bot posts a message, so that a later reaction
/// can be attributed to the correct request.
#[derive(Debug, Clone)]
pub struct BotMessageInfo {
    pub channel: String,
    pub thread_ts: String,
    pub plugin: Option<String>,
    pub user: Option<String>,
    pub question: String,
    pub created_at: Instant,
}

// ── Metrics ─────────────────────────────────────────────────────────────────

/// In-memory, thread-safe metrics store for the bot.
///
/// Atomic counters are used for hot-path increments (lock-free).
/// Mutex-guarded structures are used for maps and vecs that need iteration.
pub struct Metrics {
    // ── Atomic counters ─────────────────────────────────────────────────
    pub total_mentions: AtomicU64,
    pub total_errors: AtomicU64,
    pub thumbs_up: AtomicU64,
    pub thumbs_down: AtomicU64,
    pub total_input_tokens: AtomicU64,
    pub total_output_tokens: AtomicU64,

    // ── Guarded maps ────────────────────────────────────────────────────
    plugin_calls: Mutex<HashMap<String, u64>>,
    user_calls: Mutex<HashMap<String, u64>>,
    channel_calls: Mutex<HashMap<String, u64>>,

    // ── Response times (milliseconds) ───────────────────────────────────
    response_times_ms: Mutex<Vec<u64>>,
    first_token_times_ms: Mutex<Vec<u64>>,

    // ── Bot message tracking ────────────────────────────────────────────
    pub bot_messages: Mutex<LruCache<String, BotMessageInfo>>,
}

impl Metrics {
    pub fn new() -> Self {
        Self {
            total_mentions: AtomicU64::new(0),
            total_errors: AtomicU64::new(0),
            thumbs_up: AtomicU64::new(0),
            thumbs_down: AtomicU64::new(0),
            total_input_tokens: AtomicU64::new(0),
            total_output_tokens: AtomicU64::new(0),
            plugin_calls: Mutex::new(HashMap::new()),
            user_calls: Mutex::new(HashMap::new()),
            channel_calls: Mutex::new(HashMap::new()),
            response_times_ms: Mutex::new(Vec::new()),
            first_token_times_ms: Mutex::new(Vec::new()),
            bot_messages: Mutex::new(LruCache::new(
                NonZeroUsize::new(2000).unwrap(),
            )),
        }
    }

    // ── Recording methods ───────────────────────────────────────────────

    /// Record a new mention (request) from a user in a channel.
    pub async fn record_mention(&self, user: Option<&str>, channel: &str) {
        self.total_mentions.fetch_add(1, Ordering::Relaxed);

        if let Some(u) = user {
            *self.user_calls.lock().await.entry(u.to_string()).or_insert(0) += 1;
        }
        *self
            .channel_calls
            .lock()
            .await
            .entry(channel.to_string())
            .or_insert(0) += 1;
    }

    pub fn record_error(&self) {
        self.total_errors.fetch_add(1, Ordering::Relaxed);
    }

    pub async fn record_plugin_call(&self, plugin_fqn: &str) {
        *self
            .plugin_calls
            .lock()
            .await
            .entry(plugin_fqn.to_string())
            .or_insert(0) += 1;
    }

    pub async fn record_response_time(&self, duration: Duration) {
        self.response_times_ms
            .lock()
            .await
            .push(duration.as_millis() as u64);
    }

    pub async fn record_first_token_time(&self, duration: Duration) {
        self.first_token_times_ms
            .lock()
            .await
            .push(duration.as_millis() as u64);
    }

    pub fn record_tokens(&self, input: u64, output: u64) {
        self.total_input_tokens.fetch_add(input, Ordering::Relaxed);
        self.total_output_tokens
            .fetch_add(output, Ordering::Relaxed);
    }

    // ── Bot message registration ────────────────────────────────────────

    /// Register a bot message so that later reactions can be attributed.
    pub async fn register_bot_message(
        &self,
        channel: &str,
        ts: &str,
        thread_ts: &str,
        plugin: Option<&str>,
        user: Option<&str>,
        question: &str,
    ) {
        let key = format!("{}:{}", channel, ts);
        self.bot_messages.lock().await.put(
            key,
            BotMessageInfo {
                channel: channel.to_string(),
                thread_ts: thread_ts.to_string(),
                plugin: plugin.map(|s| s.to_string()),
                user: user.map(|s| s.to_string()),
                question: question.to_string(),
                created_at: Instant::now(),
            },
        );
    }

    // ── Reaction handling ───────────────────────────────────────────────

    /// Process a reaction on a message.
    ///
    /// Returns `Some(info)` if the reaction was on a tracked bot message
    /// and was a recognised feedback emoji, `None` otherwise.
    pub async fn record_reaction(
        &self,
        channel: &str,
        message_ts: &str,
        reaction: &str,
        user: &str,
    ) -> Option<BotMessageInfo> {
        let is_positive = matches!(reaction, "+1" | "thumbsup" | "white_check_mark" | "heavy_check_mark");
        let is_negative = matches!(reaction, "-1" | "thumbsdown" | "x");

        if !is_positive && !is_negative {
            return None;
        }

        let key = format!("{}:{}", channel, message_ts);
        let info = self.bot_messages.lock().await.get(&key).cloned()?;

        if is_positive {
            self.thumbs_up.fetch_add(1, Ordering::Relaxed);
        } else {
            self.thumbs_down.fetch_add(1, Ordering::Relaxed);
        }

        info!(
            reaction = %reaction,
            sentiment = if is_positive { "positive" } else { "negative" },
            user = %user,
            channel = %info.channel,
            thread_ts = %info.thread_ts,
            plugin = ?info.plugin,
            response_age_secs = info.created_at.elapsed().as_secs(),
            "Feedback received"
        );

        Some(info)
    }

    // ── Stats formatting ────────────────────────────────────────────────

    /// Produce a Slack-formatted stats summary.
    pub async fn format_stats(&self) -> String {
        let mentions = self.total_mentions.load(Ordering::Relaxed);
        let errors = self.total_errors.load(Ordering::Relaxed);
        let up = self.thumbs_up.load(Ordering::Relaxed);
        let down = self.thumbs_down.load(Ordering::Relaxed);
        let input_tok = self.total_input_tokens.load(Ordering::Relaxed);
        let output_tok = self.total_output_tokens.load(Ordering::Relaxed);

        let error_pct = if mentions > 0 {
            (errors as f64 / mentions as f64) * 100.0
        } else {
            0.0
        };

        let feedback_pct = if up + down > 0 {
            (up as f64 / (up + down) as f64) * 100.0
        } else {
            0.0
        };

        // Response times
        let (avg_rt, p95_rt) = percentiles(&*self.response_times_ms.lock().await);
        let (avg_ft, _) = percentiles(&*self.first_token_times_ms.lock().await);

        // Top users (top 5)
        let top_users = top_n(&*self.user_calls.lock().await, 5);
        let top_channels = top_n(&*self.channel_calls.lock().await, 5);
        let top_plugins = top_n(&*self.plugin_calls.lock().await, 5);

        let mut s = String::new();
        s.push_str("📊 *Bot Stats*\n\n");

        // Usage
        s.push_str("*Usage*\n");
        s.push_str(&format!("  Mentions: {}\n", mentions));
        s.push_str(&format!("  Errors: {} ({:.1}%)\n", errors, error_pct));
        s.push_str(&format!("  Avg response: {}\n", format_ms(avg_rt)));
        s.push_str(&format!("  P95 response: {}\n", format_ms(p95_rt)));
        s.push_str(&format!("  Avg first token: {}\n\n", format_ms(avg_ft)));

        // Tokens
        s.push_str("*Tokens*\n");
        s.push_str(&format!("  Input: {}\n", format_number(input_tok)));
        s.push_str(&format!("  Output: {}\n\n", format_number(output_tok)));

        // Feedback
        s.push_str("*Feedback*\n");
        s.push_str(&format!(
            "  👍 {}  👎 {}  ({:.0}% positive)\n\n",
            up, down, feedback_pct
        ));

        // Top users
        if !top_users.is_empty() {
            s.push_str("*Top Users*\n");
            for (i, (name, count)) in top_users.iter().enumerate() {
                s.push_str(&format!("  {}. <@{}> — {} requests\n", i + 1, name, count));
            }
            s.push('\n');
        }

        // Top channels
        if !top_channels.is_empty() {
            s.push_str("*Top Channels*\n");
            for (i, (name, count)) in top_channels.iter().enumerate() {
                s.push_str(&format!("  {}. <#{}> — {} requests\n", i + 1, name, count));
            }
            s.push('\n');
        }

        // Top plugins
        if !top_plugins.is_empty() {
            s.push_str("*Top Plugins*\n");
            for (i, (name, count)) in top_plugins.iter().enumerate() {
                s.push_str(&format!("  {}. `{}` — {} calls\n", i + 1, name, count));
            }
            s.push('\n');
        }

        s
    }

    /// Log a periodic summary via tracing.
    pub async fn log_summary(&self) {
        let mentions = self.total_mentions.load(Ordering::Relaxed);
        let errors = self.total_errors.load(Ordering::Relaxed);
        let up = self.thumbs_up.load(Ordering::Relaxed);
        let down = self.thumbs_down.load(Ordering::Relaxed);
        let input_tok = self.total_input_tokens.load(Ordering::Relaxed);
        let output_tok = self.total_output_tokens.load(Ordering::Relaxed);

        let (avg_rt, p95_rt) = percentiles(&*self.response_times_ms.lock().await);
        let top_plugins = top_n(&*self.plugin_calls.lock().await, 5);
        let top_users = top_n(&*self.user_calls.lock().await, 3);

        info!(
            mentions,
            errors,
            thumbs_up = up,
            thumbs_down = down,
            input_tokens = input_tok,
            output_tokens = output_tok,
            avg_response_ms = avg_rt,
            p95_response_ms = p95_rt,
            top_plugins = ?top_plugins,
            top_users = ?top_users,
            "Metrics summary"
        );
    }
}

// ── Helpers ─────────────────────────────────────────────────────────────────

/// Compute average and p95 from a slice of u64 values.
fn percentiles(values: &[u64]) -> (u64, u64) {
    if values.is_empty() {
        return (0, 0);
    }
    let mut sorted = values.to_vec();
    sorted.sort_unstable();
    let sum: u64 = sorted.iter().sum();
    let avg = sum / sorted.len() as u64;
    let p95_idx = ((sorted.len() as f64) * 0.95) as usize;
    let p95 = sorted[p95_idx.min(sorted.len() - 1)];
    (avg, p95)
}

/// Return the top-N entries from a map, sorted by count descending.
fn top_n(map: &HashMap<String, u64>, n: usize) -> Vec<(String, u64)> {
    let mut entries: Vec<_> = map.iter().map(|(k, v)| (k.clone(), *v)).collect();
    entries.sort_by(|a, b| b.1.cmp(&a.1));
    entries.truncate(n);
    entries
}

/// Format milliseconds into a human-readable duration string.
fn format_ms(ms: u64) -> String {
    if ms == 0 {
        return "—".to_string();
    }
    if ms < 1000 {
        format!("{}ms", ms)
    } else {
        format!("{:.1}s", ms as f64 / 1000.0)
    }
}

/// Format a large number with comma separators.
fn format_number(n: u64) -> String {
    let s = n.to_string();
    let mut result = String::new();
    for (i, c) in s.chars().rev().enumerate() {
        if i > 0 && i % 3 == 0 {
            result.push(',');
        }
        result.push(c);
    }
    result.chars().rev().collect()
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_record_mention_increments() {
        let m = Metrics::new();
        m.record_mention(Some("U123"), "C456").await;
        m.record_mention(Some("U123"), "C456").await;
        m.record_mention(Some("U789"), "C456").await;

        assert_eq!(m.total_mentions.load(Ordering::Relaxed), 3);
        assert_eq!(*m.user_calls.lock().await.get("U123").unwrap(), 2);
        assert_eq!(*m.user_calls.lock().await.get("U789").unwrap(), 1);
        assert_eq!(*m.channel_calls.lock().await.get("C456").unwrap(), 3);
    }

    #[tokio::test]
    async fn test_record_reaction_positive() {
        let m = Metrics::new();
        m.register_bot_message("C1", "1.0", "1.0", Some("plugin-a"), Some("U1"), "how?")
            .await;

        let info = m.record_reaction("C1", "1.0", "+1", "U2").await;
        assert!(info.is_some());
        assert_eq!(m.thumbs_up.load(Ordering::Relaxed), 1);
        assert_eq!(m.thumbs_down.load(Ordering::Relaxed), 0);
    }

    #[tokio::test]
    async fn test_record_reaction_negative() {
        let m = Metrics::new();
        m.register_bot_message("C1", "2.0", "2.0", None, None, "what?")
            .await;

        let info = m.record_reaction("C1", "2.0", "-1", "U3").await;
        assert!(info.is_some());
        assert_eq!(m.thumbs_down.load(Ordering::Relaxed), 1);
    }

    #[tokio::test]
    async fn test_record_reaction_untracked_message() {
        let m = Metrics::new();
        let info = m.record_reaction("C1", "999.0", "+1", "U1").await;
        assert!(info.is_none());
        assert_eq!(m.thumbs_up.load(Ordering::Relaxed), 0);
    }

    #[tokio::test]
    async fn test_record_reaction_irrelevant_emoji() {
        let m = Metrics::new();
        m.register_bot_message("C1", "1.0", "1.0", None, None, "q")
            .await;
        let info = m.record_reaction("C1", "1.0", "eyes", "U1").await;
        assert!(info.is_none());
    }

    #[tokio::test]
    async fn test_plugin_call_counting() {
        let m = Metrics::new();
        m.record_plugin_call("oncall-debugger").await;
        m.record_plugin_call("oncall-debugger").await;
        m.record_plugin_call("backend-eng").await;

        let map = m.plugin_calls.lock().await;
        assert_eq!(*map.get("oncall-debugger").unwrap(), 2);
        assert_eq!(*map.get("backend-eng").unwrap(), 1);
    }

    #[test]
    fn test_percentiles_empty() {
        assert_eq!(percentiles(&[]), (0, 0));
    }

    #[test]
    fn test_percentiles_single() {
        assert_eq!(percentiles(&[100]), (100, 100));
    }

    #[test]
    fn test_percentiles_normal() {
        let vals: Vec<u64> = (1..=100).collect();
        let (avg, p95) = percentiles(&vals);
        assert_eq!(avg, 50);
        // p95 index = (100 * 0.95) = 95, clamped to len-1 = 99, vals[95] = 96
        assert_eq!(p95, 96);
    }

    #[test]
    fn test_top_n() {
        let mut map = HashMap::new();
        map.insert("a".into(), 10);
        map.insert("b".into(), 30);
        map.insert("c".into(), 20);

        let top = top_n(&map, 2);
        assert_eq!(top.len(), 2);
        assert_eq!(top[0].0, "b");
        assert_eq!(top[1].0, "c");
    }

    #[test]
    fn test_format_ms() {
        assert_eq!(format_ms(0), "—");
        assert_eq!(format_ms(500), "500ms");
        assert_eq!(format_ms(3200), "3.2s");
    }

    #[test]
    fn test_format_number() {
        assert_eq!(format_number(0), "0");
        assert_eq!(format_number(999), "999");
        assert_eq!(format_number(1234), "1,234");
        assert_eq!(format_number(1234567), "1,234,567");
    }

    #[tokio::test]
    async fn test_format_stats_output() {
        let m = Metrics::new();
        m.record_mention(Some("U1"), "C1").await;
        let stats = m.format_stats().await;
        assert!(stats.contains("📊 *Bot Stats*"));
        assert!(stats.contains("Mentions: 1"));
    }
}

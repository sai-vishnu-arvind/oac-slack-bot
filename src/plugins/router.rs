use super::registry::{Plugin, PluginRegistry};

/// Given a user message and the plugin registry, return the best matching plugin (if any).
/// Returns None if no plugin matches well enough.
pub fn route<'a>(message: &str, registry: &'a PluginRegistry) -> Option<&'a Plugin> {
    let message_lower = message.to_lowercase();

    // 1. Force override: if message contains `/plugin <name>` exactly, return that plugin by name.
    //    Supports both FQN (`/plugin second-brain:capture`) and bare names.
    if let Some(plugin) = try_force_override(message, registry) {
        return Some(plugin);
    }

    // 2. OAC triage patterns: [FIRING], [CRITICAL], triage, diagnose
    if let Some(plugin) = try_triage_patterns(&message_lower, registry) {
        return Some(plugin);
    }

    // 3. FQN match: if message contains a `group:command` token, match it.
    if let Some(plugin) = try_fqn_match(&message_lower, registry) {
        return Some(plugin);
    }

    // 4. Keyword match on plugin FQN/name: split message into words, check if any word exactly
    //    matches a plugin FQN or bare name (case-insensitive).
    if let Some(plugin) = try_name_keyword_match(&message_lower, registry) {
        return Some(plugin);
    }

    // 5. Keyword match on description: split message into significant words (>4 chars),
    //    check if any plugin's description contains that word.
    if let Some(plugin) = try_description_keyword_match(&message_lower, registry) {
        return Some(plugin);
    }

    // 6. No match found.
    None
}

/// Step 1: Force override via `/plugin <name>` anywhere in the message.
pub fn try_force_override<'a>(message: &str, registry: &'a PluginRegistry) -> Option<&'a Plugin> {
    let marker = "/plugin ";
    if let Some(pos) = message.find(marker) {
        let after = &message[pos + marker.len()..];
        // The plugin name is everything up to the next whitespace (or end of string).
        let name = after.split_whitespace().next()?;
        if let Some(plugin) = registry.get(name) {
            return Some(plugin);
        }
    }
    None
}

/// Step 2: OAC triage patterns.
pub fn try_triage_patterns<'a>(
    message_lower: &str,
    registry: &'a PluginRegistry,
) -> Option<&'a Plugin> {
    let is_triage = message_lower.contains("[firing]")
        || message_lower.contains("[critical]")
        || message_lower.contains("triage")
        || message_lower.contains("diagnose");

    if !is_triage {
        return None;
    }

    registry
        .get("oncall-debugger")
        .or_else(|| registry.get("systematic-solver-v2"))
}

/// Step 3: FQN match — look for `group:command` patterns in the message.
fn try_fqn_match<'a>(
    message_lower: &str,
    registry: &'a PluginRegistry,
) -> Option<&'a Plugin> {
    let words: Vec<&str> = message_lower
        .split_whitespace()
        .map(|w| w.trim_matches(|c: char| !c.is_alphanumeric() && c != '-' && c != '_' && c != ':'))
        .collect();

    for word in &words {
        if word.contains(':') {
            if let Some(plugin) = registry.get(word) {
                return Some(plugin);
            }
        }
    }

    None
}

/// Step 4: Keyword match on plugin FQN or bare name (case-insensitive, exact word match).
/// Strips leading/trailing punctuation from each word before comparing.
fn try_name_keyword_match<'a>(
    message_lower: &str,
    registry: &'a PluginRegistry,
) -> Option<&'a Plugin> {
    let words: Vec<&str> = message_lower
        .split_whitespace()
        .map(|w| w.trim_matches(|c: char| !c.is_alphanumeric() && c != '-' && c != '_' && c != ':'))
        .collect();

    for (fqn, _, _) in registry.list() {
        let fqn_lower = fqn.to_lowercase();
        if words.iter().any(|w| *w == fqn_lower.as_str()) {
            return registry.get(&fqn);
        }
    }

    // Also try matching just the bare name portion.
    // Use registry.get(bare_name) which returns None for ambiguous bare names.
    let mut tried_bare: std::collections::HashSet<String> = std::collections::HashSet::new();
    for (fqn, _, _) in registry.list() {
        let bare_name = if let Some(pos) = fqn.find(':') {
            &fqn[pos + 1..]
        } else {
            &fqn
        };
        let bare_lower = bare_name.to_lowercase();
        if !tried_bare.insert(bare_lower.clone()) {
            continue; // Already tried this bare name
        }
        if words.iter().any(|w| *w == bare_lower.as_str()) {
            if let Some(plugin) = registry.get(&bare_lower) {
                return Some(plugin);
            }
        }
    }

    None
}

/// Step 5: Keyword match on plugin description using significant words (>4 chars).
fn try_description_keyword_match<'a>(
    message_lower: &str,
    registry: &'a PluginRegistry,
) -> Option<&'a Plugin> {
    // Collect significant words from the message.
    let significant_words: Vec<&str> = message_lower
        .split_whitespace()
        .filter(|w| w.len() > 4)
        .collect();

    if significant_words.is_empty() {
        return None;
    }

    for (fqn, description, _) in registry.list() {
        let desc_lower = description.to_lowercase();
        if significant_words
            .iter()
            .any(|w| desc_lower.contains(*w))
        {
            return registry.get(&fqn);
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugins::registry::PluginRegistry;
    use std::fs;
    use std::io::Write;
    use std::path::Path;
    use tempfile::TempDir;

    fn make_skill(dir: &Path, name: &str, description: &str, body: &str) {
        let plugin_dir = dir.join(name);
        fs::create_dir_all(&plugin_dir).unwrap();
        let skill_path = plugin_dir.join("SKILL.md");
        let mut f = fs::File::create(&skill_path).unwrap();
        write!(
            f,
            "---\nname: {}\ndescription: {}\n---\n\n{}",
            name, description, body
        )
        .unwrap();
    }

    fn make_grouped_skill(dir: &Path, group: &str, cmd: &str, description: &str, body: &str) {
        let skill_dir = dir.join(group).join("skills").join(cmd);
        fs::create_dir_all(&skill_dir).unwrap();
        let skill_path = skill_dir.join("SKILL.md");
        let mut f = fs::File::create(&skill_path).unwrap();
        write!(
            f,
            "---\nname: {}\ndescription: {}\n---\n\n{}",
            cmd, description, body
        )
        .unwrap();
    }

    fn build_registry(tmp: &TempDir, plugins: &[(&str, &str, &str)]) -> PluginRegistry {
        for (name, desc, body) in plugins {
            make_skill(tmp.path(), name, desc, body);
        }
        let dirs = vec![tmp.path().to_str().unwrap().to_string()];
        PluginRegistry::load(&dirs)
    }

    #[test]
    fn test_route_firing_pattern() {
        let tmp = TempDir::new().unwrap();
        let registry = build_registry(
            &tmp,
            &[
                ("oncall-debugger", "Debugs oncall incidents", "Debug prompt"),
                ("other-plugin", "Unrelated plugin", "Other prompt"),
            ],
        );

        let plugin = route("[FIRING] Alert: high error rate in payments service", &registry);
        assert!(plugin.is_some());
        assert_eq!(plugin.unwrap().fqn, "oncall-debugger");
    }

    #[test]
    fn test_route_firing_pattern_fallback() {
        let tmp = TempDir::new().unwrap();
        let registry = build_registry(
            &tmp,
            &[(
                "systematic-solver-v2",
                "Systematic problem solver",
                "Solver prompt",
            )],
        );

        let plugin = route("[CRITICAL] Database down", &registry);
        assert!(plugin.is_some());
        assert_eq!(plugin.unwrap().fqn, "systematic-solver-v2");
    }

    #[test]
    fn test_route_exact_name_match() {
        let tmp = TempDir::new().unwrap();
        let registry = build_registry(
            &tmp,
            &[
                ("nexus-platform-faq", "Nexus FAQ assistant", "FAQ prompt"),
                ("other-plugin", "Unrelated plugin", "Other prompt"),
            ],
        );

        let plugin = route("Can you help me with nexus-platform-faq?", &registry);
        assert!(plugin.is_some());
        assert_eq!(plugin.unwrap().fqn, "nexus-platform-faq");
    }

    #[test]
    fn test_route_force_override() {
        let tmp = TempDir::new().unwrap();
        let registry = build_registry(
            &tmp,
            &[
                ("nexus-platform-faq", "Nexus FAQ assistant", "FAQ prompt"),
                ("other-plugin", "Some other plugin", "Other prompt"),
            ],
        );

        let plugin = route("please use /plugin other-plugin for this question", &registry);
        assert!(plugin.is_some());
        assert_eq!(plugin.unwrap().fqn, "other-plugin");
    }

    #[test]
    fn test_route_no_match_returns_none() {
        let tmp = TempDir::new().unwrap();
        let registry = build_registry(
            &tmp,
            &[("very-specific-plugin", "Does very specific thing", "Prompt")],
        );

        let plugin = route("hi how are you", &registry);
        assert!(plugin.is_none());
    }

    #[test]
    fn test_route_description_keyword_match() {
        let tmp = TempDir::new().unwrap();
        let registry = build_registry(
            &tmp,
            &[(
                "billing-helper",
                "Assists with invoice and payment questions",
                "Billing prompt",
            )],
        );

        let plugin = route("I have a question about my invoice", &registry);
        assert!(plugin.is_some());
        assert_eq!(plugin.unwrap().fqn, "billing-helper");
    }

    // ── FQN routing tests ────────────────────────────────────────────────────

    #[test]
    fn test_route_fqn_force_override() {
        let tmp = TempDir::new().unwrap();
        make_grouped_skill(
            tmp.path(),
            "second-brain",
            "capture",
            "Capture findings",
            "prompt",
        );
        make_grouped_skill(
            tmp.path(),
            "second-brain",
            "process",
            "Process findings",
            "prompt",
        );
        let dirs = vec![tmp.path().to_str().unwrap().to_string()];
        let registry = PluginRegistry::load(&dirs);

        let plugin = route("/plugin second-brain:capture save this", &registry);
        assert!(plugin.is_some());
        assert_eq!(plugin.unwrap().fqn, "second-brain:capture");
    }

    #[test]
    fn test_route_fqn_in_message() {
        let tmp = TempDir::new().unwrap();
        make_grouped_skill(
            tmp.path(),
            "second-brain",
            "capture",
            "Capture findings",
            "prompt",
        );
        let dirs = vec![tmp.path().to_str().unwrap().to_string()];
        let registry = PluginRegistry::load(&dirs);

        let plugin = route("run second-brain:capture on this finding", &registry);
        assert!(plugin.is_some());
        assert_eq!(plugin.unwrap().fqn, "second-brain:capture");
    }

    #[test]
    fn test_route_ambiguous_bare_name_no_match() {
        let tmp = TempDir::new().unwrap();
        make_grouped_skill(tmp.path(), "agent-ready", "init", "Sets up AR", "p1");
        make_grouped_skill(tmp.path(), "fe-agent-ready", "init", "Sets up FE", "p2");
        let dirs = vec![tmp.path().to_str().unwrap().to_string()];
        let registry = PluginRegistry::load(&dirs);

        // Bare "init" is ambiguous — router should not match.
        // Descriptions kept short (words <=4 chars) so description keyword match won't fire.
        let plugin = route("run init", &registry);
        assert!(plugin.is_none());
    }

    #[test]
    fn test_route_fqn_resolves_ambiguity() {
        let tmp = TempDir::new().unwrap();
        make_grouped_skill(tmp.path(), "agent-ready", "init", "AR init", "p1");
        make_grouped_skill(tmp.path(), "fe-agent-ready", "init", "FE init", "p2");
        let dirs = vec![tmp.path().to_str().unwrap().to_string()];
        let registry = PluginRegistry::load(&dirs);

        // FQN resolves it
        let plugin = route("run agent-ready:init", &registry);
        assert!(plugin.is_some());
        assert_eq!(plugin.unwrap().fqn, "agent-ready:init");

        let plugin2 = route("run fe-agent-ready:init", &registry);
        assert!(plugin2.is_some());
        assert_eq!(plugin2.unwrap().fqn, "fe-agent-ready:init");
    }
}

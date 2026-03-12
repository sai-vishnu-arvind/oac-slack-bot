use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use tracing::{debug, info, warn};

#[derive(Debug, Clone, PartialEq)]
pub struct Plugin {
    /// Bare command name from frontmatter (e.g. "capture", "init").
    pub name: String,
    /// Plugin group derived from directory structure (e.g. "second-brain").
    /// `None` for flat plugins like `~/.agents/skills/oncall-debugger/SKILL.md`.
    pub group: Option<String>,
    /// Fully-qualified name: `group:name` for grouped plugins, bare `name` for flat ones.
    pub fqn: String,
    pub description: String,
    pub system_prompt: String,
    pub source_path: PathBuf,
}

pub struct PluginRegistry {
    /// Keyed by FQN (e.g. "second-brain:capture" or "oncall-debugger").
    plugins: HashMap<String, Plugin>,
}

impl PluginRegistry {
    /// Scan all given directories for SKILL.md files and build registry.
    /// Later directories override earlier ones with the same FQN.
    pub fn load(dirs: &[String]) -> Self {
        let mut plugins: HashMap<String, Plugin> = HashMap::new();

        for dir in dirs {
            let path = Path::new(dir);
            if !path.exists() {
                debug!(dir = %dir, "Plugin dir does not exist, skipping");
                continue;
            }
            if !path.is_dir() {
                warn!(dir = %dir, "Plugin dir path is not a directory, skipping");
                continue;
            }

            let skill_files = find_skill_files(path, 0);
            debug!(dir = %dir, count = skill_files.len(), "Found SKILL.md files");

            for skill_path in skill_files {
                match parse_skill_file(&skill_path) {
                    Some(plugin) => {
                        if plugin.name.starts_with('_') {
                            debug!(name = %plugin.name, "Skipping private plugin (starts with _)");
                            continue;
                        }
                        debug!(fqn = %plugin.fqn, path = ?skill_path, "Loaded plugin");
                        plugins.insert(plugin.fqn.clone(), plugin);
                    }
                    None => {
                        warn!(path = ?skill_path, "Failed to parse SKILL.md, skipping");
                    }
                }
            }
        }

        info!(count = plugins.len(), "Plugin registry loaded");

        Self { plugins }
    }

    /// Look up a plugin by FQN first, then try bare name (if unambiguous).
    pub fn get(&self, key: &str) -> Option<&Plugin> {
        // 1. Exact FQN match (e.g. "second-brain:capture" or "oncall-debugger").
        if let Some(p) = self.plugins.get(key) {
            return Some(p);
        }

        // 2. If the key contains ':', it was meant as an FQN — no fallback.
        if key.contains(':') {
            return None;
        }

        // 3. Bare name fallback: find all plugins whose bare `name` matches.
        let matches: Vec<&Plugin> = self
            .plugins
            .values()
            .filter(|p| p.name == key)
            .collect();

        match matches.len() {
            1 => Some(matches[0]),
            0 => None,
            _ => {
                // Ambiguous — log and return None so the caller can handle it.
                let fqns: Vec<&str> = matches.iter().map(|p| p.fqn.as_str()).collect();
                debug!(bare_name = key, candidates = ?fqns, "Ambiguous bare name lookup");
                None
            }
        }
    }

    /// Like `get`, but when a bare name is ambiguous, returns the list of
    /// candidate FQNs instead of silently returning None.
    pub fn get_or_ambiguous(&self, key: &str) -> Result<&Plugin, GetError> {
        if let Some(p) = self.plugins.get(key) {
            return Ok(p);
        }

        if key.contains(':') {
            return Err(GetError::NotFound);
        }

        let matches: Vec<&Plugin> = self
            .plugins
            .values()
            .filter(|p| p.name == key)
            .collect();

        match matches.len() {
            1 => Ok(matches[0]),
            0 => Err(GetError::NotFound),
            _ => {
                let fqns = matches.iter().map(|p| p.fqn.clone()).collect();
                Err(GetError::Ambiguous(fqns))
            }
        }
    }

    /// Returns (fqn, description, group) tuples for all loaded plugins, sorted by FQN.
    pub fn list(&self) -> Vec<(String, String, Option<String>)> {
        let mut result: Vec<(String, String, Option<String>)> = self
            .plugins
            .values()
            .map(|p| (p.fqn.clone(), p.description.clone(), p.group.clone()))
            .collect();
        result.sort_by(|a, b| a.0.cmp(&b.0));
        result
    }

    /// List all sub-commands belonging to a specific group.
    pub fn get_group_commands(&self, group: &str) -> Vec<&Plugin> {
        let mut result: Vec<&Plugin> = self
            .plugins
            .values()
            .filter(|p| p.group.as_deref() == Some(group))
            .collect();
        result.sort_by(|a, b| a.fqn.cmp(&b.fqn));
        result
    }

    /// List all distinct group names.
    pub fn groups(&self) -> Vec<String> {
        let mut groups: Vec<String> = self
            .plugins
            .values()
            .filter_map(|p| p.group.clone())
            .collect::<std::collections::HashSet<_>>()
            .into_iter()
            .collect();
        groups.sort();
        groups
    }

    pub fn len(&self) -> usize {
        self.plugins.len()
    }

    pub fn is_empty(&self) -> bool {
        self.plugins.is_empty()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum GetError {
    NotFound,
    /// Bare name matched multiple plugins — contains their FQNs.
    Ambiguous(Vec<String>),
}

const MAX_DEPTH: usize = 5;

/// Recursively collect all SKILL.md paths under `dir`, up to `MAX_DEPTH` levels deep.
/// Skips directories named `node_modules`, `.git`, `target`.
fn find_skill_files(dir: &Path, depth: usize) -> Vec<PathBuf> {
    if depth > MAX_DEPTH {
        return vec![];
    }

    let mut results = Vec::new();

    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(err) => {
            warn!(dir = ?dir, error = %err, "Cannot read plugin directory");
            return results;
        }
    };

    for entry in entries.flatten() {
        let path = entry.path();

        if path.is_dir() {
            let dir_name = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("");

            if matches!(dir_name, "node_modules" | ".git" | "target") {
                debug!(dir = ?path, "Skipping excluded directory");
                continue;
            }

            let mut nested = find_skill_files(&path, depth + 1);
            results.append(&mut nested);
        } else if path.is_file() {
            if path.file_name().and_then(|n| n.to_str()) == Some("SKILL.md") {
                results.push(path);
            }
        }
    }

    results
}

/// Derive (group, fqn) from the SKILL.md file path.
///
/// Pattern: `.../{group}/skills/{command}/SKILL.md` → group = Some(group), fqn = "group:name"
/// Pattern: `.../{command}/SKILL.md`                → group = None, fqn = "name"
///
/// The `name` parameter is the bare name parsed from frontmatter.
fn derive_fqn(path: &Path, name: &str) -> (Option<String>, String) {
    // Walk up: SKILL.md → command_dir → maybe "skills" → maybe group_dir
    let command_dir = match path.parent() {
        Some(p) => p,
        None => return (None, name.to_string()),
    };

    let maybe_skills_dir = match command_dir.parent() {
        Some(p) => p,
        None => return (None, name.to_string()),
    };

    let skills_dir_name = maybe_skills_dir
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("");

    if skills_dir_name == "skills" {
        // We have the pattern: {group}/skills/{command}/SKILL.md
        // But skip if the group dir starts with '.' (e.g. .agents/skills/ is a flat layout)
        if let Some(group_dir) = maybe_skills_dir.parent() {
            if let Some(group_name) = group_dir.file_name().and_then(|n| n.to_str()) {
                if !group_name.starts_with('.') {
                    let fqn = format!("{}:{}", group_name, name);
                    return (Some(group_name.to_string()), fqn);
                }
            }
        }
    }

    // Flat structure: no group
    (None, name.to_string())
}

/// Parse a SKILL.md file into a Plugin.
/// Returns None if the file cannot be read or lacks valid frontmatter with name + description.
fn parse_skill_file(path: &Path) -> Option<Plugin> {
    let content = match fs::read_to_string(path) {
        Ok(c) => c,
        Err(err) => {
            warn!(path = ?path, error = %err, "Cannot read SKILL.md");
            return None;
        }
    };

    parse_skill_content(&content, path)
}

/// Parse skill content (separated for testability).
fn parse_skill_content(content: &str, path: &Path) -> Option<Plugin> {
    // The file must start with ---
    let content = content.trim_start_matches('\n');
    if !content.starts_with("---") {
        warn!(path = ?path, "SKILL.md does not start with ---");
        return None;
    }

    // Find the second --- delimiter
    let after_first_delimiter = &content["---".len()..];

    // The closing --- must be on its own line
    let closing = after_first_delimiter.find("\n---")?;
    let frontmatter = &after_first_delimiter[..closing].trim();
    let rest = &after_first_delimiter[closing + "\n---".len()..];
    let system_prompt = rest.trim().to_string();

    let name = extract_frontmatter_field(frontmatter, "name")?;
    let description = extract_frontmatter_field(frontmatter, "description")
        .unwrap_or_else(|| String::new());

    if name.is_empty() {
        warn!(path = ?path, "SKILL.md has empty name field");
        return None;
    }

    let (group, fqn) = derive_fqn(path, &name);

    // Load companion files if declared in frontmatter.
    let companion_files = extract_frontmatter_list(frontmatter, "companion-files");
    let system_prompt = if companion_files.is_empty() {
        system_prompt
    } else {
        let skill_dir = path.parent().unwrap_or(Path::new("."));
        let mut full_prompt = system_prompt;
        for filename in &companion_files {
            let companion_path = skill_dir.join(filename);
            match fs::read_to_string(&companion_path) {
                Ok(content) => {
                    full_prompt.push_str(&format!("\n\n---\n# {}\n\n{}", filename, content.trim()));
                    debug!(fqn = %fqn, file = %filename, "Loaded companion file");
                }
                Err(err) => {
                    warn!(fqn = %fqn, file = %filename, error = %err, "Companion file not found, skipping");
                }
            }
        }
        full_prompt
    };

    Some(Plugin {
        name,
        group,
        fqn,
        description,
        system_prompt,
        source_path: path.to_path_buf(),
    })
}

/// Extract a YAML list from frontmatter. Handles:
/// ```yaml
/// companion-files:
///   - DECISION_TREE.md
///   - SYSTEM_PROMPT.md
/// ```
fn extract_frontmatter_list(frontmatter: &str, field: &str) -> Vec<String> {
    let prefix = format!("{}:", field);
    let mut result = Vec::new();
    let mut in_list = false;

    for line in frontmatter.lines() {
        let trimmed = line.trim();

        if trimmed.starts_with(&prefix) {
            // Check if there's an inline value (not a list)
            let after = trimmed[prefix.len()..].trim();
            if after.is_empty() {
                in_list = true;
                continue;
            }
            // Not a list — ignore
            return result;
        }

        if in_list {
            if let Some(item) = trimmed.strip_prefix("- ") {
                let item = item.trim();
                // Strip quotes
                let item = if (item.starts_with('"') && item.ends_with('"'))
                    || (item.starts_with('\'') && item.ends_with('\''))
                {
                    &item[1..item.len() - 1]
                } else {
                    item
                };
                if !item.is_empty() {
                    result.push(item.to_string());
                }
            } else if !trimmed.is_empty() {
                // Non-list-item, non-empty line → list ended
                break;
            }
        }
    }

    result
}

/// Extract a simple scalar value from a YAML-like frontmatter block.
fn extract_frontmatter_field(frontmatter: &str, field: &str) -> Option<String> {
    let prefix = format!("{}:", field);
    for line in frontmatter.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix(&prefix) {
            let value = rest.trim();
            let value = if (value.starts_with('"') && value.ends_with('"'))
                || (value.starts_with('\'') && value.ends_with('\''))
            {
                &value[1..value.len() - 1]
            } else {
                value
            };
            return Some(value.to_string());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::TempDir;

    /// Create a flat skill: `dir/<name>/SKILL.md`
    fn make_skill(dir: &Path, name: &str, description: &str, body: &str) {
        let plugin_dir = dir.join(name);
        fs::create_dir_all(&plugin_dir).expect("create plugin subdir");
        let skill_path = plugin_dir.join("SKILL.md");
        let mut f = fs::File::create(&skill_path).expect("create SKILL.md");
        write!(
            f,
            "---\nname: {}\ndescription: {}\n---\n\n{}",
            name, description, body
        )
        .expect("write SKILL.md");
    }

    /// Create a grouped skill: `dir/<group>/skills/<cmd>/SKILL.md`
    fn make_grouped_skill(
        dir: &Path,
        group: &str,
        cmd: &str,
        description: &str,
        body: &str,
    ) {
        let skill_dir = dir.join(group).join("skills").join(cmd);
        fs::create_dir_all(&skill_dir).expect("create grouped skill dir");
        let skill_path = skill_dir.join("SKILL.md");
        let mut f = fs::File::create(&skill_path).expect("create SKILL.md");
        write!(
            f,
            "---\nname: {}\ndescription: {}\n---\n\n{}",
            cmd, description, body
        )
        .expect("write SKILL.md");
    }

    // ── Existing tests (adapted for new list() signature) ────────────────────

    #[test]
    fn test_load_empty_dirs() {
        let registry = PluginRegistry::load(&[]);
        assert!(registry.is_empty());
        assert_eq!(registry.len(), 0);
    }

    #[test]
    fn test_load_single_plugin() {
        let tmp = TempDir::new().unwrap();
        make_skill(
            tmp.path(),
            "my-plugin",
            "Does something useful",
            "You are a helpful assistant.",
        );

        let dirs = vec![tmp.path().to_str().unwrap().to_string()];
        let registry = PluginRegistry::load(&dirs);

        assert_eq!(registry.len(), 1);
        let plugin = registry.get("my-plugin").expect("plugin should be present");
        assert_eq!(plugin.name, "my-plugin");
        assert_eq!(plugin.fqn, "my-plugin");
        assert!(plugin.group.is_none());
        assert_eq!(plugin.description, "Does something useful");
        assert_eq!(plugin.system_prompt, "You are a helpful assistant.");
    }

    #[test]
    fn test_skip_underscore_plugins() {
        let tmp = TempDir::new().unwrap();
        make_skill(tmp.path(), "_private-plugin", "Internal tool", "secret prompt");
        make_skill(tmp.path(), "public-plugin", "Public tool", "public prompt");

        let dirs = vec![tmp.path().to_str().unwrap().to_string()];
        let registry = PluginRegistry::load(&dirs);

        assert_eq!(registry.len(), 1);
        assert!(registry.get("_private-plugin").is_none());
        assert!(registry.get("public-plugin").is_some());
    }

    #[test]
    fn test_later_dir_overrides_earlier() {
        let tmp1 = TempDir::new().unwrap();
        let tmp2 = TempDir::new().unwrap();

        make_skill(tmp1.path(), "shared-plugin", "Version 1", "Prompt v1");
        make_skill(tmp2.path(), "shared-plugin", "Version 2", "Prompt v2");

        let dirs = vec![
            tmp1.path().to_str().unwrap().to_string(),
            tmp2.path().to_str().unwrap().to_string(),
        ];
        let registry = PluginRegistry::load(&dirs);

        assert_eq!(registry.len(), 1);
        let plugin = registry.get("shared-plugin").unwrap();
        assert_eq!(plugin.description, "Version 2");
        assert_eq!(plugin.system_prompt, "Prompt v2");
    }

    #[test]
    fn test_parse_frontmatter() {
        let content = r#"---
name: test-plugin
description: A test plugin for unit testing
---

# System Prompt

You are a test assistant.
"#;

        let path = Path::new("SKILL.md");
        let plugin = parse_skill_content(content, path).expect("should parse successfully");

        assert_eq!(plugin.name, "test-plugin");
        assert_eq!(plugin.description, "A test plugin for unit testing");
        assert!(plugin.system_prompt.contains("You are a test assistant."));
    }

    // ── FQN derivation tests ─────────────────────────────────────────────────

    #[test]
    fn test_derive_fqn_nested() {
        let path = Path::new("/home/user/claude-plugins/plugins/second-brain/skills/capture/SKILL.md");
        let (group, fqn) = derive_fqn(path, "capture");
        assert_eq!(group, Some("second-brain".to_string()));
        assert_eq!(fqn, "second-brain:capture");
    }

    #[test]
    fn test_derive_fqn_flat() {
        let path = Path::new("/home/user/.agents/skills/oncall-debugger/SKILL.md");
        let (group, fqn) = derive_fqn(path, "oncall-debugger");
        assert!(group.is_none());
        assert_eq!(fqn, "oncall-debugger");
    }

    #[test]
    fn test_derive_fqn_deeply_nested() {
        // Even if there's more nesting, we only look at immediate parent structure
        let path = Path::new("/x/y/backend-engineer/skills/brainstorming/SKILL.md");
        let (group, fqn) = derive_fqn(path, "brainstorming");
        assert_eq!(group, Some("backend-engineer".to_string()));
        assert_eq!(fqn, "backend-engineer:brainstorming");
    }

    // ── Grouped plugin loading tests ─────────────────────────────────────────

    #[test]
    fn test_load_grouped_plugin() {
        let tmp = TempDir::new().unwrap();
        make_grouped_skill(
            tmp.path(),
            "second-brain",
            "capture",
            "Captures findings",
            "capture prompt",
        );

        let dirs = vec![tmp.path().to_str().unwrap().to_string()];
        let registry = PluginRegistry::load(&dirs);

        assert_eq!(registry.len(), 1);

        // Must be findable by FQN
        let plugin = registry.get("second-brain:capture").expect("FQN lookup");
        assert_eq!(plugin.name, "capture");
        assert_eq!(plugin.group, Some("second-brain".to_string()));
        assert_eq!(plugin.fqn, "second-brain:capture");

        // Also findable by bare name (unambiguous)
        let plugin2 = registry.get("capture").expect("bare name lookup");
        assert_eq!(plugin2.fqn, "second-brain:capture");
    }

    #[test]
    fn test_no_collision_different_groups() {
        let tmp = TempDir::new().unwrap();
        make_grouped_skill(tmp.path(), "agent-ready", "init", "AR init", "ar prompt");
        make_grouped_skill(tmp.path(), "fe-agent-ready", "init", "FE init", "fe prompt");

        let dirs = vec![tmp.path().to_str().unwrap().to_string()];
        let registry = PluginRegistry::load(&dirs);

        // Both loaded — no collision
        assert_eq!(registry.len(), 2);

        // Exact FQN works
        let ar = registry.get("agent-ready:init").expect("agent-ready:init");
        assert_eq!(ar.description, "AR init");

        let fe = registry.get("fe-agent-ready:init").expect("fe-agent-ready:init");
        assert_eq!(fe.description, "FE init");

        // Bare "init" is ambiguous — returns None
        assert!(registry.get("init").is_none());
    }

    #[test]
    fn test_get_or_ambiguous() {
        let tmp = TempDir::new().unwrap();
        make_grouped_skill(tmp.path(), "group-a", "init", "A init", "a prompt");
        make_grouped_skill(tmp.path(), "group-b", "init", "B init", "b prompt");

        let dirs = vec![tmp.path().to_str().unwrap().to_string()];
        let registry = PluginRegistry::load(&dirs);

        // FQN works
        assert!(registry.get_or_ambiguous("group-a:init").is_ok());

        // Bare name is ambiguous
        match registry.get_or_ambiguous("init") {
            Err(GetError::Ambiguous(fqns)) => {
                assert_eq!(fqns.len(), 2);
                assert!(fqns.contains(&"group-a:init".to_string()));
                assert!(fqns.contains(&"group-b:init".to_string()));
            }
            other => panic!("Expected Ambiguous, got: {:?}", other),
        }

        // Nonexistent
        assert_eq!(
            registry.get_or_ambiguous("nonexistent"),
            Err(GetError::NotFound)
        );
    }

    #[test]
    fn test_get_group_commands() {
        let tmp = TempDir::new().unwrap();
        make_grouped_skill(tmp.path(), "second-brain", "capture", "Cap", "p1");
        make_grouped_skill(tmp.path(), "second-brain", "process", "Proc", "p2");
        make_grouped_skill(tmp.path(), "second-brain", "setup", "Set", "p3");
        make_grouped_skill(tmp.path(), "other-group", "foo", "Foo", "p4");

        let dirs = vec![tmp.path().to_str().unwrap().to_string()];
        let registry = PluginRegistry::load(&dirs);

        let cmds = registry.get_group_commands("second-brain");
        assert_eq!(cmds.len(), 3);
        let fqns: Vec<&str> = cmds.iter().map(|p| p.fqn.as_str()).collect();
        assert!(fqns.contains(&"second-brain:capture"));
        assert!(fqns.contains(&"second-brain:process"));
        assert!(fqns.contains(&"second-brain:setup"));

        // Other group
        let other = registry.get_group_commands("other-group");
        assert_eq!(other.len(), 1);

        // Nonexistent group
        let empty = registry.get_group_commands("nope");
        assert!(empty.is_empty());
    }

    #[test]
    fn test_groups() {
        let tmp = TempDir::new().unwrap();
        make_grouped_skill(tmp.path(), "second-brain", "capture", "Cap", "p1");
        make_grouped_skill(tmp.path(), "agent-ready", "init", "Init", "p2");
        make_skill(tmp.path(), "flat-plugin", "Flat", "p3");

        let dirs = vec![tmp.path().to_str().unwrap().to_string()];
        let registry = PluginRegistry::load(&dirs);

        let groups = registry.groups();
        assert_eq!(groups, vec!["agent-ready", "second-brain"]);
    }

    #[test]
    fn test_list_includes_fqn_and_group() {
        let tmp = TempDir::new().unwrap();
        make_grouped_skill(tmp.path(), "second-brain", "capture", "Cap", "p1");
        make_skill(tmp.path(), "flat-plugin", "Flat", "p2");

        let dirs = vec![tmp.path().to_str().unwrap().to_string()];
        let registry = PluginRegistry::load(&dirs);

        let list = registry.list();
        assert_eq!(list.len(), 2);

        // Sorted by FQN
        assert_eq!(list[0].0, "flat-plugin");
        assert!(list[0].2.is_none());

        assert_eq!(list[1].0, "second-brain:capture");
        assert_eq!(list[1].2, Some("second-brain".to_string()));
    }

    #[test]
    fn test_mixed_flat_and_grouped() {
        let tmp = TempDir::new().unwrap();
        make_grouped_skill(tmp.path(), "second-brain", "capture", "Cap", "p1");
        make_grouped_skill(tmp.path(), "second-brain", "process", "Proc", "p2");
        make_skill(tmp.path(), "oncall-debugger", "Debug", "p3");

        let dirs = vec![tmp.path().to_str().unwrap().to_string()];
        let registry = PluginRegistry::load(&dirs);

        assert_eq!(registry.len(), 3);

        // FQN lookups
        assert!(registry.get("second-brain:capture").is_some());
        assert!(registry.get("second-brain:process").is_some());
        assert!(registry.get("oncall-debugger").is_some());

        // Bare name for unambiguous grouped plugin
        assert!(registry.get("capture").is_some());

        // Bare name for flat plugin
        assert!(registry.get("oncall-debugger").is_some());
    }

    // ── Companion file tests ─────────────────────────────────────────────────

    #[test]
    fn test_extract_frontmatter_list() {
        let fm = "name: test\ndescription: Test\ncompanion-files:\n  - A.md\n  - B.md\n  - C.md";
        let list = extract_frontmatter_list(fm, "companion-files");
        assert_eq!(list, vec!["A.md", "B.md", "C.md"]);
    }

    #[test]
    fn test_extract_frontmatter_list_empty() {
        let fm = "name: test\ndescription: Test";
        let list = extract_frontmatter_list(fm, "companion-files");
        assert!(list.is_empty());
    }

    #[test]
    fn test_extract_frontmatter_list_quoted() {
        let fm = "name: test\ncompanion-files:\n  - \"A.md\"\n  - 'B.md'";
        let list = extract_frontmatter_list(fm, "companion-files");
        assert_eq!(list, vec!["A.md", "B.md"]);
    }

    #[test]
    fn test_companion_files_loaded() {
        let tmp = TempDir::new().unwrap();
        let skill_dir = tmp.path().join("my-plugin");
        fs::create_dir_all(&skill_dir).unwrap();

        // Write SKILL.md with companion-files
        let mut f = fs::File::create(skill_dir.join("SKILL.md")).unwrap();
        write!(
            f,
            "---\nname: my-plugin\ndescription: Test\ncompanion-files:\n  - TREE.md\n  - PROMPT.md\n---\n\nBase prompt."
        ).unwrap();

        // Write companion files
        fs::write(skill_dir.join("TREE.md"), "# Decision Tree\nClassify here.").unwrap();
        fs::write(skill_dir.join("PROMPT.md"), "# System Prompt\nYou are a bot.").unwrap();

        let dirs = vec![tmp.path().to_str().unwrap().to_string()];
        let registry = PluginRegistry::load(&dirs);

        let plugin = registry.get("my-plugin").unwrap();
        assert!(plugin.system_prompt.contains("Base prompt."));
        assert!(plugin.system_prompt.contains("# TREE.md"));
        assert!(plugin.system_prompt.contains("Classify here."));
        assert!(plugin.system_prompt.contains("# PROMPT.md"));
        assert!(plugin.system_prompt.contains("You are a bot."));
    }

    #[test]
    fn test_no_companion_files_backward_compat() {
        let tmp = TempDir::new().unwrap();
        let skill_dir = tmp.path().join("my-plugin");
        fs::create_dir_all(&skill_dir).unwrap();

        // SKILL.md without companion-files field
        let mut f = fs::File::create(skill_dir.join("SKILL.md")).unwrap();
        write!(f, "---\nname: my-plugin\ndescription: Test\n---\n\nJust the base.").unwrap();

        // Even if extra .md files exist, they should NOT be loaded
        fs::write(skill_dir.join("EXTRA.md"), "Should not appear").unwrap();

        let dirs = vec![tmp.path().to_str().unwrap().to_string()];
        let registry = PluginRegistry::load(&dirs);

        let plugin = registry.get("my-plugin").unwrap();
        assert_eq!(plugin.system_prompt, "Just the base.");
        assert!(!plugin.system_prompt.contains("Should not appear"));
    }

    #[test]
    fn test_companion_file_missing_skipped() {
        let tmp = TempDir::new().unwrap();
        let skill_dir = tmp.path().join("my-plugin");
        fs::create_dir_all(&skill_dir).unwrap();

        let mut f = fs::File::create(skill_dir.join("SKILL.md")).unwrap();
        write!(
            f,
            "---\nname: my-plugin\ndescription: Test\ncompanion-files:\n  - EXISTS.md\n  - MISSING.md\n---\n\nBase."
        ).unwrap();

        fs::write(skill_dir.join("EXISTS.md"), "I exist.").unwrap();
        // MISSING.md intentionally not created

        let dirs = vec![tmp.path().to_str().unwrap().to_string()];
        let registry = PluginRegistry::load(&dirs);

        let plugin = registry.get("my-plugin").unwrap();
        assert!(plugin.system_prompt.contains("I exist."));
        assert!(!plugin.system_prompt.contains("MISSING.md"));
    }

    #[test]
    fn test_companion_files_ordering() {
        let tmp = TempDir::new().unwrap();
        let skill_dir = tmp.path().join("my-plugin");
        fs::create_dir_all(&skill_dir).unwrap();

        let mut f = fs::File::create(skill_dir.join("SKILL.md")).unwrap();
        write!(
            f,
            "---\nname: my-plugin\ndescription: Test\ncompanion-files:\n  - FIRST.md\n  - SECOND.md\n  - THIRD.md\n---\n\nBase."
        ).unwrap();

        fs::write(skill_dir.join("FIRST.md"), "content-first").unwrap();
        fs::write(skill_dir.join("SECOND.md"), "content-second").unwrap();
        fs::write(skill_dir.join("THIRD.md"), "content-third").unwrap();

        let dirs = vec![tmp.path().to_str().unwrap().to_string()];
        let registry = PluginRegistry::load(&dirs);

        let plugin = registry.get("my-plugin").unwrap();
        let first_pos = plugin.system_prompt.find("content-first").unwrap();
        let second_pos = plugin.system_prompt.find("content-second").unwrap();
        let third_pos = plugin.system_prompt.find("content-third").unwrap();
        assert!(first_pos < second_pos);
        assert!(second_pos < third_pos);
    }
}

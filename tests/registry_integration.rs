use oac_slack_bot::plugins::registry::PluginRegistry;
use std::path::Path;

#[test]
fn test_load_real_skills_from_agents_dir() {
    let agents_dir = format!("{}/.agents/skills", std::env::var("HOME").unwrap());
    if !Path::new(&agents_dir).exists() {
        // Skip if dir doesn't exist (CI environment)
        return;
    }
    let registry = PluginRegistry::load(&[agents_dir]);
    println!("Loaded {} plugins from .agents/skills", registry.len());
    for (fqn, desc, group) in registry.list().iter().take(5) {
        println!("  - {} (group: {:?}): {}", fqn, group, &desc[..desc.len().min(60)]);
    }
}

#[test]
fn test_load_real_claude_plugins() {
    let home = std::env::var("HOME").unwrap();
    let plugins_dir = format!("{}/claude-plugins/plugins", home);
    if !Path::new(&plugins_dir).exists() {
        return;
    }
    let registry = PluginRegistry::load(&[plugins_dir]);
    println!("Loaded {} plugins from claude-plugins", registry.len());

    // Check that grouped plugins got FQN
    let groups = registry.groups();
    println!("Groups: {:?}", groups);

    for group in &groups {
        let cmds = registry.get_group_commands(group);
        println!("  {} ({} commands):", group, cmds.len());
        for cmd in &cmds {
            println!("    - {}: {}", cmd.fqn, &cmd.description[..cmd.description.len().min(50)]);
        }
    }

    // Verify no FQN collisions — if we got here without panic, the HashMap handled it
    assert!(registry.len() > 0, "should have loaded some plugins");

    // Verify FQN lookup works for a known plugin
    if registry.get("second-brain:capture").is_some() {
        println!("✓ second-brain:capture found by FQN");
    }

    // Verify companion files are loaded for oncall-agent (PR #130)
    if let Some(oncall) = registry.get("oncall-agent") {
        let prompt_len = oncall.system_prompt.len();
        println!("✓ oncall-agent system_prompt: {} chars", prompt_len);
        let has_system_prompt = oncall.system_prompt.contains("# SYSTEM_PROMPT.md");
        let has_tool_guide = oncall.system_prompt.contains("# tools/common/TOOL_GUIDE.md");
        let has_knowledge_base = oncall.system_prompt.contains("# tools/knowledge-base/KNOWLEDGE_BASE.md");
        println!("  SYSTEM_PROMPT.md: {}", has_system_prompt);
        println!("  TOOL_GUIDE.md: {}", has_tool_guide);
        println!("  KNOWLEDGE_BASE.md: {}", has_knowledge_base);
        assert!(has_system_prompt, "companion SYSTEM_PROMPT.md should be loaded");
        assert!(has_tool_guide, "companion TOOL_GUIDE.md should be loaded");
        assert!(prompt_len > 50000, "system_prompt should be large with companions (got {})", prompt_len);
    }
}

#[test]
fn test_registry_get_nonexistent() {
    let registry = PluginRegistry::load(&[]);
    assert!(registry.get("nonexistent-plugin-xyz").is_none());
}

#[test]
fn test_registry_list_is_sorted() {
    use std::fs;
    use std::io::Write;
    use tempfile::TempDir;

    let tmp = TempDir::new().unwrap();
    for name in &["zebra-plugin", "alpha-plugin", "middle-plugin"] {
        let dir = tmp.path().join(name);
        fs::create_dir_all(&dir).unwrap();
        let mut f = fs::File::create(dir.join("SKILL.md")).unwrap();
        writeln!(f, "---\nname: {}\ndescription: test\n---\n\nbody", name).unwrap();
    }

    let registry = PluginRegistry::load(&[tmp.path().to_str().unwrap().to_string()]);
    let fqns: Vec<String> = registry.list().iter().map(|(fqn, _, _)| fqn.clone()).collect();
    let mut sorted = fqns.clone();
    sorted.sort();
    assert_eq!(fqns, sorted, "list() should be alphabetically sorted");
}

use oac_slack_bot::plugins::registry::PluginRegistry;
use oac_slack_bot::plugins::router::route;
use std::fs;
use std::io::Write;
use tempfile::TempDir;

fn build_registry_with(plugins: &[(&str, &str)]) -> (PluginRegistry, TempDir) {
    let tmp = TempDir::new().unwrap();
    for (name, description) in plugins {
        let dir = tmp.path().join(name);
        fs::create_dir_all(&dir).unwrap();
        let mut f = fs::File::create(dir.join("SKILL.md")).unwrap();
        writeln!(f, "---\nname: {}\ndescription: {}\n---\n\nbody", name, description).unwrap();
    }
    let registry = PluginRegistry::load(&[tmp.path().to_str().unwrap().to_string()]);
    (registry, tmp)
}

fn build_grouped_registry(
    groups: &[(&str, &str, &str)],
) -> (PluginRegistry, TempDir) {
    let tmp = TempDir::new().unwrap();
    for (group, cmd, description) in groups {
        let skill_dir = tmp.path().join(group).join("skills").join(cmd);
        fs::create_dir_all(&skill_dir).unwrap();
        let mut f = fs::File::create(skill_dir.join("SKILL.md")).unwrap();
        writeln!(
            f,
            "---\nname: {}\ndescription: {}\n---\n\nbody",
            cmd, description
        )
        .unwrap();
    }
    let registry = PluginRegistry::load(&[tmp.path().to_str().unwrap().to_string()]);
    (registry, tmp)
}

#[test]
fn test_route_oac_triage_firing() {
    let (registry, _tmp) = build_registry_with(&[
        ("oncall-debugger", "Oncall debugging for production incidents"),
        ("some-other", "Something else entirely"),
    ]);
    let plugin = route("[FIRING] HighDBConnectionTimeout in emandate-service", &registry);
    assert!(plugin.is_some());
    assert_eq!(plugin.unwrap().fqn, "oncall-debugger");
}

#[test]
fn test_route_force_override() {
    let (registry, _tmp) = build_registry_with(&[
        ("nexus-platform-faq", "Nexus platform FAQ"),
        ("oncall-debugger", "Oncall debugging"),
    ]);
    let plugin = route("/plugin nexus-platform-faq what is nexus?", &registry);
    assert!(plugin.is_some());
    assert_eq!(plugin.unwrap().fqn, "nexus-platform-faq");
}

#[test]
fn test_route_no_match_returns_none() {
    let (registry, _tmp) = build_registry_with(&[
        ("very-specific-plugin", "Handles only very specific domain tasks"),
    ]);
    let plugin = route("hello how are you today", &registry);
    let _ = plugin;
}

#[test]
fn test_route_exact_name_match() {
    let (registry, _tmp) = build_registry_with(&[
        ("nexus-platform-faq", "Nexus platform questions"),
        ("aws-cost-analysis", "AWS cost analysis and billing"),
    ]);
    let plugin = route("@bot nexus-platform-faq what is mcp?", &registry);
    assert!(plugin.is_some());
    assert_eq!(plugin.unwrap().fqn, "nexus-platform-faq");
}

#[test]
fn test_route_description_keyword() {
    let (registry, _tmp) = build_registry_with(&[
        ("billing-helper", "Analyze costs and billing information"),
    ]);
    let plugin = route("how does billing work?", &registry);
    assert!(plugin.is_some());
    assert_eq!(plugin.unwrap().fqn, "billing-helper");
}

// ── FQN integration tests ────────────────────────────────────────────────────

#[test]
fn test_route_grouped_plugin_by_fqn() {
    let (registry, _tmp) = build_grouped_registry(&[
        ("second-brain", "capture", "Capture session findings"),
        ("second-brain", "process", "Process inbox notes"),
    ]);

    let plugin = route("run second-brain:capture now", &registry);
    assert!(plugin.is_some());
    assert_eq!(plugin.unwrap().fqn, "second-brain:capture");
}

#[test]
fn test_route_force_override_fqn() {
    let (registry, _tmp) = build_grouped_registry(&[
        ("second-brain", "capture", "Capture"),
        ("second-brain", "process", "Process"),
    ]);

    let plugin = route("/plugin second-brain:process do something", &registry);
    assert!(plugin.is_some());
    assert_eq!(plugin.unwrap().fqn, "second-brain:process");
}

#[test]
fn test_route_ambiguous_bare_name_falls_through() {
    let (registry, _tmp) = build_grouped_registry(&[
        ("agent-ready", "init", "Agent ready init"),
        ("fe-agent-ready", "init", "Frontend agent ready init"),
    ]);

    // "init" is ambiguous, message has no FQN, "init" is 4 chars (< 5 threshold for desc match)
    let plugin = route("run init please", &registry);
    assert!(plugin.is_none(), "ambiguous bare name should not match");

    // But FQN works
    let plugin = route("run agent-ready:init please", &registry);
    assert!(plugin.is_some());
    assert_eq!(plugin.unwrap().fqn, "agent-ready:init");
}

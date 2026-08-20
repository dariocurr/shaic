use shaic_core::adapters::opencode::OpenCode;
use shaic_core::materialize::{
    apply, apply_mcp, plan_materialize, plan_mcp, reconcile_items, reconcile_mcp,
};
use shaic_core::mcp::McpServer;
use shaic_core::model::{AgentId, Frontmatter, Item, ItemKind, Scope};
use shaic_core::store::Store;
use std::collections::BTreeMap;

fn item(kind: ItemKind, name: &str) -> Item {
    Item::new(
        kind,
        Frontmatter {
            name: name.to_string(),
            description: "test item".to_string(),
            applies_to: vec![],
            tags: vec![],
            scope: vec![Scope::Project, Scope::Global],
            agents: AgentId::ALL.to_vec(),
        },
        "Do the thing.".to_string(),
    )
    .unwrap()
}

/// OpenCode's layout (confirmed against its docs): global under
/// `$XDG_CONFIG_HOME/opencode` (or `~/.config/opencode`), project skills/
/// commands under `.opencode/`, rules in `AGENTS.md`, MCP in `opencode.json`
/// with OpenCode's `type`/`command`-as-array shape. One test function so
/// `HOME`/`XDG_CONFIG_HOME` env mutations don't race other tests in this
/// binary.
#[test]
fn opencode_materialize_and_reconcile() {
    let home = tempfile::tempdir().unwrap();
    let xdg = home.path().join(".config");
    std::fs::create_dir_all(&xdg).unwrap();
    unsafe {
        std::env::set_var("HOME", home.path());
        std::env::set_var("XDG_CONFIG_HOME", &xdg);
    }
    let project = tempfile::tempdir().unwrap();
    let store = Store::init(home.path().join("store"), None).unwrap();
    let agent = OpenCode;

    store
        .save_item(&item(ItemKind::Skill, "weather-lookup"))
        .unwrap();
    store
        .save_item(&item(ItemKind::Command, "ship-it"))
        .unwrap();
    store.save_item(&item(ItemKind::Rule, "no-any")).unwrap();

    for &scope in &[Scope::Global, Scope::Project] {
        let plan = plan_materialize(&agent, &store, scope, project.path()).unwrap();
        apply(&agent, &plan, scope, project.path()).unwrap();
    }

    assert!(
        xdg.join("opencode/skills/weather-lookup/SKILL.md").exists(),
        "Global skills under ~/.config/opencode/skills/"
    );
    assert!(
        xdg.join("opencode/commands/ship-it.md").exists(),
        "Global commands under ~/.config/opencode/commands/"
    );
    assert!(
        xdg.join("opencode/AGENTS.md").exists(),
        "Global rules at ~/.config/opencode/AGENTS.md"
    );
    assert!(
        project
            .path()
            .join(".opencode/skills/weather-lookup/SKILL.md")
            .exists(),
        "Project skills under .opencode/skills/"
    );
    assert!(
        project
            .path()
            .join(".opencode/commands/ship-it.md")
            .exists(),
        "Project commands under .opencode/commands/"
    );
    assert!(
        project.path().join("AGENTS.md").exists(),
        "Project rules at AGENTS.md (shared with Codex)"
    );

    let skill_dir = project.path().join(".opencode/skills/hand-written");
    std::fs::create_dir_all(&skill_dir).unwrap();
    std::fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: hand-written\ndescription: written by hand\n---\n\nDo the other thing.\n",
    )
    .unwrap();

    let report = reconcile_items(
        &agent,
        &store,
        ItemKind::Skill,
        Scope::Project,
        project.path(),
    )
    .unwrap();
    assert_eq!(report.pulled, vec!["hand-written".to_string()]);

    // MCP stdio → OpenCode `type: local` with command-as-array.
    store
        .save_mcp_server(
            &McpServer::new(
                "playwright".to_string(),
                "npx".to_string(),
                vec!["-y".to_string(), "@playwright/mcp@latest".to_string()],
                BTreeMap::new(),
                vec![Scope::Project],
            )
            .unwrap(),
        )
        .unwrap();

    let plan = plan_mcp(&agent, &store, Scope::Project, project.path()).unwrap();
    apply_mcp(&agent, &store, &plan, Scope::Project, project.path()).unwrap();

    let raw = std::fs::read_to_string(project.path().join("opencode.json")).unwrap();
    let value: serde_json::Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(value["mcp"]["playwright"]["type"], "local");
    assert_eq!(
        value["mcp"]["playwright"]["command"],
        serde_json::json!(["npx", "-y", "@playwright/mcp@latest"])
    );

    // Hand-add an OpenCode-native MCP entry and reconcile it back.
    std::fs::write(
        project.path().join("opencode.json"),
        r#"{
  "model": "anthropic/claude-sonnet-4-5",
  "mcp": {
    "hand-mcp": {
      "type": "local",
      "command": ["bun", "x", "hand-mcp"]
    }
  }
}"#,
    )
    .unwrap();
    let report = reconcile_mcp(&agent, &store, Scope::Project, project.path()).unwrap();
    assert!(
        report.pulled.iter().any(|n| n == "hand-mcp"),
        "expected hand-mcp pulled, got {:?}",
        report.pulled
    );
    let loaded = store.load_mcp_server("hand-mcp").unwrap();
    assert_eq!(loaded.command, "bun");
    assert_eq!(loaded.args, vec!["x", "hand-mcp"]);

    // Global MCP must work when only XDG_CONFIG_HOME is set (no HOME).
    unsafe {
        std::env::remove_var("HOME");
        std::env::remove_var("USERPROFILE");
    }
    store
        .save_mcp_server(
            &McpServer::new(
                "xdg-only".to_string(),
                "npx".to_string(),
                vec!["-y".to_string(), "xdg-tool".to_string()],
                BTreeMap::new(),
                vec![Scope::Global],
            )
            .unwrap(),
        )
        .unwrap();
    let plan = plan_mcp(&agent, &store, Scope::Global, project.path()).unwrap();
    apply_mcp(&agent, &store, &plan, Scope::Global, project.path()).unwrap();
    let global_mcp = xdg.join("opencode/opencode.json");
    assert!(
        global_mcp.exists(),
        "global OpenCode MCP must write under XDG without HOME"
    );
    let raw = std::fs::read_to_string(&global_mcp).unwrap();
    assert!(raw.contains("xdg-only"), "{raw}");
}

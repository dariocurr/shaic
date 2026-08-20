use shaic_core::adapters::codex::Codex;
use shaic_core::adapters::opencode::OpenCode;
use shaic_core::materialize::{apply, plan_materialize};
use shaic_core::model::{AgentId, Frontmatter, Item, ItemKind, Scope};
use shaic_core::store::Store;

fn rule(name: &str, agents: Vec<AgentId>) -> Item {
    Item::new(
        ItemKind::Rule,
        Frontmatter {
            name: name.to_string(),
            description: "test rule".to_string(),
            applies_to: vec![],
            tags: vec![],
            scope: vec![Scope::Project],
            agents,
        },
        format!("Body for {name}."),
    )
    .unwrap()
}

/// Codex and OpenCode share project `AGENTS.md`. A sync of either must keep
/// rules aimed at the other writer so the last agent does not wipe them.
#[test]
fn shared_agents_md_keeps_codex_and_opencode_rules() {
    let home = tempfile::tempdir().unwrap();
    let xdg = home.path().join(".config");
    std::fs::create_dir_all(&xdg).unwrap();
    unsafe {
        std::env::set_var("HOME", home.path());
        std::env::set_var("XDG_CONFIG_HOME", &xdg);
    }
    let project = tempfile::tempdir().unwrap();
    let store = Store::init(home.path().join("store"), None).unwrap();

    store
        .save_item(&rule("codex-only", vec![AgentId::Codex]))
        .unwrap();
    store
        .save_item(&rule("opencode-only", vec![AgentId::OpenCode]))
        .unwrap();

    let plan = plan_materialize(&Codex, &store, Scope::Project, project.path()).unwrap();
    apply(&Codex, &plan, Scope::Project, project.path()).unwrap();
    let after_codex = std::fs::read_to_string(project.path().join("AGENTS.md")).unwrap();
    assert!(after_codex.contains("## codex-only"));
    assert!(
        after_codex.contains("## opencode-only"),
        "Codex sync must keep OpenCode-targeted rules in shared AGENTS.md: {after_codex}"
    );

    let plan = plan_materialize(&OpenCode, &store, Scope::Project, project.path()).unwrap();
    apply(&OpenCode, &plan, Scope::Project, project.path()).unwrap();
    let after_opencode = std::fs::read_to_string(project.path().join("AGENTS.md")).unwrap();
    assert!(after_opencode.contains("## codex-only"));
    assert!(after_opencode.contains("## opencode-only"));
}

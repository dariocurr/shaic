use shaic_core::adapters::claude_code::ClaudeCode;
use shaic_core::adapters::copilot::Copilot;
use shaic_core::materialize::{reconcile_items, reconcile_mcp};
use shaic_core::model::{ItemKind, Scope};
use shaic_core::store::Store;

/// Regression coverage flagged by review: `reconcile_mcp`'s report doc says
/// a hand-typed literal credential is caught by `Store::save_mcp_server`'s
/// secret-scan tripwire, but nothing previously exercised that path with a
/// real obviously-shaped secret — every existing bidirectional-sync test
/// only asserted `rejected` stayed *empty*. This pins the actual rejection.
///
/// Sets `HOME` for the process (provenance manifests live under it).
#[test]
fn reconcile_mcp_rejects_a_literal_secret_shaped_env_value() {
    let home = tempfile::tempdir().unwrap();
    unsafe {
        std::env::set_var("HOME", home.path());
    }
    let store = Store::init(home.path().join("store"), None).unwrap();
    let project = tempfile::tempdir().unwrap();

    // Hand-add a server directly in Claude Code's project `.mcp.json` with
    // an obviously AWS-key-shaped literal in `env` — never routed through
    // `shaic mcp secret set`.
    let mcp_path = project.path().join(".mcp.json");
    std::fs::write(
        &mcp_path,
        r#"{"mcpServers":{"leaky":{"command":"npx","args":["-y","leaky-tool"],"env":{"AWS_ACCESS_KEY_ID":"AKIAABCDEFGHIJKLMNOP"}}}}"#,
    )
    .unwrap();

    let claude = ClaudeCode;
    let report = reconcile_mcp(&claude, &store, Scope::Project, project.path()).unwrap();

    assert!(
        report.pulled.is_empty(),
        "a literal secret-shaped credential must not be pulled into the store: {:?}",
        report.pulled
    );
    assert_eq!(report.rejected.len(), 1);
    assert_eq!(report.rejected[0].0, "leaky");
    assert!(
        report.rejected[0].1.contains("secret"),
        "rejection reason should mention the secret-scan hit: {:?}",
        report.rejected[0]
    );
    assert!(
        store.load_mcp_server("leaky").is_err(),
        "the rejected server must not have been written into the store"
    );
}

/// Copilot's Skill/Command filenames carry a second, meaningful extension
/// segment (`.instructions.md`, `.prompt.md`) that `strip_file_suffix` has
/// to peel off explicitly. This is exactly the class of string-handling bug
/// that caused the Cursor/Windsurf/Cline Skill-vs-Rule duplicate-item bug
/// found earlier by hand-testing, so it gets a dedicated round-trip test
/// rather than only incidental coverage via the two other adapters that
/// happen to exercise `reconcile_existing` today.
#[test]
fn copilot_reconciles_skill_and_command_suffixes_and_skips_malformed_names() {
    let home = tempfile::tempdir().unwrap();
    unsafe {
        std::env::set_var("HOME", home.path());
    }
    let store = Store::init(home.path().join("store"), None).unwrap();
    let project = tempfile::tempdir().unwrap();

    let instructions_dir = project.path().join(".github").join("instructions");
    std::fs::create_dir_all(&instructions_dir).unwrap();
    std::fs::write(
        instructions_dir.join("react-conventions.instructions.md"),
        "---\napplyTo: \"**/*.tsx\"\n---\n\nPrefer function components.\n",
    )
    .unwrap();
    // Malformed: missing the `.instructions.md` compound suffix entirely —
    // must be silently skipped, not misparsed into a wrong name.
    std::fs::write(
        instructions_dir.join("stray.md"),
        "not a real instructions file\n",
    )
    .unwrap();

    let prompts_dir = project.path().join(".github").join("prompts");
    std::fs::create_dir_all(&prompts_dir).unwrap();
    std::fs::write(
        prompts_dir.join("release-checklist.prompt.md"),
        "---\ndescription: Cut a release.\n---\n\nRun the checklist.\n",
    )
    .unwrap();

    let copilot = Copilot;

    let skill_report = reconcile_items(
        &copilot,
        &store,
        ItemKind::Skill,
        Scope::Project,
        project.path(),
    )
    .unwrap();
    assert_eq!(skill_report.pulled, vec!["react-conventions".to_string()]);
    let skill = store
        .load_item(ItemKind::Skill, "react-conventions")
        .unwrap();
    assert_eq!(skill.frontmatter.applies_to, vec!["**/*.tsx".to_string()]);
    assert_eq!(skill.body.trim(), "Prefer function components.");
    assert!(
        store.load_item(ItemKind::Skill, "stray").is_err(),
        "a file missing the compound suffix must not become a phantom item"
    );

    let command_report = reconcile_items(
        &copilot,
        &store,
        ItemKind::Command,
        Scope::Project,
        project.path(),
    )
    .unwrap();
    assert_eq!(command_report.pulled, vec!["release-checklist".to_string()]);
    let command = store
        .load_item(ItemKind::Command, "release-checklist")
        .unwrap();
    assert_eq!(command.frontmatter.description, "Cut a release.");
    assert_eq!(command.body.trim(), "Run the checklist.");
}

use shaic_core::adapters::Agent;
use shaic_core::adapters::claude_code::ClaudeCode;
use shaic_core::adapters::cursor::Cursor;
use shaic_core::materialize::{apply, plan_materialize, reconcile_items};
use shaic_core::model::{ItemKind, Scope};
use shaic_core::store::Store;

/// Regression coverage for a real bug found while dogfooding bidirectional
/// item sync: Cursor (and Windsurf/Cline) render Skill and Rule into the
/// *same* on-disk files, so a naive reconcile that just reads "whatever's on
/// disk" mistakes its own just-written Skill-kind output for new Rule-kind
/// content on the very next sync — creating a phantom duplicate item that
/// then double-writes the same target file forever. The fix filters
/// reconciled content through the provenance manifest first.
///
/// Sets `HOME` for the process (`Store::state_dir()` lives under it).
#[test]
fn item_bidirectional_sync_is_idempotent_and_avoids_duplicate_writes() {
    let home = tempfile::tempdir().unwrap();
    unsafe {
        std::env::set_var("HOME", home.path());
    }
    let store = Store::init(home.path().join("store"), None).unwrap();
    let project = tempfile::tempdir().unwrap();

    // Hand-add a skill directly in Claude Code's own directory, bypassing
    // `shaic item add` entirely.
    let skill_dir = project
        .path()
        .join(".claude")
        .join("skills")
        .join("weather-lookup");
    std::fs::create_dir_all(&skill_dir).unwrap();
    std::fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: weather-lookup\ndescription: Look up current weather\n---\n\nUse the weather API.\n",
    )
    .unwrap();

    let claude = ClaudeCode;
    let cursor = Cursor;

    // Reconcile + apply for Claude: pulls it into the store, writes it back
    // out (re-normalizing formatting).
    for &kind in claude.supported_kinds() {
        let report =
            reconcile_items(&claude, &store, kind, Scope::Project, project.path()).unwrap();
        if kind == ItemKind::Skill {
            assert_eq!(report.pulled, vec!["weather-lookup".to_string()]);
        }
    }
    let plan = plan_materialize(&claude, &store, Scope::Project, project.path()).unwrap();
    apply(&claude, &plan, Scope::Project, project.path()).unwrap();

    // Reconcile + apply for Cursor: the store's Skill-kind item renders into
    // `rules/weather-lookup.mdc`.
    for &kind in cursor.supported_kinds() {
        reconcile_items(&cursor, &store, kind, Scope::Project, project.path()).unwrap();
    }
    let cursor_plan = plan_materialize(&cursor, &store, Scope::Project, project.path()).unwrap();
    apply(&cursor, &cursor_plan, Scope::Project, project.path()).unwrap();

    // Second full pass: Cursor's Rule-kind reconcile must not reinterpret
    // its own just-written Skill-kind output as new Rule content.
    for &kind in cursor.supported_kinds() {
        let report =
            reconcile_items(&cursor, &store, kind, Scope::Project, project.path()).unwrap();
        assert!(
            report.pulled.is_empty(),
            "kind {kind:?} should have nothing left to pull on a second pass: {:?}",
            report.pulled
        );
    }
    let cursor_plan2 = plan_materialize(&cursor, &store, Scope::Project, project.path()).unwrap();
    assert!(
        cursor_plan2.is_empty(),
        "second Cursor plan should be a no-op: {:?}",
        cursor_plan2.writes
    );

    assert!(
        store.load_item(ItemKind::Rule, "weather-lookup").is_err(),
        "must not have created a duplicate Rule-kind item from Cursor's own Skill-kind output"
    );
}

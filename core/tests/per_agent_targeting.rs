use shaic_core::adapters::claude_code::ClaudeCode;
use shaic_core::adapters::cursor::Cursor;
use shaic_core::materialize::{apply, plan_materialize};
use shaic_core::model::{AgentId, Frontmatter, Item, ItemKind, Scope};
use shaic_core::store::Store;

fn skill(name: &str, agents: Vec<AgentId>) -> Item {
    Item::new(
        ItemKind::Skill,
        Frontmatter {
            name: name.to_string(),
            description: "test skill".to_string(),
            applies_to: vec![],
            tags: vec![],
            scope: vec![Scope::Project],
            agents,
        },
        "Do the thing.".to_string(),
    )
    .unwrap()
}

/// Covers per-agent targeting end to end: an item restricted to one agent
/// never materializes for another, and narrowing an already-materialized
/// item's `agents` cleans up the now-excluded agent's stale copy the same
/// delete-safety-gated way any other kind of drop already does.
///
/// One `#[test]` function, not two: both cases need their own `HOME`, and
/// integration-test functions in the same binary run concurrently by
/// default, so two functions each calling `std::env::set_var("HOME", ...)`
/// race each other (same lesson as `push_all_now_after_delete.rs`).
#[test]
fn per_agent_targeting_restricts_materialize_and_cleans_up_on_narrowing() {
    let home = tempfile::tempdir().unwrap();
    unsafe {
        std::env::set_var("HOME", home.path());
    }
    let project = tempfile::tempdir().unwrap();
    let store = Store::init(home.path().join("store"), None).unwrap();
    let claude = ClaudeCode;
    let cursor = Cursor;

    store
        .save_item(&skill("claude-only", vec![AgentId::ClaudeCode]))
        .unwrap();

    let claude_plan = plan_materialize(&claude, &store, Scope::Project, project.path()).unwrap();
    assert_eq!(claude_plan.changed_writes().count(), 1);

    let cursor_plan = plan_materialize(&cursor, &store, Scope::Project, project.path()).unwrap();
    assert_eq!(
        cursor_plan.changed_writes().count(),
        0,
        "an item not targeting Cursor must produce no write for Cursor"
    );

    store
        .save_item(&skill("everywhere", AgentId::ALL.to_vec()))
        .unwrap();
    let plan = plan_materialize(&cursor, &store, Scope::Project, project.path()).unwrap();
    apply(&cursor, &plan, Scope::Project, project.path()).unwrap();
    let materialized = project.path().join(".cursor/rules/everywhere.mdc");
    assert!(materialized.exists());

    store
        .save_item(&skill("everywhere", vec![AgentId::ClaudeCode]))
        .unwrap();
    let plan = plan_materialize(&cursor, &store, Scope::Project, project.path()).unwrap();
    assert_eq!(
        plan.deletes.len(),
        1,
        "Cursor should see its stale copy queued for deletion"
    );
    apply(&cursor, &plan, Scope::Project, project.path()).unwrap();
    assert!(
        !materialized.exists(),
        "Cursor's copy should be gone once the item no longer targets Cursor"
    );
}

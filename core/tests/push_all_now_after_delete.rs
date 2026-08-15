use shaic_core::adapters::claude_code::ClaudeCode;
use shaic_core::materialize::{apply, plan_materialize, push_all_now};
use shaic_core::model::{AgentId, Frontmatter, Item, ItemKind, Scope};
use shaic_core::store::Store;

fn skill(name: &str) -> Item {
    Item::new(
        ItemKind::Skill,
        Frontmatter {
            name: name.to_string(),
            description: "test skill".to_string(),
            applies_to: vec![],
            tags: vec![],
            scope: vec![Scope::Project],
            agents: AgentId::ALL.to_vec(),
        },
        "Do the thing.".to_string(),
    )
    .unwrap()
}

/// `push_all_now` is what `item rm`/the TUI's delete key call right after
/// removing an item from the store, so a deletion reaches disk in the same
/// action instead of waiting for a separate `sync`. Covers the wiring across
/// every agent/scope, not just the underlying delete-safety primitive
/// (already covered by `materialize_delete_safety.rs`): a shaic-written file
/// must actually disappear, and a file shaic never wrote must be left alone
/// even though the same function pass touches every agent unconditionally.
/// Sets `HOME` for the process (provenance manifests live under it) — must
/// stay the only test in this binary to avoid racing that env var.
#[test]
fn push_all_now_deletes_owned_file_and_leaves_unowned_file_alone() {
    let home = tempfile::tempdir().unwrap();
    unsafe {
        std::env::set_var("HOME", home.path());
    }
    let project = tempfile::tempdir().unwrap();
    let store = Store::init(home.path().join("store"), None).unwrap();
    let agent = ClaudeCode;

    // A file shaic never wrote — discovered on disk before shaic ever
    // touched this project — must survive `push_all_now` no matter what.
    let unowned = project.path().join(".claude/skills/hand-written/SKILL.md");
    std::fs::create_dir_all(unowned.parent().unwrap()).unwrap();
    std::fs::write(&unowned, "pre-existing, not shaic's").unwrap();

    store.save_item(&skill("temp-skill")).unwrap();
    let plan = plan_materialize(&agent, &store, Scope::Project, project.path()).unwrap();
    apply(&agent, &plan, Scope::Project, project.path()).unwrap();
    let owned = project.path().join(".claude/skills/temp-skill/SKILL.md");
    assert!(owned.exists(), "sync should have created the skill file");

    store.remove_item(ItemKind::Skill, "temp-skill").unwrap();
    let (applied, notes) = push_all_now(&store, project.path());
    assert!(notes.is_empty(), "unexpected notes: {notes:?}");
    assert!(applied > 0, "at least the owning agent/scope should apply");

    assert!(
        !owned.exists(),
        "push_all_now should delete the file for the item just removed"
    );
    assert_eq!(
        std::fs::read_to_string(&unowned).unwrap(),
        "pre-existing, not shaic's",
        "push_all_now must never touch a file shaic didn't write, even while \
         sweeping every agent/scope unconditionally"
    );
}

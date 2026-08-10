use shaic_core::adapters::claude_code::ClaudeCode;
use shaic_core::materialize::{apply, plan_materialize};
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

/// Exercises `plan_materialize`/`apply`'s delete path end to end (not just
/// `Manifest::safe_to_delete` in isolation): a shaic-written, untouched file
/// must be cleaned up once its item is removed from the store, while a
/// hand-edited file at the same path must survive — the entire reason the
/// provenance manifest exists. Sets `HOME` for the process since the manifest
/// lives under `Store::state_dir()`, which resolves off it; this must stay
/// the only test in this binary to avoid racing that env var across threads.
#[test]
fn delete_safety_survives_orchestration_and_protects_hand_edits() {
    let home = tempfile::tempdir().unwrap();
    unsafe {
        std::env::set_var("HOME", home.path());
    }
    let project = tempfile::tempdir().unwrap();
    let store = Store::init(home.path().join("store"), None).unwrap();
    let agent = ClaudeCode;

    // Untouched shaic-written file: safe to delete once the item is gone.
    store.save_item(&skill("temp-skill")).unwrap();
    let plan = plan_materialize(&agent, &store, Scope::Project, project.path()).unwrap();
    apply(&agent, &plan, Scope::Project, project.path()).unwrap();
    let file = project.path().join(".claude/skills/temp-skill/SKILL.md");
    assert!(file.exists(), "sync should have created the skill file");

    store.remove_item(ItemKind::Skill, "temp-skill").unwrap();
    let plan2 = plan_materialize(&agent, &store, Scope::Project, project.path()).unwrap();
    assert_eq!(
        plan2.deletes.len(),
        1,
        "removed item should be exactly one delete candidate"
    );
    apply(&agent, &plan2, Scope::Project, project.path()).unwrap();
    assert!(
        !file.exists(),
        "manifest-tracked, untouched file should be deleted"
    );

    // Hand-edited file at the same relative path must survive, even after
    // its item is removed from the store.
    store.save_item(&skill("kept-skill")).unwrap();
    let plan3 = plan_materialize(&agent, &store, Scope::Project, project.path()).unwrap();
    apply(&agent, &plan3, Scope::Project, project.path()).unwrap();
    let kept_file = project.path().join(".claude/skills/kept-skill/SKILL.md");
    assert!(kept_file.exists());

    std::fs::write(&kept_file, "hand-edited content, not shaic's").unwrap();
    store.remove_item(ItemKind::Skill, "kept-skill").unwrap();
    let plan4 = plan_materialize(&agent, &store, Scope::Project, project.path()).unwrap();
    assert!(
        plan4.deletes.is_empty(),
        "hand-edited file must not be proposed for deletion"
    );
    apply(&agent, &plan4, Scope::Project, project.path()).unwrap();
    assert_eq!(
        std::fs::read_to_string(&kept_file).unwrap(),
        "hand-edited content, not shaic's",
        "hand-edited file must survive apply()"
    );
}

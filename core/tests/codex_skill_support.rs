use shaic_core::adapters::codex::Codex;
use shaic_core::materialize::{apply, plan_materialize, reconcile_items};
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
            scope: vec![Scope::Project, Scope::Global],
            agents: AgentId::ALL.to_vec(),
        },
        "Do the thing.".to_string(),
    )
    .unwrap()
}

/// Codex CLI's real skill mechanism (confirmed against its own docs) is a
/// `SKILL.md`-per-directory layout identical to Claude Code's, at
/// `~/.codex/skills/` (Global) and `<project>/.codex/skills/` (Project) — the
/// project path needs the extra `.codex/` prefix `AGENTS.md` doesn't, since
/// `root()` for Project scope is the bare project root. Covers both halves of
/// that asymmetry, plus the reverse (reconcile) direction.
///
/// One `#[test]` function, not two: both cases need their own `HOME`, and
/// integration-test functions in the same binary run concurrently by
/// default, so two functions each calling `std::env::set_var("HOME", ...)`
/// race each other (same lesson as `push_all_now_after_delete.rs`).
#[test]
fn codex_skill_materialize_and_reconcile() {
    let home = tempfile::tempdir().unwrap();
    unsafe {
        std::env::set_var("HOME", home.path());
    }
    let project = tempfile::tempdir().unwrap();
    let store = Store::init(home.path().join("store"), None).unwrap();
    let codex = Codex;

    store.save_item(&skill("weather-lookup")).unwrap();

    for &scope in &[Scope::Global, Scope::Project] {
        let plan = plan_materialize(&codex, &store, scope, project.path()).unwrap();
        apply(&codex, &plan, scope, project.path()).unwrap();
    }

    assert!(
        home.path()
            .join(".codex/skills/weather-lookup/SKILL.md")
            .exists(),
        "Global scope should write under ~/.codex/skills/, not ~/.codex/"
    );
    assert!(
        project
            .path()
            .join(".codex/skills/weather-lookup/SKILL.md")
            .exists(),
        "Project scope should write under <project>/.codex/skills/, not <project>/skills/"
    );

    let skill_dir = project.path().join(".codex/skills/hand-written");
    std::fs::create_dir_all(&skill_dir).unwrap();
    std::fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: hand-written\ndescription: written by hand\n---\n\nDo the other thing.\n",
    )
    .unwrap();

    let report = reconcile_items(
        &codex,
        &store,
        ItemKind::Skill,
        Scope::Project,
        project.path(),
    )
    .unwrap();
    assert_eq!(report.pulled, vec!["hand-written".to_string()]);
    assert!(store.load_item(ItemKind::Skill, "hand-written").is_ok());
}

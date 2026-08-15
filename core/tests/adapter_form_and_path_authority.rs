use shaic_core::adapters::claude_code::ClaudeCode;
use shaic_core::adapters::cursor::Cursor;
use shaic_core::materialize::reconcile_items;
use shaic_core::model::{ItemKind, Scope};
use shaic_core::store::Store;

/// Three confirmed adapter-layer bugs, all about *which* on-disk thing shaic
/// treats as the truth. One `#[test]` function, not three: each needs its own
/// `HOME` (provenance manifests live under it) and integration-test functions
/// in one binary run concurrently, so separate functions would race that env
/// var (same lesson as `push_all_now_after_delete.rs`).
///
/// 1. Legacy single-file and directory form used to be reconciled *together*,
///    with single-file sections processed last — so a stale `.cursorrules`
///    section overwrote the item just reconciled from the matching `.mdc`.
/// 2. A `SKILL.md`'s frontmatter `name:` used to win over its directory name,
///    leaving the store and disk permanently disagreeing.
/// 3. Discovery stopped at two levels deep, so a grouped
///    `skills/<group>/<name>/SKILL.md` was never found at all.
#[test]
fn active_form_and_path_derived_names_decide_what_gets_reconciled() {
    let home = tempfile::tempdir().unwrap();
    unsafe {
        std::env::set_var("HOME", home.path());
    }
    let store = Store::init(home.path().join("store"), None).unwrap();
    let project = tempfile::tempdir().unwrap();

    // ---- 1. Directory form wins over a stale legacy single file ----
    let rules_dir = project.path().join(".cursor").join("rules");
    std::fs::create_dir_all(&rules_dir).unwrap();
    std::fs::write(
        rules_dir.join("no-any.mdc"),
        "---\ndescription: Never use any\nglobs: '**/*.ts'\nalwaysApply: false\n---\n\nCurrent body from the directory form.\n",
    )
    .unwrap();
    std::fs::write(
        project.path().join(".cursorrules"),
        "# no-any\n\nStale body from the legacy file.\n",
    )
    .unwrap();

    let report = reconcile_items(
        &Cursor,
        &store,
        ItemKind::Rule,
        Scope::Project,
        project.path(),
    )
    .unwrap();
    assert_eq!(report.pulled, vec!["no-any".to_string()]);
    let pulled = store.load_item(ItemKind::Rule, "no-any").unwrap();
    assert_eq!(
        pulled.body.trim(),
        "Current body from the directory form.",
        "the legacy `.cursorrules` must not overwrite the item reconciled from `.mdc`"
    );
    assert_eq!(pulled.frontmatter.applies_to, vec!["**/*.ts".to_string()]);
    assert!(
        project.path().join(".cursorrules").exists(),
        "the user's legacy file is ignored, never deleted"
    );

    // ---- 2 + 3. A grouped skill, named by its directory, not its frontmatter ----
    let nested = project
        .path()
        .join(".claude")
        .join("skills")
        .join("group")
        .join("weather-lookup");
    std::fs::create_dir_all(nested.join("reference")).unwrap();
    std::fs::write(
        nested.join("SKILL.md"),
        "---\nname: totally-different\ndescription: Look up current weather\n---\n\nUse the weather API.\n",
    )
    .unwrap();
    // Supporting doc inside the skill: payload, not an item of its own.
    std::fs::write(
        nested.join("reference").join("api.md"),
        "---\nname: api\ndescription: notes\n---\n\nEndpoint list.\n",
    )
    .unwrap();

    let report = reconcile_items(
        &ClaudeCode,
        &store,
        ItemKind::Skill,
        Scope::Project,
        project.path(),
    )
    .unwrap();
    assert_eq!(
        report.pulled,
        vec!["weather-lookup".to_string()],
        "a skill nested one level deeper must be discovered, named after its own directory"
    );
    let skill = store.load_item(ItemKind::Skill, "weather-lookup").unwrap();
    assert_eq!(skill.frontmatter.description, "Look up current weather");
    assert_eq!(skill.body.trim(), "Use the weather API.");
    assert!(
        store
            .load_item(ItemKind::Skill, "totally-different")
            .is_err(),
        "the frontmatter `name` must not win over the directory the file lives in"
    );
    assert!(
        store.load_item(ItemKind::Skill, "api").is_err(),
        "a skill's supporting doc must not become a phantom item"
    );
}

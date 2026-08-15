use shaic_core::adapters::claude_code::ClaudeCode;
use shaic_core::materialize::reconcile_items;
use shaic_core::model::{ItemKind, Scope};
use shaic_core::store::Store;

/// Regression coverage for a silent-failure bug found via review: a store
/// item that already exists but is unreadable (corrupt frontmatter) must
/// not be treated the same as "doesn't exist yet". Confusing the two would
/// silently overwrite the corrupt file with a copy re-scoped to *only* the
/// scope currently being reconciled, discarding whatever other scopes the
/// unreadable file actually covered — with no report of anything having
/// gone wrong.
///
/// Sets `HOME` for the process (`Store::state_dir()`/`ClaudeCode::root` for
/// `Scope::Global` both live under it).
#[test]
fn reconcile_refuses_to_silently_overwrite_a_corrupt_store_item() {
    let home = tempfile::tempdir().unwrap();
    unsafe {
        std::env::set_var("HOME", home.path());
    }
    let store = Store::init(home.path().join("store"), None).unwrap();
    let project = tempfile::tempdir().unwrap();

    // A store rule file that exists but fails to parse (no frontmatter
    // block at all) — simulating on-disk corruption rather than a missing
    // file.
    let rule_path = store.root().join("rules").join("no-any.md");
    std::fs::create_dir_all(rule_path.parent().unwrap()).unwrap();
    std::fs::write(&rule_path, "this is not valid frontmatter content\n").unwrap();
    let corrupt_contents_before = std::fs::read_to_string(&rule_path).unwrap();

    // Hand-add the same-named rule directly in Claude Code's own file,
    // bypassing `shaic item add` entirely.
    let claude_dir = project.path().join(".claude");
    std::fs::create_dir_all(&claude_dir).unwrap();
    std::fs::write(
        claude_dir.join("CLAUDE.md"),
        "## no-any\n\nNever use `any` in TypeScript.\n",
    )
    .unwrap();

    let claude = ClaudeCode;
    let report = reconcile_items(
        &claude,
        &store,
        ItemKind::Rule,
        Scope::Project,
        project.path(),
    )
    .unwrap();

    assert!(
        report.pulled.is_empty(),
        "must not report a pull for a name whose store copy is corrupt: {:?}",
        report.pulled
    );
    assert_eq!(report.rejected.len(), 1);
    assert_eq!(report.rejected[0].0, "no-any");

    let corrupt_contents_after = std::fs::read_to_string(&rule_path).unwrap();
    assert_eq!(
        corrupt_contents_before, corrupt_contents_after,
        "the corrupt store file must be left untouched, not silently overwritten"
    );
}

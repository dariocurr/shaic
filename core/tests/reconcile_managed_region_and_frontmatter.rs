use shaic_core::adapters::claude_code::ClaudeCode;
use shaic_core::materialize::{apply, plan_materialize, reconcile_items};
use shaic_core::model::{AgentId, Frontmatter, Item, ItemKind, Scope};
use shaic_core::store::Store;

/// Regression coverage for a critical bug found by review: reconciling a
/// single-file combined agent (`CLAUDE.md`, `AGENTS.md`, `GEMINI.md`,
/// `copilot-instructions.md`) used to read the *whole file*, not just the
/// shaic-managed block between markers — so hand-written notes left below
/// the block got pulled into the last item's body and re-emitted inside the
/// block on every subsequent sync, growing without bound. Separately, the
/// heading-only reconciled frontmatter (which can never carry
/// description/tags/applies_to) used to blindly overwrite whatever the
/// store already had for those fields instead of preserving them.
///
/// Sets `HOME` for the process (provenance manifests live under it).
#[test]
fn reconcile_ignores_hand_written_notes_outside_the_managed_block_and_keeps_description() {
    let home = tempfile::tempdir().unwrap();
    unsafe {
        std::env::set_var("HOME", home.path());
    }
    let store = Store::init(home.path().join("store"), None).unwrap();
    let project = tempfile::tempdir().unwrap();

    store
        .save_item(
            &Item::new(
                ItemKind::Rule,
                Frontmatter {
                    name: "no-any".to_string(),
                    description: "Never use `any` in TypeScript.".to_string(),
                    applies_to: Vec::new(),
                    tags: vec!["typescript".to_string()],
                    scope: vec![Scope::Project],
                    agents: AgentId::ALL.to_vec(),
                },
                "Body.".to_string(),
            )
            .unwrap(),
        )
        .unwrap();

    let claude = ClaudeCode;
    let claude_md = project.path().join(".claude").join("CLAUDE.md");

    let mut previous_len = 0usize;
    for round in 1..=3 {
        let plan = plan_materialize(&claude, &store, Scope::Project, project.path()).unwrap();
        apply(&claude, &plan, Scope::Project, project.path()).unwrap();

        if round == 1 {
            // Hand-written notes below the managed block, exactly the shape
            // that used to get devoured on the next reconcile.
            let mut contents = std::fs::read_to_string(&claude_md).unwrap();
            contents.push_str("\n\nSome hand-written notes I added myself.\n");
            std::fs::write(&claude_md, &contents).unwrap();
        }

        let report = reconcile_items(
            &claude,
            &store,
            ItemKind::Rule,
            Scope::Project,
            project.path(),
        )
        .unwrap();
        assert!(
            report.pulled.is_empty() || round == 1,
            "round {round}: nothing hand-edited inside the item itself, should have nothing to pull: {:?}",
            report.pulled
        );

        let stored = store.load_item(ItemKind::Rule, "no-any").unwrap();
        assert_eq!(
            stored.body, "Body.",
            "round {round}: body must not have absorbed the hand-written notes"
        );
        assert_eq!(
            stored.frontmatter.description, "Never use `any` in TypeScript.",
            "round {round}: description must survive reconcile, not get wiped to empty"
        );
        assert_eq!(
            stored.frontmatter.tags,
            vec!["typescript".to_string()],
            "round {round}: tags must survive reconcile too"
        );

        let len = std::fs::read_to_string(&claude_md).unwrap().len();
        if round > 1 {
            assert_eq!(
                len, previous_len,
                "round {round}: CLAUDE.md must not grow round over round"
            );
        }
        previous_len = len;
    }
}

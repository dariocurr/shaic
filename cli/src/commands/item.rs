use shaic_core::materialize;
use shaic_core::model::{AgentId, ItemKind};
use shaic_core::store;

use crate::ItemAction;
use crate::error::{CliError, Result, bail};

use super::{current_project_root, open_store};

pub fn run(action: ItemAction) -> Result<()> {
    let store = open_store()?;
    match action {
        ItemAction::Add { name, kind } => {
            shaic_core::model::validate_name(&name)?;
            if store.load_item(kind, &name).is_ok() {
                bail!("{kind:?} {name:?} already exists — use `shaic item edit` instead");
            }
            let raw = shaic_core::editor::edit_in_editor(&store::item_template(&name))?;
            let item = store::parse_item(kind, &raw)?;
            if item.name() != name {
                bail!(
                    "frontmatter name {:?} must match the CLI name {name:?} — renaming via add is not supported",
                    item.name()
                );
            }
            store.save_item(&item)?;
            println!("added {kind:?} {name:?}");
            Ok(())
        }
        ItemAction::Edit { name, kind } => {
            let existing = store.load_item(kind, &name)?;
            let raw = shaic_core::editor::edit_in_editor(&store::render_for_edit(&existing))?;
            let item = store::parse_item(kind, &raw)?;
            if item.name() != name {
                bail!(
                    "renaming via edit is not supported — keep name {name:?} (got {:?}), or `shaic item rm` then add",
                    item.name()
                );
            }
            store.save_item(&item)?;
            println!("updated {kind:?} {name:?}");
            Ok(())
        }
        ItemAction::Rm { name, kind } => {
            store.remove_item(kind, &name)?;
            println!("removed {kind:?} {name:?}");
            let project_root = current_project_root()?;
            let (applied, notes) = materialize::push_all_now(&store, &project_root);
            for note in &notes {
                println!("[skip] {note}");
            }
            println!("materialized the deletion to {applied} agent/scope(s)");
            if !notes.is_empty() {
                return Err(CliError::Message(format!(
                    "deletion materialized to {applied} agent/scope(s), but {} failed",
                    notes.len()
                )));
            }
            Ok(())
        }
        ItemAction::List { kind } => {
            let kinds = kind.map_or_else(|| ItemKind::ALL.to_vec(), |k| vec![k]);
            for k in kinds {
                let (items, skipped) = store.list_items_with_skips(k)?;
                for item in items {
                    let agents = if item.frontmatter.agents.len() < AgentId::ALL.len() {
                        format!(
                            "\tagents=[{}]",
                            item.frontmatter
                                .agents
                                .iter()
                                .map(AgentId::as_str)
                                .collect::<Vec<_>>()
                                .join(",")
                        )
                    } else {
                        String::new()
                    };
                    println!(
                        "{k:?}\t{}\t{}{agents}",
                        item.name(),
                        item.frontmatter.description
                    );
                }
                for (_, message) in skipped {
                    println!("[skip] {message}");
                }
            }
            Ok(())
        }
    }
}

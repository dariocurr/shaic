use shaic_core::materialize;
use shaic_core::model::{AgentId, ItemKind};
use shaic_core::store;

use crate::ItemAction;
use crate::error::{Result, bail};

use super::{current_project_root, open_store};

pub fn run(action: ItemAction) -> Result<()> {
    let store = open_store()?;
    match action {
        ItemAction::Add { name, kind } => {
            if store.load_item(kind, &name).is_ok() {
                bail!("{kind:?} {name:?} already exists — use `shaic item edit` instead");
            }
            let raw = shaic_core::editor::edit_in_editor(&store::item_template(&name))?;
            let item = store::parse_item(kind, &raw)?;
            store.save_item(&item)?;
            println!("added {kind:?} {name:?}");
            Ok(())
        }
        ItemAction::Edit { name, kind } => {
            let existing = store.load_item(kind, &name)?;
            let raw = shaic_core::editor::edit_in_editor(&store::render_for_edit(&existing))?;
            let item = store::parse_item(kind, &raw)?;
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
            println!("pushed the deletion to {applied} agent/scope(s)");
            Ok(())
        }
        ItemAction::List { kind } => {
            let kinds = kind.map_or_else(|| ItemKind::ALL.to_vec(), |k| vec![k]);
            for k in kinds {
                for item in store.list_items(k)? {
                    // Only called out when restricted — an item targeting
                    // every agent (the common case) stays silent here, same
                    // reasoning as `mcp_template`'s omit-if-default fields.
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
            }
            Ok(())
        }
    }
}

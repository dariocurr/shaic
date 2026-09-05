use std::path::{Path, PathBuf};

use crate::model::ItemKind;

pub fn kind_dir(kind: ItemKind) -> &'static str {
    match kind {
        ItemKind::Skill => "skills",
        ItemKind::Rule => "rules",
        ItemKind::Command => "commands",
        ItemKind::Subagent => "subagents",
    }
}

/// Skills get a directory-per-item (`skills/<name>/SKILL.md`, matching Claude
/// Code's own convention 1:1); rules, commands, and subagents are lighter,
/// file-per-item.
pub fn item_path(store_root: &Path, kind: ItemKind, name: &str) -> PathBuf {
    match kind {
        ItemKind::Skill => store_root.join("skills").join(name).join("SKILL.md"),
        ItemKind::Rule => store_root.join("rules").join(format!("{name}.md")),
        ItemKind::Command => store_root.join("commands").join(format!("{name}.md")),
        ItemKind::Subagent => store_root.join("subagents").join(format!("{name}.md")),
    }
}

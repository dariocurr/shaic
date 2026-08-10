use std::path::{Path, PathBuf};

use crate::adapters::{Agent, DiscoveredContent};
use crate::error::{Error, Result};
use crate::model::{ContentForm, ItemKind, Scope};
use crate::store::Store;

use super::mcp::ReconcileReport;
use super::writer::{self, Manifest, WriteAction};

#[derive(Debug, Clone)]
pub struct PlannedWrite {
    pub relative_path: PathBuf,
    pub action: WriteAction,
    pub contents: String,
    pub form: ContentForm,
}

#[derive(Debug, Clone)]
pub struct PlannedDelete {
    pub relative_path: PathBuf,
}

#[derive(Debug, Default)]
pub struct MaterializePlan {
    pub writes: Vec<PlannedWrite>,
    pub deletes: Vec<PlannedDelete>,
    pub skipped: Vec<String>,
}

impl MaterializePlan {
    pub fn is_empty(&self) -> bool {
        self.writes.iter().all(|w| w.action == WriteAction::NoOp) && self.deletes.is_empty()
    }

    /// Writes that would actually change something on disk.
    pub fn changed_writes(&self) -> impl Iterator<Item = &PlannedWrite> {
        self.writes.iter().filter(|w| w.action != WriteAction::NoOp)
    }
}

/// Compute what would change for one agent+scope, without writing anything.
/// This is what `sync --dry-run` and the Diff Preview TUI screen both render.
pub fn plan_materialize(
    agent: &dyn Agent,
    store: &Store,
    scope: Scope,
    project_root: &Path,
) -> Result<MaterializePlan> {
    let mut plan = MaterializePlan::default();

    if !agent.supported_scopes().contains(&scope) {
        plan.skipped.push(format!(
            "{} does not support {scope:?} scope",
            agent.display_name()
        ));
        return Ok(plan);
    }
    if agent.experimental_read_only() {
        plan.skipped.push(format!(
            "{} is experimental/read-only — not materialized in v1",
            agent.display_name()
        ));
        return Ok(plan);
    }

    let root = agent.root(scope, project_root);
    let manifest_path = Manifest::path_for(agent.id(), scope);
    let manifest = Manifest::load(&manifest_path);
    let mut rendered_dir_paths: Vec<String> = Vec::new();

    for &kind in agent.supported_kinds() {
        let items: Vec<_> = store
            .list_items(kind)?
            .into_iter()
            .filter(|i| {
                i.frontmatter.scope.contains(&scope) && i.frontmatter.agents.contains(&agent.id())
            })
            .collect();
        if items.is_empty() {
            continue;
        }

        let discovered = agent.discover_existing(kind, scope, project_root);
        let existing_form = determine_existing_form(&discovered);
        let rendered = agent.render(kind, &items, scope, existing_form);

        for file in rendered {
            let target = root.join(&file.relative_path);
            let action = writer::classify(&target, file.form, &file.contents);
            if file.form == ContentForm::Directory {
                rendered_dir_paths.push(file.relative_path.to_string_lossy().into_owned());
            }
            plan.writes.push(PlannedWrite {
                relative_path: file.relative_path,
                action,
                contents: file.contents,
                form: file.form,
            });
        }
    }

    // Safe-delete candidates: manifest-tracked directory-form paths that are
    // no longer part of this round's render output, and whose on-disk content
    // still matches shaic's last recorded write (untouched since).
    for tracked in manifest.tracked_paths() {
        if rendered_dir_paths.iter().any(|p| p == tracked) {
            continue;
        }
        let full = root.join(tracked);
        match std::fs::read_to_string(&full) {
            Ok(on_disk) => {
                if manifest.safe_to_delete(tracked, &on_disk) {
                    plan.deletes.push(PlannedDelete {
                        relative_path: PathBuf::from(tracked),
                    });
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => plan
                .skipped
                .push(format!("could not check {tracked} for delete: {e}")),
        }
    }

    Ok(plan)
}

fn determine_existing_form(discovered: &[DiscoveredContent]) -> Option<ContentForm> {
    let has_dir = discovered.iter().any(|d| d.form == ContentForm::Directory);
    let has_single = discovered.iter().any(|d| d.form == ContentForm::SingleFile);
    match (has_dir, has_single) {
        (true, _) => Some(ContentForm::Directory),
        (false, true) => Some(ContentForm::SingleFile),
        (false, false) => None,
    }
}

/// Pull an agent's on-disk items for `kind`+`scope` back into the canonical
/// store — the same idea as `reconcile_mcp`, via `Agent::reconcile_existing`
/// (the inverse of `render`, which most agents can only support for some
/// kinds; see each adapter for exactly which). New or changed items are
/// saved; an item whose parsed content already exactly matches the store's
/// copy is left untouched. Callers must only invoke this from a real,
/// confirmed apply — never a `--dry-run`/Diff Preview path — since it
/// writes into the store immediately rather than returning a plan to review
/// first.
pub fn reconcile_items(
    agent: &dyn Agent,
    store: &Store,
    kind: ItemKind,
    scope: Scope,
    project_root: &Path,
) -> Result<ReconcileReport> {
    let mut report = ReconcileReport::default();
    for mut candidate in agent.reconcile_existing(kind, scope, project_root) {
        let name = candidate.name().to_string();
        // Distinguish "no store copy yet" from "store copy exists but is
        // unreadable" (corrupt frontmatter, etc.) — treating the latter as
        // the former would silently narrow the item's scope to just this
        // one on save, discarding whatever scopes the (unreadable) existing
        // copy actually covered.
        let existing = match store.load_item(kind, &name) {
            Ok(item) => Some(item),
            Err(Error::Io { source, .. }) if source.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => {
                report.rejected.push((
                    name,
                    format!("existing store copy unreadable, refusing to reconcile: {e}"),
                ));
                continue;
            }
        };
        if let Some(existing) = &existing {
            // Keep every scope the store already had this item materializing
            // into, plus this one — so pulling a change made via one agent
            // doesn't quietly drop the item from scopes it already covered.
            let mut scopes = existing.frontmatter.scope.clone();
            if !scopes.contains(&scope) {
                scopes.push(scope);
            }
            candidate.frontmatter.scope = scopes;
            // Most on-disk formats can't carry description/tags/applies_to
            // at all (a heading-only Rule section, for instance) — a
            // `reconcile_existing` impl that can't recover a field always
            // leaves it empty, never a deliberate "clear this" signal, so an
            // empty field here means "inherit whatever the store already
            // had" rather than "wipe it".
            if candidate.frontmatter.description.is_empty() {
                candidate.frontmatter.description = existing.frontmatter.description.clone();
            }
            if candidate.frontmatter.tags.is_empty() {
                candidate.frontmatter.tags = existing.frontmatter.tags.clone();
            }
            if candidate.frontmatter.applies_to.is_empty() {
                candidate.frontmatter.applies_to = existing.frontmatter.applies_to.clone();
            }
        }
        if existing.as_ref() == Some(&candidate) {
            continue;
        }
        match store.save_item(&candidate) {
            Ok(()) => report.pulled.push(name),
            Err(e) => report.rejected.push((name, e.to_string())),
        }
    }
    Ok(report)
}

/// Execute a previously computed plan: perform each non-no-op write, apply
/// safe deletes, and update the provenance manifest to match.
pub fn apply(
    agent: &dyn Agent,
    plan: &MaterializePlan,
    scope: Scope,
    project_root: &Path,
) -> Result<Vec<PlannedWrite>> {
    let root = agent.root(scope, project_root);
    let manifest_path = Manifest::path_for(agent.id(), scope);
    let mut manifest = Manifest::load(&manifest_path);
    let mut applied = Vec::new();

    for write in &plan.writes {
        if write.action == WriteAction::NoOp {
            continue;
        }
        let (_target, action) =
            writer::write_item(&root, &write.relative_path, write.form, &write.contents)?;
        if write.form == ContentForm::Directory {
            manifest.record(&write.relative_path.to_string_lossy(), &write.contents);
        }
        applied.push(PlannedWrite {
            action,
            ..write.clone()
        });
    }

    for delete in &plan.deletes {
        let full = root.join(&delete.relative_path);
        match std::fs::remove_file(&full) {
            Ok(()) => manifest.forget(&delete.relative_path.to_string_lossy()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                manifest.forget(&delete.relative_path.to_string_lossy());
            }
            Err(e) => eprintln!(
                "warning: could not delete {} ({e}); will retry next sync",
                full.display()
            ),
        }
    }

    manifest.save(&manifest_path)?;
    Ok(applied)
}

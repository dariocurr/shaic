use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::adapters::common::canonical_item_name;
use crate::adapters::{Agent, DiscoveredContent};
use crate::error::{Error, Result};
use crate::model::{AgentId, ContentForm, Item, ItemKind, Scope};
use crate::security::path_guard;
use crate::store::Store;

use super::mcp::ReconcileReport;
use super::writer::{self, Manifest, TrackedContent, WriteAction};

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
    /// Things that went wrong but didn't stop the plan — a store item that
    /// wouldn't parse, two items fighting over one path, a delete that had to
    /// be abandoned.
    ///
    /// Returned as data rather than printed. This is a library: the TUI runs
    /// on an alternate screen that a stray `eprintln!` corrupts mid-frame,
    /// and a print is impossible to assert on in a test. Plain `String`s
    /// rather than a typed enum to match the `skipped` field they sit beside
    /// — the two are the same kind of thing (a human-readable note about one
    /// agent/scope pass) and every consumer does the same thing with both,
    /// which is print them.
    pub warnings: Vec<String>,
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

/// What `apply` actually did, plus anything it had to give up on.
#[derive(Debug, Default)]
pub struct ApplyReport {
    pub writes: Vec<PlannedWrite>,
    /// Same contract as `MaterializePlan::warnings`: non-fatal problems
    /// (a delete refused by the path guard, a file edited between planning
    /// and applying) surfaced as data for the caller to display.
    pub warnings: Vec<String>,
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

    // No resolvable root (no home directory for a global scope) means skip,
    // never guess a path — writing an agent's config into whatever directory
    // the user happens to be in is worse than doing nothing.
    let Some(root) = agent.root(scope, project_root) else {
        plan.skipped.push(format!(
            "{} has no resolvable {scope:?} root on this machine (no home directory) — skipped",
            agent.display_name()
        ));
        return Ok(plan);
    };
    let manifest_path = Manifest::path_for(agent.id(), scope);
    let manifest = Manifest::load(&manifest_path);
    let mut rendered_paths: Vec<String> = Vec::new();
    // Which (kind, item) first claimed each relative path this round. Two
    // kinds can render into the same directory with the same file-naming
    // rule — Cursor/Windsurf/Cline treat Skill and Rule as one on-disk shape
    // — so a Skill and a Rule both named `deploy` produce the same path, and
    // whichever kind ran last used to silently overwrite the other (with the
    // manifest then recording only the survivor, so the loser's content was
    // gone with no trace and no message).
    let mut claimed_paths: BTreeMap<String, String> = BTreeMap::new();

    for &kind in agent.supported_kinds() {
        // Cursor / Windsurf / Cline collapse Skill+Rule into one SingleFile
        // path (`.cursorrules` etc.). Rendering each kind separately would
        // make the last kind wipe the earlier one — fold Skills into the
        // Rule pass when that form is live.
        let discovered = agent.discover_existing(kind, scope, project_root);
        let existing_form = determine_existing_form(&discovered);
        if kind == ItemKind::Skill
            && collapses_skill_into_rule(agent)
            && existing_form == Some(ContentForm::SingleFile)
        {
            continue;
        }

        let mut items = items_for(store, kind, agent, scope, &mut plan.warnings)?;
        if kind == ItemKind::Rule
            && collapses_skill_into_rule(agent)
            && existing_form == Some(ContentForm::SingleFile)
        {
            items.extend(items_for(
                store,
                ItemKind::Skill,
                agent,
                scope,
                &mut plan.warnings,
            )?);
        }
        // Directory empties are handled by the delete pass below. A
        // SingleFile managed region lives inside a file shaic must never
        // unlink, so an empty item set still has to splice an empty region
        // or stale content stays there forever.
        let rendered = if items.is_empty() {
            if existing_form != Some(ContentForm::SingleFile) {
                continue;
            }
            agent.render(kind, &[], scope, existing_form)
        } else {
            agent.render(kind, &items, scope, existing_form)
        };

        for file in rendered {
            let path_key = file.relative_path.to_string_lossy().into_owned();
            let claim = claim_label(kind, &file.relative_path);
            if let Some(first) = claimed_paths.get(&path_key) {
                // Keep the first claim, deterministically: `supported_kinds()`
                // is a fixed-order constant, so which one survives doesn't
                // depend on filesystem or hash iteration order. The user
                // resolves it by renaming one of the two.
                plan.warnings.push(format!(
                    "{path_key} is rendered by both {first} and {claim} — keeping {first}; \
                     rename one of them so both can materialize"
                ));
                continue;
            }
            claimed_paths.insert(path_key.clone(), claim);

            let target = root.join(&file.relative_path);
            let action = writer::classify(&target, file.form, &file.contents);
            rendered_paths.push(path_key);
            plan.writes.push(PlannedWrite {
                relative_path: file.relative_path,
                action,
                contents: file.contents,
                form: file.form,
            });
        }
    }

    // Safe-delete candidates: manifest-tracked paths that are no longer part
    // of this round's render output, and whose on-disk content still matches
    // shaic's last recorded write (untouched since). Managed-region entries
    // can never qualify — see `Manifest::safe_to_delete` — so a legacy
    // single-file target that dropped out of the render output is read once
    // and then left alone.
    for tracked in manifest.tracked_paths() {
        if rendered_paths.iter().any(|p| p == tracked) {
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

/// The store's items of `kind` that target this agent+scope, plus a warning
/// per file that couldn't be read.
///
/// Uses `list_items_with_skips` rather than `list_items` so a single corrupt
/// item file is *reported* instead of quietly vanishing from the plan: an
/// item silently missing from a render pass is indistinguishable from one the
/// user deleted on purpose, right up until its agent file gets cleaned up.
fn items_for(
    store: &Store,
    kind: ItemKind,
    agent: &dyn Agent,
    scope: Scope,
    warnings: &mut Vec<String>,
) -> Result<Vec<Item>> {
    let (items, skipped) = store.list_items_with_skips(kind)?;
    warnings.extend(
        skipped
            .into_iter()
            .map(|(_, message)| format!("could not read {message}")),
    );
    Ok(items
        .into_iter()
        .filter(|i| {
            i.frontmatter.scope.contains(&scope) && i.frontmatter.agents.contains(&agent.id())
        })
        .collect())
}

/// How a rendered path's owner is named in a collision warning. The item name
/// is recovered from the path the same way every reverse direction in the
/// crate does it (`canonical_item_name`), since `RenderedFile` doesn't carry
/// one; a path that isn't item-shaped (a combined `.cursorrules`) is
/// identified by kind alone.
fn claim_label(kind: ItemKind, relative_path: &Path) -> String {
    match canonical_item_name(relative_path) {
        Some(name) => format!("{kind:?} {name:?}"),
        None => format!("{kind:?}"),
    }
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

/// Cursor / Windsurf / Cline treat Skill and Rule as the same on-disk shape
/// for SingleFile legacy paths — rendering them in separate kind passes
/// would make the second wipe the first.
fn collapses_skill_into_rule(agent: &dyn Agent) -> bool {
    matches!(
        agent.id(),
        AgentId::Cursor | AgentId::Windsurf | AgentId::Cline
    )
}

/// Pull an agent's on-disk items for `kind`+`scope` back into the canonical
/// store — the same idea as `reconcile_mcp`, via `Agent::reconcile_existing`
/// (the inverse of `render`, which most agents can only support for some
/// kinds; see each adapter for exactly which). New or changed items are
/// saved; an item whose parsed content already exactly matches the store's
/// copy is left untouched. Callers must only invoke this from `shaic import`
/// / TUI import — never from `sync` or a `--dry-run`/Diff Preview path —
/// since it writes into the store immediately rather than returning a plan
/// to review first.
pub fn reconcile_items(
    agent: &dyn Agent,
    store: &Store,
    kind: ItemKind,
    scope: Scope,
    project_root: &Path,
) -> Result<ReconcileReport> {
    let mut report = ReconcileReport::default();
    // `discover_unowned` already yields nothing without a root, so this used
    // to be a silent no-op — the user saw a clean reconcile for an agent that
    // was never even looked at. Say so instead.
    if agent.root(scope, project_root).is_none() {
        report.warnings.push(format!(
            "{} has no resolvable {scope:?} root on this machine (no home directory) — \
             nothing was pulled from it",
            agent.display_name()
        ));
        return Ok(report);
    }
    for mut candidate in agent.reconcile_existing(kind, scope, project_root) {
        let name = candidate.name().to_string();
        // Distinguish "no store copy yet" from "store copy exists but is
        // unreadable" (corrupt frontmatter, etc.) — treating the latter as
        // the former would silently narrow the item's scope to just this
        // one on save, discarding whatever scopes the (unreadable) existing
        // copy actually covered.
        let existing = match store.load_item_with_warnings(kind, &name) {
            Ok((item, parse_warnings)) => {
                report.warnings.extend(parse_warnings);
                Some(item)
            }
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
            // Same as MCP reconcile: on-disk formats can't express per-agent
            // targeting, so an empty/default `agents` from parse must not
            // expand a restricted item to every agent.
            candidate.frontmatter.agents = existing.frontmatter.agents.clone();
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
) -> Result<ApplyReport> {
    // Unreachable in practice: a plan for an agent+scope with no resolvable
    // root is always empty, and `plan_materialize` already recorded the skip
    // note the user sees. Nothing to apply, and nowhere safe to apply it.
    let Some(root) = agent.root(scope, project_root) else {
        return Ok(ApplyReport::default());
    };
    let manifest_path = Manifest::path_for(agent.id(), scope);
    let mut manifest = Manifest::load(&manifest_path);
    let mut report = ApplyReport::default();

    let write_error = run_writes(&root, plan, &mut manifest, &mut report);
    // Deletes are computed from a complete render; a pass that couldn't
    // finish writing hasn't earned the right to remove anything.
    if write_error.is_none() {
        run_deletes(&root, plan, &mut manifest, &mut report);
    }

    // Save the manifest even when a write failed. Propagating the error
    // straight out used to skip this entirely, so every file the pass *had*
    // already written stayed unrecorded — permanently untracked, and
    // therefore never cleanable by any later sync. Recording what actually
    // landed is correct regardless of how the pass ended.
    let saved = manifest.save(&manifest_path);
    match write_error {
        Some(e) => {
            if let Err(save_error) = saved {
                report
                    .warnings
                    .push(format!("could not record what was written: {save_error}"));
            }
            Err(e)
        }
        None => {
            saved?;
            Ok(report)
        }
    }
}

/// Perform the plan's writes, recording each one in `manifest`. Returns the
/// first error rather than propagating it, so the caller can still persist
/// the manifest for everything written before it.
fn run_writes(
    root: &Path,
    plan: &MaterializePlan,
    manifest: &mut Manifest,
    report: &mut ApplyReport,
) -> Option<Error> {
    for write in &plan.writes {
        if write.action == WriteAction::NoOp {
            continue;
        }
        let action =
            match writer::write_item(root, &write.relative_path, write.form, &write.contents) {
                Ok((_target, action)) => action,
                Err(e) => return Some(e),
            };
        // Track single-file writes too, not just directory ones. Without
        // this, `common::is_still_shaic_owned` could never recognize shaic's
        // own single-file output, so the next reconcile read a legacy
        // `.cursorrules`/`.windsurfrules`/`.clinerules` back in as brand-new
        // *Rule* content — including the Skill items folded into that same
        // render pass — creating a phantom duplicate item that then
        // double-wrote the file forever. The tracked value is the managed
        // *region*, never the whole file, because everything outside the
        // markers is the user's to edit.
        let tracked = TrackedContent::for_form(write.form);
        let recorded = match write.form {
            ContentForm::Directory => write.contents.as_str(),
            ContentForm::SingleFile => write.contents.trim(),
        };
        manifest.record(&write.relative_path.to_string_lossy(), tracked, recorded);
        report.writes.push(PlannedWrite {
            action,
            ..write.clone()
        });
    }
    None
}

fn run_deletes(
    root: &Path,
    plan: &MaterializePlan,
    manifest: &mut Manifest,
    report: &mut ApplyReport,
) {
    for delete in &plan.deletes {
        let full = root.join(&delete.relative_path);
        let guarded = match path_guard::ensure_within(root, &full) {
            Ok(p) => p,
            Err(e) => {
                report.warnings.push(format!(
                    "refusing to delete {} ({e}); will retry next sync",
                    full.display()
                ));
                continue;
            }
        };
        if let Err(e) = path_guard::reject_if_symlink(&guarded) {
            report.warnings.push(format!(
                "refusing to delete {} ({e}); will retry next sync",
                guarded.display()
            ));
            continue;
        }
        // Re-check ownership against what's on disk *now*, not against what
        // planning saw. A plan is shown to the user and waits for a
        // confirmation; in that window the file can be edited, and deleting
        // it then destroys an edit shaic explicitly promised never to touch.
        // The read is cheap next to the consequence of skipping it.
        let tracked_key = delete.relative_path.to_string_lossy().into_owned();
        match std::fs::read_to_string(&guarded) {
            Ok(on_disk) if !manifest.safe_to_delete(&tracked_key, &on_disk) => {
                report.warnings.push(format!(
                    "{} changed since this was planned — leaving it alone",
                    guarded.display()
                ));
                continue;
            }
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
                report.warnings.push(format!(
                    "could not re-check {} before deleting it ({e}); will retry next sync",
                    guarded.display()
                ));
                continue;
            }
            // Already gone: nothing to remove, but the manifest entry should
            // still go, which the `remove_file` arm below handles.
            _ => {}
        }
        match std::fs::remove_file(&guarded) {
            Ok(()) => manifest.forget(&tracked_key),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => manifest.forget(&tracked_key),
            Err(e) => report.warnings.push(format!(
                "could not delete {} ({e}); will retry next sync",
                guarded.display()
            )),
        }
    }
}

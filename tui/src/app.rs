use std::path::PathBuf;

use shaic_core::adapters;
use shaic_core::config::Config;
use shaic_core::materialize::{self, MaterializePlan, McpPlan, WriteAction};
use shaic_core::model::{AgentId, ItemKind, Scope};
use shaic_core::store::Store;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Screen {
    SetupWizard,
    Dashboard,
    ItemBrowser,
    DiffPreview,
    AgentDetail,
}

/// One agent's sync state for a single scope, covering both content
/// (skills/rules/commands) and MCP servers in that scope — `None` for
/// whichever axis this agent/scope combination doesn't support. Kept as one
/// row per scope rather than one per (scope, axis): from the user's point of
/// view "this agent, this scope" is one thing, not two.
pub struct AgentSubRow {
    pub scope: Scope,
    pub content_glyph: Option<&'static str>,
    pub mcp_glyph: Option<&'static str>,
}

pub struct AgentRow {
    pub id: AgentId,
    pub name: String,
    /// Worst-of across every `sub_rows` entry — see `worst_glyph`.
    pub glyph: &'static str,
    pub sub_rows: Vec<AgentSubRow>,
}

/// Precedence for combining several sub-row statuses into one glyph for the
/// agent as a whole: a single scope/content-axis problem should be visible
/// at the top level even if every other axis is fine.
fn worst_glyph(glyphs: impl Iterator<Item = &'static str>) -> &'static str {
    fn rank(glyph: &str) -> u8 {
        match glyph {
            "error" => 0,
            "drift" => 1,
            "unconfirmed" => 2,
            _ => 3, // "in-sync"
        }
    }
    glyphs.min_by_key(|g| rank(g)).unwrap_or("in-sync")
}

/// The status glyph for one already-computed plan, keyed on whether it came
/// out empty (nothing to write) — shared by the base-content and MCP passes.
fn plan_glyph(in_sync: shaic_core::Result<bool>) -> &'static str {
    match in_sync {
        Ok(true) => "in-sync",
        Ok(false) => "drift",
        Err(_) => "error",
    }
}

/// Plain-language summary of what a not-yet-applied plan would do — replaces
/// the old "materialize plan: N write(s), M delete(s) pending" jargon, which
/// read as an internal term (users asked "what is plan?"). `y`/`m` in Agent
/// Detail applies exactly this: it pushes the store's version to disk.
fn pending_line(writes: usize, removals: usize, removal_word: &str) -> String {
    if writes == 0 && removals == 0 {
        "up to date — nothing to push".to_string()
    } else {
        format!("not yet pushed: {writes} to write, {removals} to {removal_word}")
    }
}

fn warn_suffix(warnings: &[String]) -> String {
    if warnings.is_empty() {
        String::new()
    } else {
        format!(" — {}", warnings.join("; "))
    }
}

/// Move a list selection by `delta`, clamped to `len`. Leaves an empty list's
/// index untouched rather than forcing it to 0.
fn move_index(index: usize, len: usize, delta: i32) -> usize {
    if len == 0 {
        return index;
    }
    (index as i32 + delta).clamp(0, len as i32 - 1) as usize
}

/// Either kind of plan a Diff Preview can show — base materialization
/// (skills/rules/commands) or MCP server sync. Kept as an enum rather than
/// two optional fields so a preview is always unambiguously one or the
/// other, matching the two independent code paths `plan_materialize` and
/// `plan_mcp` already are in `shaic-core`.
pub enum PreviewPlan {
    Base(MaterializePlan),
    Mcp(McpPlan),
}

#[derive(Default)]
pub struct WizardState {
    pub remote_input: String,
    pub status: String,
}

#[derive(Clone)]
pub struct ItemRow {
    pub kind: ItemKind,
    pub name: String,
    pub description: String,
}

pub struct BrowserState {
    pub items: Vec<ItemRow>,
    pub selected: usize,
    pub name_input: Option<String>,
    pub pending_kind: ItemKind,
}

impl Default for BrowserState {
    fn default() -> Self {
        BrowserState {
            items: Vec::new(),
            selected: 0,
            name_input: None,
            pending_kind: ItemKind::Skill,
        }
    }
}

pub struct DiffPreviewState {
    pub agent: AgentId,
    pub scope: Scope,
    pub plan: PreviewPlan,
}

pub struct AgentDetailSubRow {
    pub scope: Scope,
    pub content_glyph: Option<&'static str>,
    pub mcp_glyph: Option<&'static str>,
    pub lines: Vec<String>,
}

pub struct AgentDetailState {
    pub agent: AgentId,
    pub display_name: String,
    pub sub_rows: Vec<AgentDetailSubRow>,
    pub selected_sub_row: usize,
}

/// What the TUI event loop needs to do outside of drawing — currently just
/// "suspend raw mode/alt-screen and hand off to `$EDITOR`". Kept separate
/// from `App` because only the event loop owns the `Terminal`.
pub enum PendingAction {
    None,
    EditItem {
        kind: ItemKind,
        name: String,
        initial: String,
        is_new: bool,
    },
}

#[derive(Clone, Copy)]
pub enum PendingConfirm {
    DeleteItem,
    Push,
    Pull,
    ApplyDiff,
    ImportScope,
}

pub struct App {
    pub screen: Screen,
    pub show_help: bool,
    pub project_root: PathBuf,
    pub agent_rows: Vec<AgentRow>,
    pub selected_agent_row: usize,
    pub message: String,
    pub wizard: WizardState,
    pub browser: BrowserState,
    pub diff: Option<DiffPreviewState>,
    pub detail: Option<AgentDetailState>,
    pub pull_rejections: Vec<(String, String)>,
    pub pending_confirm: Option<PendingConfirm>,
}

impl App {
    pub fn new() -> crate::Result<Self> {
        let project_root = shaic_core::config::infer_project_root()?;
        let has_store = Store::default_path().and_then(Store::open).is_ok();
        let mut app = App {
            screen: if has_store {
                Screen::Dashboard
            } else {
                Screen::SetupWizard
            },
            show_help: false,
            project_root,
            agent_rows: Vec::new(),
            selected_agent_row: 0,
            message: "q=quit".to_string(),
            wizard: WizardState::default(),
            browser: BrowserState::default(),
            diff: None,
            detail: None,
            pull_rejections: Vec::new(),
            pending_confirm: None,
        };
        app.refresh_dashboard();
        Ok(app)
    }

    // ---- Dashboard ----

    pub fn refresh_dashboard(&mut self) {
        let Ok(store) = Store::default_path().and_then(Store::open) else {
            self.agent_rows.clear();
            return;
        };

        self.agent_rows.clear();
        for &agent in adapters::registry() {
            let mut sub_rows = Vec::new();
            // Content and MCP support don't necessarily agree on which
            // scopes they cover (an agent can sync MCP servers in a scope it
            // has no skills/rules/commands support for, or vice versa), so
            // each scope's row is built from the union of both axes rather
            // than assuming they line up.
            for &scope in &[Scope::Global, Scope::Project] {
                let content_glyph = agent.supported_scopes().contains(&scope).then(|| {
                    if agent.experimental_read_only() {
                        "unconfirmed"
                    } else {
                        plan_glyph(
                            materialize::plan_materialize(agent, &store, scope, &self.project_root)
                                .map(|plan| plan.is_empty()),
                        )
                    }
                });
                let mcp_glyph = agent
                    .mcp_target(scope, &self.project_root)
                    .is_some()
                    .then(|| {
                        plan_glyph(
                            materialize::plan_mcp(agent, &store, scope, &self.project_root)
                                .map(|plan| plan.is_empty()),
                        )
                    });
                if content_glyph.is_none() && mcp_glyph.is_none() {
                    continue;
                }
                sub_rows.push(AgentSubRow {
                    scope,
                    content_glyph,
                    mcp_glyph,
                });
            }

            let glyph = worst_glyph(
                sub_rows
                    .iter()
                    .flat_map(|r| r.content_glyph.into_iter().chain(r.mcp_glyph)),
            );
            self.agent_rows.push(AgentRow {
                id: agent.id(),
                name: agent.display_name().to_string(),
                glyph,
                sub_rows,
            });
        }
        if self.selected_agent_row >= self.agent_rows.len() {
            self.selected_agent_row = self.agent_rows.len().saturating_sub(1);
        }
    }

    pub fn move_selection(&mut self, delta: i32) {
        self.selected_agent_row = move_index(self.selected_agent_row, self.agent_rows.len(), delta);
    }

    pub fn move_detail_selection(&mut self, delta: i32) {
        if let Some(detail) = &mut self.detail {
            detail.selected_sub_row =
                move_index(detail.selected_sub_row, detail.sub_rows.len(), delta);
        }
    }

    pub fn request_confirm(&mut self, action: PendingConfirm) {
        let prompt = match action {
            PendingConfirm::DeleteItem => {
                let Some(row) = self.selected_item() else {
                    return;
                };
                format!(
                    "delete {:?} {:?} and materialize to agents? [y/N]",
                    row.kind, row.name
                )
            }
            PendingConfirm::Push => "push store to remote? [y/N]".to_string(),
            PendingConfirm::Pull => "pull store from remote? [y/N]".to_string(),
            PendingConfirm::ApplyDiff => {
                "apply these changes (writes agent files from the store)? [y/N]".to_string()
            }
            PendingConfirm::ImportScope => {
                let Some(detail) = &self.detail else {
                    return;
                };
                let Some(sub) = detail.sub_rows.get(detail.selected_sub_row) else {
                    return;
                };
                format!(
                    "import {:?}/{:?} on-disk files into the store? [y/N]",
                    detail.agent, sub.scope
                )
            }
        };
        self.pending_confirm = Some(action);
        self.message = prompt;
    }

    pub fn cancel_confirm(&mut self) {
        self.pending_confirm = None;
        self.message = "cancelled".to_string();
    }

    pub fn take_confirm(&mut self) -> Option<PendingConfirm> {
        self.pending_confirm.take()
    }

    pub fn push(&mut self) {
        self.message = match Store::default_path()
            .and_then(Store::open)
            .and_then(|s| s.push(false))
        {
            Ok(r) if r.pushed && r.committed => {
                format!("pushed: {}", r.summary.unwrap_or_default())
            }
            Ok(r) if r.pushed => {
                format!(
                    "pushed previously-unpushed commits: {}",
                    r.summary.unwrap_or_default()
                )
            }
            Ok(_) => "nothing to push".to_string(),
            Err(e) => format!("push failed: {e}"),
        };
        self.refresh_dashboard();
    }

    pub fn pull(&mut self) {
        self.message = match Store::default_path()
            .and_then(Store::open)
            .and_then(|s| s.pull(false))
        {
            Ok(r) if r.updated => "pulled changes".to_string(),
            Ok(_) => "already up to date".to_string(),
            Err(e) => format!("pull failed: {e}"),
        };
        self.refresh_dashboard();
    }

    // ---- Setup wizard ----

    pub fn run_wizard(&mut self) {
        let url = self.wizard.remote_input.trim().to_string();
        if url.is_empty() {
            self.wizard.status = "enter a remote url first (or press Esc to skip)".to_string();
            return;
        }
        if let Err(e) = shaic_core::store::git::ls_remote(&url) {
            self.wizard.status = format!(
                "could not reach {}: {e}",
                shaic_core::store::git::redact_userinfo(&url)
            );
            return;
        }
        if let Err(e) = Store::default_path().and_then(|p| Store::init(p, Some(&url))) {
            self.wizard.status = format!("init failed: {e}");
            return;
        }
        self.message = match Config::load() {
            Ok(mut config) => {
                if config.set_remote(&url).is_ok() {
                    let _ = config.save();
                }
                format!(
                    "store ready (remote: {})",
                    shaic_core::store::git::redact_userinfo(&url)
                )
            }
            Err(e) => format!(
                "store ready (remote: {}), but config is corrupted and was left untouched: {e}",
                shaic_core::store::git::redact_userinfo(&url)
            ),
        };
        self.screen = Screen::Dashboard;
        self.refresh_dashboard();
    }

    // ---- Item browser ----

    pub fn open_browser(&mut self) {
        self.refresh_browser();
        self.screen = Screen::ItemBrowser;
    }

    pub fn refresh_browser(&mut self) {
        let mut rows = Vec::new();
        if let Ok(store) = Store::default_path().and_then(Store::open) {
            for kind in ItemKind::ALL {
                if let Ok(items) = store.list_items(kind) {
                    for item in items {
                        rows.push(ItemRow {
                            kind,
                            name: item.name().to_string(),
                            description: item.frontmatter.description.clone(),
                        });
                    }
                }
            }
        }
        if self.browser.selected >= rows.len() {
            self.browser.selected = rows.len().saturating_sub(1);
        }
        self.browser.items = rows;
    }

    pub fn move_browser_selection(&mut self, delta: i32) {
        self.browser.selected = move_index(self.browser.selected, self.browser.items.len(), delta);
    }

    pub fn selected_item(&self) -> Option<&ItemRow> {
        self.browser.items.get(self.browser.selected)
    }

    /// Load the selected item's full content back into editable text, for
    /// handing off to `$EDITOR`.
    pub fn load_selected_for_edit(&mut self) -> Option<PendingAction> {
        let Some(row) = self.selected_item().cloned() else {
            self.message = "no item selected".to_string();
            return None;
        };
        let store = match Store::default_path().and_then(Store::open) {
            Ok(s) => s,
            Err(e) => {
                self.message = format!("could not open store: {e}");
                return None;
            }
        };
        match store.load_item(row.kind, &row.name) {
            Ok(item) => Some(PendingAction::EditItem {
                kind: row.kind,
                name: row.name,
                initial: shaic_core::store::render_for_edit(&item),
                is_new: false,
            }),
            Err(e) => {
                self.message = format!("could not load {:?} {:?}: {e}", row.kind, row.name);
                None
            }
        }
    }

    pub fn begin_add(&mut self) {
        self.browser.name_input = Some(String::new());
        self.browser.pending_kind = ItemKind::Skill;
    }

    pub fn cancel_add(&mut self) {
        self.browser.name_input = None;
    }

    pub fn cycle_pending_kind(&mut self) {
        self.browser.pending_kind = match self.browser.pending_kind {
            ItemKind::Skill => ItemKind::Rule,
            ItemKind::Rule => ItemKind::Command,
            ItemKind::Command => ItemKind::Skill,
        };
    }

    /// Confirms the in-progress "add" name prompt and returns the editor
    /// hand-off action, or `None` if the name was left empty.
    pub fn confirm_add_name(&mut self) -> Option<PendingAction> {
        let name = self.browser.name_input.take()?.trim().to_string();
        if name.is_empty() {
            return None;
        }
        if shaic_core::model::validate_name(&name).is_err() {
            self.message = format!("{name:?} is not a valid item name");
            return None;
        }
        let kind = self.browser.pending_kind;
        Some(PendingAction::EditItem {
            initial: shaic_core::store::item_template(&name),
            kind,
            name,
            is_new: true,
        })
    }

    pub fn remove_selected_item(&mut self) {
        let Some(row) = self.selected_item().cloned() else {
            return;
        };
        let Ok(store) = Store::default_path().and_then(Store::open) else {
            self.message = "no store yet".to_string();
            return;
        };
        match store.remove_item(row.kind, &row.name) {
            Ok(()) => {
                let (applied, notes) = materialize::push_all_now(&store, &self.project_root);
                self.message = if let Some(note) = notes.first() {
                    format!("removed {:?} {:?}, but: {note}", row.kind, row.name)
                } else {
                    format!(
                        "removed {:?} {:?}, pushed to {applied} agent/scope(s)",
                        row.kind, row.name
                    )
                };
            }
            Err(e) => self.message = format!("remove failed: {e}"),
        }
        self.refresh_browser();
    }

    pub fn finish_edit(
        &mut self,
        kind: ItemKind,
        name: String,
        edited: crate::Result<String>,
        is_new: bool,
    ) {
        let raw = match edited {
            Ok(raw) => raw,
            Err(e) => {
                self.message = format!("editor failed: {e}");
                return;
            }
        };
        let result = Store::default_path()
            .and_then(Store::open)
            .and_then(|store| {
                let item = shaic_core::store::parse_item(kind, &raw)?;
                if item.name() != name {
                    return Err(shaic_core::Error::Config(format!(
                        "renaming via edit is not supported — keep name {name:?}"
                    )));
                }
                store.save_item(&item)
            });
        match result {
            Ok(()) => {
                self.message = format!(
                    "{} {kind:?} {name:?}",
                    if is_new { "added" } else { "updated" }
                )
            }
            Err(e) => self.message = format!("save failed: {e}"),
        }
        self.refresh_browser();
    }

    // ---- Diff preview ----

    pub fn open_diff_preview(&mut self, agent: AgentId, scope: Scope, is_mcp: bool) {
        if scope == Scope::Project {
            match Config::load() {
                Ok(mut config) => {
                    if let Err(e) = config.ensure_project_registered(&self.project_root) {
                        self.message = format!("{e}");
                        return;
                    }
                }
                Err(e) => {
                    self.message = format!("could not load config: {e}");
                    return;
                }
            }
        }
        let Ok(store) = Store::default_path().and_then(Store::open) else {
            self.message = "no store yet".to_string();
            return;
        };
        let agent_impl = adapters::by_id(agent);
        let plan = if is_mcp {
            materialize::plan_mcp(agent_impl, &store, scope, &self.project_root)
                .map(PreviewPlan::Mcp)
        } else {
            materialize::plan_materialize(agent_impl, &store, scope, &self.project_root)
                .map(PreviewPlan::Base)
        };
        match plan {
            Ok(plan) => {
                self.diff = Some(DiffPreviewState { agent, scope, plan });
                self.screen = Screen::DiffPreview;
            }
            Err(e) => self.message = format!("could not compute plan: {e}"),
        }
    }

    pub fn apply_diff_preview(&mut self) {
        let Some(diff) = &self.diff else { return };
        let agent = diff.agent;
        let scope = diff.scope;
        let is_mcp = matches!(&diff.plan, PreviewPlan::Mcp(_));
        let agent_impl = adapters::by_id(agent);
        // Re-plan from the store (source of truth) then apply. Import is a
        // separate action — applying must not pull agent files into the store.
        let result = Store::default_path()
            .and_then(Store::open)
            .and_then(|store| {
                if is_mcp {
                    let plan =
                        materialize::plan_mcp(agent_impl, &store, scope, &self.project_root)?;
                    materialize::apply_mcp(agent_impl, &store, &plan, scope, &self.project_root)
                        .map(|report| (report.applied, report.warnings))
                } else {
                    let plan = materialize::plan_materialize(
                        agent_impl,
                        &store,
                        scope,
                        &self.project_root,
                    )?;
                    materialize::apply(agent_impl, &plan, scope, &self.project_root).map(|report| {
                        let changed = report
                            .writes
                            .iter()
                            .filter(|w| w.action != WriteAction::NoOp)
                            .count();
                        (changed, report.warnings)
                    })
                }
            });

        match result {
            Ok((changed, warnings)) => {
                self.message = format!("applied {changed} change(s){}", warn_suffix(&warnings))
            }
            Err(e) => self.message = format!("apply failed: {e}"),
        }
        self.return_from_diff_preview();
    }

    pub fn import_selected_scope(&mut self) {
        let Some(detail) = &self.detail else { return };
        let Some(sub) = detail.sub_rows.get(detail.selected_sub_row) else {
            return;
        };
        let agent = detail.agent;
        let scope = sub.scope;
        let agent_impl = adapters::by_id(agent);
        let mut pulled = 0;
        let mut rejections = Vec::new();
        let result = Store::default_path().and_then(Store::open).map(|store| {
            if let Ok(report) =
                materialize::reconcile_mcp(agent_impl, &store, scope, &self.project_root)
            {
                rejections.extend(report.rejected);
                pulled += report.pulled.len();
            }
            if agent_impl.supported_scopes().contains(&scope) {
                for &kind in agent_impl.supported_kinds() {
                    if let Ok(report) = materialize::reconcile_items(
                        agent_impl,
                        &store,
                        kind,
                        scope,
                        &self.project_root,
                    ) {
                        rejections.extend(report.rejected);
                        pulled += report.pulled.len();
                    }
                }
            }
        });
        self.pull_rejections = rejections;
        let rejected_suffix = if self.pull_rejections.is_empty() {
            String::new()
        } else {
            let names: Vec<_> = self
                .pull_rejections
                .iter()
                .map(|(name, reason)| format!("{name:?} ({reason})"))
                .collect();
            format!(" — skipped: {}", names.join(", "))
        };
        match result {
            Ok(()) => self.message = format!("imported {pulled} item(s){rejected_suffix}"),
            Err(e) => self.message = format!("import failed: {e}"),
        }
        self.refresh_dashboard();
        if let Some(idx) = self.agent_rows.iter().position(|r| r.id == agent) {
            self.selected_agent_row = idx;
        }
        self.open_agent_detail();
    }

    /// Leave the diff preview and land back on Agent Detail — the only
    /// screen that opens one — refreshing both its data and the dashboard's
    /// underlying `agent_rows` (an apply can change what any sub-row's
    /// glyph should read).
    pub fn return_from_diff_preview(&mut self) {
        self.diff = None;
        self.refresh_dashboard();
        if self.detail.is_some() {
            self.open_agent_detail();
        } else {
            self.screen = Screen::Dashboard;
        }
    }

    // ---- Agent detail ----

    pub fn open_agent_detail(&mut self) {
        let Some(row) = self.agent_rows.get(self.selected_agent_row) else {
            return;
        };
        let agent = row.id;
        let agent_impl = adapters::by_id(agent);
        let store = Store::default_path().and_then(Store::open).ok();

        let sub_rows: Vec<AgentDetailSubRow> = row
            .sub_rows
            .iter()
            .map(|sub| self.build_detail_sub_row(agent_impl, sub, &store))
            .collect();

        // Preserve whichever sub-row was highlighted before, if this is a
        // re-entry (e.g. right after applying a diff preview opened from
        // here) rather than resetting the user back to the top every time.
        let selected_sub_row = self
            .detail
            .as_ref()
            .map(|d| d.selected_sub_row)
            .unwrap_or(0)
            .min(sub_rows.len().saturating_sub(1));

        self.detail = Some(AgentDetailState {
            agent,
            display_name: agent_impl.display_name().to_string(),
            sub_rows,
            selected_sub_row,
        });
        self.screen = Screen::AgentDetail;
    }

    /// Builds one scope's detail row, covering whichever of content/MCP
    /// `sub` says this agent+scope actually supports. Glyphs are recomputed
    /// fresh from the plan here rather than reusing `sub`'s snapshot (from
    /// whenever the dashboard was last refreshed) — otherwise the status
    /// column can still read "drift" for a scope this same screen just
    /// reported as applied.
    fn build_detail_sub_row(
        &self,
        agent_impl: &dyn adapters::Agent,
        sub: &AgentSubRow,
        store: &Option<Store>,
    ) -> AgentDetailSubRow {
        let scope = sub.scope;
        let mut lines = Vec::new();

        let content_glyph = sub.content_glyph.is_some().then(|| {
            lines.push("── content ──".to_string());
            lines.push(match agent_impl.root(scope, &self.project_root) {
                Some(root) => format!("location: {}", root.display()),
                // No home directory to root this scope at, so nothing will be
                // written for it — say so instead of showing a path shaic
                // would refuse to use.
                None => "location: unavailable — no home directory on this machine".to_string(),
            });
            if agent_impl.experimental_read_only() {
                lines.push(
                    "convention unconfirmed — read-only, nothing will be written here".to_string(),
                );
                return "unconfirmed";
            }
            for &kind in agent_impl.supported_kinds() {
                let discovered = agent_impl.discover_existing(kind, scope, &self.project_root);
                lines.push(format!("{kind:?}: {} on disk", discovered.len()));
            }
            match store {
                Some(store) => {
                    match materialize::plan_materialize(
                        agent_impl,
                        store,
                        scope,
                        &self.project_root,
                    ) {
                        Ok(plan) => {
                            lines.push(pending_line(
                                plan.changed_writes().count(),
                                plan.deletes.len(),
                                "delete",
                            ));
                            plan_glyph(Ok(plan.is_empty()))
                        }
                        Err(e) => {
                            lines.push(format!("could not check for pending changes: {e}"));
                            "error"
                        }
                    }
                }
                None => "error",
            }
        });

        let mcp_glyph = sub.mcp_glyph.is_some().then(|| {
            let root = agent_impl
                .mcp_target(scope, &self.project_root)
                .map(|t| t.path)
                .unwrap_or_default();
            lines.push("── mcp servers ──".to_string());
            lines.push(format!("location: {}", root.display()));
            match store {
                Some(store) => {
                    match materialize::plan_mcp(agent_impl, store, scope, &self.project_root) {
                        Ok(plan) => {
                            lines.push(pending_line(
                                plan.changed_writes().count(),
                                plan.removals.len(),
                                "remove",
                            ));
                            plan_glyph(Ok(plan.is_empty()))
                        }
                        Err(e) => {
                            lines.push(format!("could not check for pending changes: {e}"));
                            "error"
                        }
                    }
                }
                None => "error",
            }
        });

        AgentDetailSubRow {
            scope,
            content_glyph,
            mcp_glyph,
            lines,
        }
    }

    pub fn selected_row(&self) -> Option<&AgentRow> {
        self.agent_rows.get(self.selected_agent_row)
    }
}

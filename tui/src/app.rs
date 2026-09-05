use std::path::PathBuf;

use shaic_core::adapters;
use shaic_core::config::Config;
use shaic_core::materialize::{self, MaterializePlan, McpPlan, WriteAction};
use shaic_core::model::{AgentId, ItemKind, Scope};
use shaic_core::store::Store;

use crate::theme::{MessageKind, Status};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Screen {
    SetupWizard,
    Dashboard,
    ItemBrowser,
    DiffPreview,
    AgentDetail,
}

/// One agent's sync state for a single scope, covering both content
/// (skills/rules/commands/subagents) and MCP servers in that scope — `None` for
/// whichever axis this agent/scope combination doesn't support. Kept as one
/// row per scope rather than one per (scope, axis): from the user's point of
/// view "this agent, this scope" is one thing, not two.
pub struct AgentSubRow {
    pub scope: Scope,
    pub content_glyph: Option<Status>,
    pub mcp_glyph: Option<Status>,
}

pub struct AgentRow {
    pub id: AgentId,
    pub name: String,
    /// Worst-of across every `sub_rows` entry — see `Status::worst`.
    pub glyph: Status,
    pub sub_rows: Vec<AgentSubRow>,
}

/// The status for one already-computed plan, keyed on whether it came
/// out empty (nothing to write) — shared by the base-content and MCP passes.
fn plan_glyph(in_sync: shaic_core::Result<bool>) -> Status {
    match in_sync {
        Ok(true) => Status::InSync,
        Ok(false) => Status::Drift,
        Err(_) => Status::Error,
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
/// (skills/rules/commands/subagents) or MCP server sync. Kept as an enum rather than
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
    pub content_glyph: Option<Status>,
    pub mcp_glyph: Option<Status>,
    pub lines: Vec<DetailLine>,
}

/// One line in Agent Detail with explicit severity — replaces the old
/// substring heuristic (`contains("up to date")`, ...) in `detail_line_color`.
#[derive(Debug, Clone)]
pub struct DetailLine {
    pub text: String,
    pub kind: MessageKind,
}

impl DetailLine {
    pub fn info(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            kind: MessageKind::Info,
        }
    }

    pub fn status(text: impl Into<String>, status: Status) -> Self {
        let kind = match status {
            Status::InSync => MessageKind::Success,
            Status::Drift => MessageKind::Warning,
            Status::Unconfirmed => MessageKind::Info,
            Status::Error => MessageKind::Error,
        };
        Self {
            text: text.into(),
            kind,
        }
    }

    pub fn error(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            kind: MessageKind::Error,
        }
    }
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
    PushForce,
    Pull,
    PullForce,
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
    pub message_kind: MessageKind,
    pub wizard: WizardState,
    pub browser: BrowserState,
    pub diff: Option<DiffPreviewState>,
    pub detail: Option<AgentDetailState>,
    pub pull_rejections: Vec<(String, String)>,
    pub pending_confirm: Option<PendingConfirm>,
    pub detail_scroll: u16,
    pub diff_scroll: u16,
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
            message_kind: MessageKind::Info,
            wizard: WizardState::default(),
            browser: BrowserState::default(),
            diff: None,
            detail: None,
            pull_rejections: Vec::new(),
            pending_confirm: None,
            detail_scroll: 0,
            diff_scroll: 0,
        };
        app.refresh_dashboard();
        Ok(app)
    }

    pub fn set_message(&mut self, kind: MessageKind, message: impl Into<String>) {
        self.message = message.into();
        self.message_kind = kind;
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
            // has no skills/rules/commands/subagents support for, or vice versa), so
            // each scope's row is built from the union of both axes rather
            // than assuming they line up.
            for &scope in &[Scope::Global, Scope::Project] {
                let content_glyph = agent.supported_scopes().contains(&scope).then(|| {
                    if agent.experimental_read_only() {
                        Status::Unconfirmed
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

            let glyph = Status::worst(
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
        self.detail_scroll = 0;
    }

    pub fn scroll_detail(&mut self, delta: i32) {
        self.detail_scroll = self.detail_scroll.saturating_add_signed(delta as i16);
    }

    pub fn scroll_diff(&mut self, delta: i32) {
        self.diff_scroll = self.diff_scroll.saturating_add_signed(delta as i16);
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
            PendingConfirm::PushForce => {
                "push blocked by secret scan — push anyway? [y/N]".to_string()
            }
            PendingConfirm::Pull => "pull store from remote? [y/N]".to_string(),
            PendingConfirm::PullForce => {
                "pull blocked by secret scan — pull anyway? [y/N]".to_string()
            }
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
        self.set_message(MessageKind::Info, prompt);
    }

    pub fn cancel_confirm(&mut self) {
        self.pending_confirm = None;
        self.set_message(MessageKind::Info, "cancelled");
    }

    pub fn take_confirm(&mut self) -> Option<PendingConfirm> {
        self.pending_confirm.take()
    }

    fn operation_kind_to_message(kind: shaic_core::operations::OperationKind) -> MessageKind {
        match kind {
            shaic_core::operations::OperationKind::Success => MessageKind::Success,
            shaic_core::operations::OperationKind::Warning => MessageKind::Warning,
            shaic_core::operations::OperationKind::Error => MessageKind::Error,
            shaic_core::operations::OperationKind::Info => MessageKind::Info,
        }
    }

    pub fn push(&mut self) {
        self.push_with(false);
    }

    fn push_with(&mut self, allow_secrets: bool) {
        let result = Store::default_path().and_then(Store::open).and_then(|s| {
            shaic_core::operations::push_store(&s, allow_secrets).map_err(|e| match e {
                shaic_core::Error::SecretDetected(msg) if !allow_secrets => {
                    shaic_core::Error::SecretDetected(msg)
                }
                other => other,
            })
        });
        match result {
            Ok((msg, kind)) => self.set_message(Self::operation_kind_to_message(kind), msg),
            Err(shaic_core::Error::SecretDetected(msg)) if !allow_secrets => {
                self.set_message(
                    MessageKind::Error,
                    format!(
                        "push blocked by secret scan: {msg} — use CLI `--i-know-what-im-doing` or confirm override [y/N]"
                    ),
                );
                self.pending_confirm = Some(PendingConfirm::PushForce);
            }
            Err(e) => self.set_message(MessageKind::Error, format!("push failed: {e}")),
        }
        // Refresh unless we're waiting on an override confirm (which would
        // wipe the explanatory message's context by redrawing stale rows).
        if self.pending_confirm.is_none() {
            self.refresh_dashboard();
        }
    }

    pub fn push_force(&mut self) {
        self.push_with(true);
    }

    pub fn pull(&mut self) {
        self.pull_with(false);
    }

    fn pull_with(&mut self, allow_secrets: bool) {
        let result = Store::default_path()
            .and_then(Store::open)
            .and_then(|s| shaic_core::operations::pull_store(&s, allow_secrets));
        match result {
            Ok((msg, kind)) => self.set_message(Self::operation_kind_to_message(kind), msg),
            Err(shaic_core::Error::SecretDetected(msg)) if !allow_secrets => {
                self.set_message(
                    MessageKind::Error,
                    format!("pull blocked by secret scan: {msg} — confirm override? [y/N]"),
                );
                self.pending_confirm = Some(PendingConfirm::PullForce);
            }
            Err(e) => self.set_message(MessageKind::Error, format!("pull failed: {e}")),
        }
        if self.pending_confirm.is_none() {
            self.refresh_dashboard();
        }
    }

    pub fn pull_force(&mut self) {
        self.pull_with(true);
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
        let (msg, kind) = match Config::load() {
            Ok(mut config) => {
                if config.set_remote(&url).is_ok() {
                    let _ = config.save();
                }
                (
                    format!(
                        "store ready (remote: {})",
                        shaic_core::store::git::redact_userinfo(&url)
                    ),
                    MessageKind::Success,
                )
            }
            Err(e) => (
                format!(
                    "store ready (remote: {}), but config is corrupted and was left untouched: {e}",
                    shaic_core::store::git::redact_userinfo(&url)
                ),
                MessageKind::Warning,
            ),
        };
        self.set_message(kind, msg);
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
            self.set_message(MessageKind::Warning, "no item selected");
            return None;
        };
        let store = match Store::default_path().and_then(Store::open) {
            Ok(s) => s,
            Err(e) => {
                self.set_message(MessageKind::Error, format!("could not open store: {e}"));
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
                self.set_message(
                    MessageKind::Error,
                    format!("could not load {:?} {:?}: {e}", row.kind, row.name),
                );
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
            ItemKind::Command => ItemKind::Subagent,
            ItemKind::Subagent => ItemKind::Skill,
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
            self.set_message(
                MessageKind::Error,
                format!("{name:?} is not a valid item name"),
            );
            return None;
        }
        let kind = self.browser.pending_kind;
        Some(PendingAction::EditItem {
            initial: shaic_core::store::item_template(kind, &name),
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
            self.set_message(MessageKind::Error, "no store yet");
            return;
        };
        match store.remove_item(row.kind, &row.name) {
            Ok(()) => {
                let (applied, notes) = materialize::push_all_now(&store, &self.project_root);
                if let Some(note) = notes.first() {
                    self.set_message(
                        MessageKind::Warning,
                        format!("removed {:?} {:?}, but: {note}", row.kind, row.name),
                    );
                } else {
                    self.set_message(
                        MessageKind::Success,
                        format!(
                            "removed {:?} {:?}, pushed to {applied} agent/scope(s)",
                            row.kind, row.name
                        ),
                    );
                }
            }
            Err(e) => self.set_message(MessageKind::Error, format!("remove failed: {e}")),
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
                self.set_message(MessageKind::Error, format!("editor failed: {e}"));
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
            Ok(()) => self.set_message(
                MessageKind::Success,
                format!(
                    "{} {kind:?} {name:?}",
                    if is_new { "added" } else { "updated" }
                ),
            ),
            Err(e) => self.set_message(MessageKind::Error, format!("save failed: {e}")),
        }
        self.refresh_browser();
    }

    // ---- Diff preview ----

    pub fn open_diff_preview(&mut self, agent: AgentId, scope: Scope, is_mcp: bool) {
        if scope == Scope::Project {
            match Config::load() {
                Ok(mut config) => {
                    if let Err(e) = config.ensure_project_registered(&self.project_root) {
                        self.set_message(MessageKind::Error, format!("{e}"));
                        return;
                    }
                }
                Err(e) => {
                    self.set_message(MessageKind::Error, format!("could not load config: {e}"));
                    return;
                }
            }
        }
        let Ok(store) = Store::default_path().and_then(Store::open) else {
            self.set_message(MessageKind::Error, "no store yet");
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
                self.diff_scroll = 0;
                self.screen = Screen::DiffPreview;
            }
            Err(e) => self.set_message(MessageKind::Error, format!("could not compute plan: {e}")),
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
                let kind = if warnings.is_empty() {
                    MessageKind::Success
                } else {
                    MessageKind::Warning
                };
                self.set_message(
                    kind,
                    format!("applied {changed} change(s){}", warn_suffix(&warnings)),
                );
            }
            Err(e) => self.set_message(MessageKind::Error, format!("apply failed: {e}")),
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
        let result = Store::default_path().and_then(Store::open).map(|store| {
            shaic_core::operations::import_scope(
                agent_impl,
                &store,
                scope,
                &self.project_root,
                false,
            )
        });
        match result {
            Ok(summary) => {
                self.pull_rejections = summary.rejected.clone();
                let (msg, op_kind) = summary.message();
                let kind = Self::operation_kind_to_message(op_kind);
                let mut full = msg;
                if !summary.warnings.is_empty() {
                    full.push_str(&format!(" — warnings: {}", summary.warnings.join("; ")));
                }
                if !summary.errors.is_empty() {
                    full.push_str(&format!(" — errors: {}", summary.errors.join("; ")));
                }
                self.set_message(kind, full);
            }
            Err(e) => self.set_message(MessageKind::Error, format!("import failed: {e}")),
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
        self.detail_scroll = 0;
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
        let mut lines: Vec<DetailLine> = Vec::new();

        let content_glyph = sub.content_glyph.is_some().then(|| {
            lines.push(DetailLine::info("── content ──"));
            lines.push(DetailLine::info(
                match agent_impl.root(scope, &self.project_root) {
                    Some(root) => format!("location: {}", root.display()),
                    // No home directory to root this scope at, so nothing will be
                    // written for it — say so instead of showing a path shaic
                    // would refuse to use.
                    None => "location: unavailable — no home directory on this machine".to_string(),
                },
            ));
            if agent_impl.experimental_read_only() {
                lines.push(DetailLine::info(
                    "convention unconfirmed — read-only, nothing will be written here",
                ));
                return Status::Unconfirmed;
            }
            for &kind in agent_impl.supported_kinds() {
                let discovered = agent_impl.discover_existing(kind, scope, &self.project_root);
                lines.push(DetailLine::info(format!(
                    "{kind:?}: {} on disk",
                    discovered.len()
                )));
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
                            let status = plan_glyph(Ok(plan.is_empty()));
                            lines.push(DetailLine::status(
                                pending_line(
                                    plan.changed_writes().count(),
                                    plan.deletes.len(),
                                    "delete",
                                ),
                                status,
                            ));
                            status
                        }
                        Err(e) => {
                            lines.push(DetailLine::error(format!(
                                "could not check for pending changes: {e}"
                            )));
                            Status::Error
                        }
                    }
                }
                None => Status::Error,
            }
        });

        let mcp_glyph = sub.mcp_glyph.is_some().then(|| {
            let root = agent_impl
                .mcp_target(scope, &self.project_root)
                .map(|t| t.path)
                .unwrap_or_default();
            lines.push(DetailLine::info("── mcp servers ──"));
            lines.push(DetailLine::info(format!("location: {}", root.display())));
            match store {
                Some(store) => {
                    match materialize::plan_mcp(agent_impl, store, scope, &self.project_root) {
                        Ok(plan) => {
                            let status = plan_glyph(Ok(plan.is_empty()));
                            lines.push(DetailLine::status(
                                pending_line(
                                    plan.changed_writes().count(),
                                    plan.removals.len(),
                                    "remove",
                                ),
                                status,
                            ));
                            status
                        }
                        Err(e) => {
                            lines.push(DetailLine::error(format!(
                                "could not check for pending changes: {e}"
                            )));
                            Status::Error
                        }
                    }
                }
                None => Status::Error,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detail_line_status_maps_to_message_kind() {
        assert_eq!(
            DetailLine::status("up to date", Status::InSync).kind,
            MessageKind::Success
        );
        assert_eq!(
            DetailLine::status("not yet pushed", Status::Drift).kind,
            MessageKind::Warning
        );
        assert_eq!(
            DetailLine::status("unconfirmed", Status::Unconfirmed).kind,
            MessageKind::Info
        );
        assert_eq!(DetailLine::error("boom").kind, MessageKind::Error);
    }

    #[test]
    fn operation_kind_maps_to_message_kind() {
        use shaic_core::operations::OperationKind;
        assert_eq!(
            App::operation_kind_to_message(OperationKind::Success),
            MessageKind::Success
        );
        assert_eq!(
            App::operation_kind_to_message(OperationKind::Error),
            MessageKind::Error
        );
    }
}

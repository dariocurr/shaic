//! Shared store↔agent operations used by both CLI and TUI.
//!
//! The CLI previously formatted `push`/`pull`/`import` results inline while
//! the TUI reimplemented the same logic (including a hardcoded
//! `allow_secrets = false` with no override path). Centralizing here keeps
//! messages consistent and surfaces per-kind import errors instead of
//! swallowing them with `if let Ok`.

use std::path::Path;

use crate::adapters::Agent;
use crate::materialize::{self, ReconcileReport};
use crate::model::Scope;
use crate::store::{PullResult, PushResult, Store};

/// Severity for a human-readable operation message. Mirrors the TUI's
/// `MessageKind` without depending on it — CLI maps to exit codes/prints,
// TUI maps to colors.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OperationKind {
    Success,
    Info,
    Warning,
    Error,
}

pub fn format_push(result: &PushResult) -> (String, OperationKind) {
    match (&result.pushed, &result.committed, &result.summary) {
        (true, true, Some(summary)) => (format!("pushed: {summary}"), OperationKind::Success),
        (true, false, Some(summary)) => (
            format!("pushed previously-unpushed commits: {summary}"),
            OperationKind::Success,
        ),
        (true, _, None) => ("pushed".to_string(), OperationKind::Success),
        (false, _, _) => (
            "nothing to push — store is clean".to_string(),
            OperationKind::Info,
        ),
    }
}

pub fn format_pull(result: &PullResult) -> (String, OperationKind) {
    if result.updated {
        ("pulled changes".to_string(), OperationKind::Success)
    } else {
        ("already up to date".to_string(), OperationKind::Info)
    }
}

pub fn push_store(
    store: &Store,
    allow_secrets: bool,
) -> Result<(String, OperationKind), crate::Error> {
    store.push(allow_secrets).map(|r| format_push(&r))
}

pub fn pull_store(
    store: &Store,
    allow_secrets: bool,
) -> Result<(String, OperationKind), crate::Error> {
    store.pull(allow_secrets).map(|r| format_pull(&r))
}

/// One agent+scope import pass, collecting per-axis results instead of
/// swallowing `Err` with `if let Ok` (the old TUI behavior hid which kind
/// failed and why).
#[derive(Debug, Default)]
pub struct ImportSummary {
    pub pulled: Vec<String>,
    pub rejected: Vec<(String, String)>,
    pub warnings: Vec<String>,
    pub errors: Vec<String>,
}

impl ImportSummary {
    pub fn is_empty(&self) -> bool {
        self.pulled.is_empty() && self.rejected.is_empty() && self.errors.is_empty()
    }

    pub fn message(&self) -> (String, OperationKind) {
        let mut parts = vec![format!("imported {} item(s)", self.pulled.len())];
        if !self.rejected.is_empty() {
            let names: Vec<_> = self
                .rejected
                .iter()
                .map(|(name, reason)| format!("{name:?} ({reason})"))
                .collect();
            parts.push(format!("skipped: {}", names.join(", ")));
        }
        if !self.errors.is_empty() {
            parts.push(format!("errors: {}", self.errors.join("; ")));
        }
        let kind = if !self.errors.is_empty() {
            OperationKind::Error
        } else if !self.rejected.is_empty() || !self.warnings.is_empty() {
            OperationKind::Warning
        } else {
            OperationKind::Success
        };
        (parts.join(" — "), kind)
    }
}

fn extend_from_report(summary: &mut ImportSummary, report: ReconcileReport) {
    summary.pulled.extend(report.pulled);
    summary.rejected.extend(report.rejected);
    summary.warnings.extend(report.warnings);
}

pub fn import_scope(
    agent: &dyn Agent,
    store: &Store,
    scope: Scope,
    project_root: &Path,
    force: bool,
) -> ImportSummary {
    let mut summary = ImportSummary::default();
    match materialize::reconcile_mcp(agent, store, scope, project_root) {
        Ok(report) => extend_from_report(&mut summary, report),
        Err(e) => summary.errors.push(format!(
            "MCP import {} / {scope:?}: {e}",
            agent.display_name()
        )),
    }
    if agent.supported_scopes().contains(&scope) {
        for &kind in agent.supported_kinds() {
            match materialize::reconcile_items(agent, store, kind, scope, project_root, force) {
                Ok(report) => extend_from_report(&mut summary, report),
                Err(e) => summary.errors.push(format!(
                    "{kind:?} import {} / {scope:?}: {e}",
                    agent.display_name()
                )),
            }
        }
    }
    summary
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_push_messages() {
        let (msg, kind) = format_push(&PushResult {
            committed: true,
            pushed: true,
            summary: Some("shaic: update 2 item(s)".to_string()),
        });
        assert!(msg.contains("pushed:"));
        assert_eq!(kind, OperationKind::Success);

        let (msg, kind) = format_push(&PushResult {
            committed: false,
            pushed: false,
            summary: None,
        });
        assert!(msg.contains("nothing to push"));
        assert_eq!(kind, OperationKind::Info);
    }

    #[test]
    fn import_summary_reports_errors_not_silence() {
        let mut s = ImportSummary::default();
        s.errors.push("Rule import X: boom".to_string());
        let (msg, kind) = s.message();
        assert!(msg.contains("boom"));
        assert_eq!(kind, OperationKind::Error);
    }
}

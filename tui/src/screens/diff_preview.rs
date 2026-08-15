use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::Style;
use ratatui::text::Line;
use ratatui::widgets::{List, ListItem, Paragraph};

use shaic_core::materialize::WriteAction;

use crate::app::{App, PreviewPlan};
use crate::screens;
use crate::theme;

/// Shows exactly what `sync` would change for one agent+scope before
/// anything is written — the TUI's expression of materialize being the
/// security boundary. No write happens without landing on this screen (or
/// its CLI `--dry-run` equivalent) first. Lines use `+`/`~`/`-` prefixes
/// like a real diff, since that's exactly what this screen is.
pub fn draw(frame: &mut Frame, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(3),
            Constraint::Min(3),
            Constraint::Length(4),
        ])
        .split(frame.area());

    frame.render_widget(screens::header_line("diff preview"), chunks[0]);

    let Some(diff) = &app.diff else {
        frame.render_widget(Paragraph::new("nothing to preview"), chunks[2]);
        return;
    };

    let suffix = if matches!(diff.plan, PreviewPlan::Mcp(_)) {
        " (mcp)"
    } else {
        ""
    };
    let title = format!(
        "materialize preview — {:?} / {:?}{suffix}",
        diff.agent, diff.scope
    );
    frame.render_widget(
        Paragraph::new(title).block(screens::panel("plan")),
        chunks[1],
    );

    let mut lines: Vec<ListItem> = Vec::new();
    // Both plan shapes render the same way — a labelled line per changed
    // entry, then per dropped entry — over different field names.
    let (skipped, warnings) = match &diff.plan {
        PreviewPlan::Base(plan) => {
            for write in plan.changed_writes() {
                if let Some(line) = change_line(write.action, &write.relative_path.display()) {
                    lines.push(line);
                }
            }
            for delete in &plan.deletes {
                lines.push(dropped_line("delete", &delete.relative_path.display()));
            }
            (&plan.skipped, &plan.warnings)
        }
        PreviewPlan::Mcp(plan) => {
            for write in plan.changed_writes() {
                let target = if write.summary.is_empty() {
                    write.name.clone()
                } else {
                    format!("{} ({})", write.name, write.summary)
                };
                if let Some(line) = change_line(write.action, &target) {
                    lines.push(line);
                }
            }
            for name in &plan.removals {
                lines.push(dropped_line("remove", name));
            }
            (&plan.skipped, &plan.warnings)
        }
    };
    for note in skipped {
        lines.push(ListItem::new(Line::styled(
            format!("[skip] {note}"),
            Style::default().fg(theme::WARNING),
        )));
    }
    for note in warnings {
        lines.push(ListItem::new(Line::styled(
            format!("[warn] {note}"),
            Style::default().fg(theme::WARNING),
        )));
    }
    if lines.is_empty() {
        lines.push(ListItem::new(Line::styled(
            "everything already in sync — nothing to apply",
            Style::default().fg(theme::SUCCESS),
        )));
    }
    let list = List::new(lines).block(screens::panel_focused("planned changes"));
    frame.render_widget(list, chunks[2]);

    frame.render_widget(
        screens::footer(&app.message, "   [a=apply (asks y/N)  ?=help  Esc=cancel]"),
        chunks[3],
    );
}

/// One pending create/update, rendered like a diff hunk line (`+`/`~`).
/// `WriteAction::NoOp` is skipped — callers already iterate `changed_writes()`,
/// but a plan bug must not panic the TUI.
fn change_line(action: WriteAction, target: &dyn std::fmt::Display) -> Option<ListItem<'static>> {
    let (symbol, label, color) = match action {
        WriteAction::Create => ("+", "create", theme::SUCCESS),
        WriteAction::Update => ("~", "update", theme::WARNING),
        WriteAction::NoOp => return None,
    };
    Some(ListItem::new(Line::styled(
        format!("{symbol} {label:<7} {target}"),
        Style::default().fg(color),
    )))
}

/// One pending delete (a file) or removal (an MCP server entry), rendered
/// like a diff hunk's removed line (`-`).
fn dropped_line(label: &str, target: &dyn std::fmt::Display) -> ListItem<'static> {
    ListItem::new(Line::styled(
        format!("- {label:<7} {target}"),
        Style::default().fg(theme::DANGER),
    ))
}

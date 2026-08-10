pub mod agent_detail;
pub mod dashboard;
pub mod diff_preview;
pub mod help;
pub mod item_browser;
pub mod setup_wizard;

use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Paragraph, Wrap};

use crate::theme;

/// One-line wordmark + breadcrumb every full screen opens with. Cheap
/// wayfinding (which screen am I on, inside which app) for one row of
/// vertical space — and the one place the accent color appears unconditionally,
/// so it reads as shaic's signature rather than just "a status color". Each
/// screen reserves a `Constraint::Length(1)` chunk at the top of its layout
/// and renders this into it.
pub(crate) fn header_line(breadcrumb: &str) -> Paragraph<'static> {
    Paragraph::new(Line::from(vec![
        Span::styled(
            theme::WORDMARK,
            Style::default()
                .fg(theme::ACCENT)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("  ›  {breadcrumb}"),
            Style::default().add_modifier(Modifier::DIM),
        ),
    ]))
}

/// Block for the pane a screen is actively navigating (the thing ↑/↓ moves
/// through) — accent border, so it's visually obvious what's interactive
/// without every panel on screen shouting for attention equally.
pub(crate) fn panel_focused(title: impl Into<String>) -> Block<'static> {
    Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(theme::ACCENT))
        .title(Span::styled(
            title.into(),
            Style::default()
                .fg(theme::ACCENT)
                .add_modifier(Modifier::BOLD),
        ))
}

/// Block for a secondary/read-only pane (a preview, a detail list) — same
/// rounded shape as `panel_focused` for a consistent silhouette, quieter
/// border so it doesn't compete with the pane you're actually driving.
pub(crate) fn panel(title: impl Into<String>) -> Block<'static> {
    Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .title(Span::styled(
            title.into(),
            Style::default().add_modifier(Modifier::BOLD),
        ))
}

/// The bordered status line every full screen ends with: the app's current
/// message, colored by its own vocabulary, followed by that screen's key hints
/// (dimmed — the message is the thing worth noticing, the hints are reference).
pub(crate) fn footer(message: &str, hints: &str) -> Paragraph<'static> {
    framed_footer(Line::from(vec![
        Span::styled(
            message.to_string(),
            Style::default().fg(theme::message_color(message)),
        ),
        Span::styled(
            hints.to_string(),
            Style::default().add_modifier(Modifier::DIM),
        ),
    ]))
}

/// `footer` for a screen whose status line isn't a message+hints pair (the item
/// browser's "new item name" prompt).
pub(crate) fn framed_footer(line: Line<'static>) -> Paragraph<'static> {
    Paragraph::new(line).wrap(Wrap { trim: false }).block(
        Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded),
    )
}

/// Highlight for the currently selected row of a list or table: a solid
/// background tint plus bold, rather than `Modifier::REVERSED` (see
/// `theme::SELECTION_BG` for why reversed video is the wrong tool here).
pub(crate) fn row_style(selected: bool) -> Style {
    if selected {
        Style::default()
            .bg(theme::SELECTION_BG)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default()
    }
}

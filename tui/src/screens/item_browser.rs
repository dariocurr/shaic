use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{List, ListItem, Paragraph};

use crate::app::App;
use crate::screens;
use crate::theme;

/// Canonical skills/rules/commands, one flat list (kind shown per row).
/// `a` adds, `e` edits the selected item, `d` removes it — add/edit both
/// suspend the TUI and hand off to `$EDITOR`.
pub fn draw(frame: &mut Frame, app: &App) {
    let outer = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(3),
            Constraint::Length(4),
        ])
        .split(frame.area());

    frame.render_widget(screens::header_line("skills / rules / commands"), outer[0]);

    let panes = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(40), Constraint::Percentage(60)])
        .split(outer[1]);

    let items: Vec<ListItem> = app
        .browser
        .items
        .iter()
        .enumerate()
        .map(|(i, row)| {
            let line = Line::from(vec![
                Span::styled(
                    format!("[{:?}]", row.kind),
                    Style::default().fg(theme::kind_color(row.kind)),
                ),
                Span::raw(format!(" {}", row.name)),
            ]);
            ListItem::new(line).style(screens::row_style(i == app.browser.selected))
        })
        .collect();
    let list = List::new(items).block(screens::panel_focused("items"));
    frame.render_widget(list, panes[0]);

    let preview_text = match app.selected_item() {
        Some(row) => vec![
            Line::from(format!("kind: {:?}", row.kind)),
            Line::from(format!("name: {}", row.name)),
            Line::from(format!("description: {}", row.description)),
        ],
        None => vec![
            Line::from("no items yet — press 'a' to add one"),
            Line::from(""),
            Line::from("already have skills/rules for an agent? run `shaic agents discover`"),
            Line::from("in a terminal to see what's on disk but not in the store yet."),
        ],
    };
    let preview = Paragraph::new(preview_text).block(screens::panel("preview"));
    frame.render_widget(preview, panes[1]);

    let footer = match &app.browser.name_input {
        Some(name) => screens::framed_footer(Line::from(format!(
            "new [{:?}] name: {name}_   [Tab=change kind  Enter=confirm  Esc=cancel]",
            app.browser.pending_kind
        ))),
        None => screens::footer(
            &app.message,
            "   [↑/↓=move a=add e=edit d=delete  ?=help  Esc=back]",
        ),
    };
    frame.render_widget(footer, outer[2]);
}

use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::{Modifier, Style};
use ratatui::text::Span;
use ratatui::widgets::{Cell, Row, Table};

use crate::app::App;
use crate::screens;
use crate::theme;

/// The single "am I okay?" glance screen: one row per agent, showing the
/// worst status across every scope/content axis that agent supports. `Enter`
/// drills into Agent Detail, where those axes are broken out individually.
pub fn draw(frame: &mut Frame, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(3),
            Constraint::Length(4),
        ])
        .split(frame.area());

    frame.render_widget(screens::header_line("dashboard"), chunks[0]);

    let rows: Vec<Row> = app
        .agent_rows
        .iter()
        .enumerate()
        .map(|(i, row)| {
            Row::new(vec![
                Cell::from(row.name.clone()),
                Cell::from(Span::styled(
                    format!("{} {}", theme::glyph_icon(row.glyph), row.glyph),
                    Style::default().fg(theme::glyph_color(row.glyph)),
                )),
            ])
            .style(screens::row_style(i == app.selected_agent_row))
        })
        .collect();

    let table = Table::new(rows, [Constraint::Min(24), Constraint::Length(16)])
        .header(
            Row::new(vec!["agent", "status"]).style(Style::default().add_modifier(Modifier::BOLD)),
        )
        .block(screens::panel_focused("agents (↑/↓ select, Enter=detail)"));
    frame.render_widget(table, chunks[1]);

    frame.render_widget(
        screens::footer(
            &app.message,
            "   [p=push u=pull s=browse skill  i=setup  ?=help  q=quit]",
        ),
        chunks[2],
    );
}

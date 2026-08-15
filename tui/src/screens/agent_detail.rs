use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Cell, List, ListItem, Paragraph, Row, Table};

use crate::app::App;
use crate::screens;
use crate::theme;

/// Drill-down from a dashboard row: every scope/content-axis sub-row for
/// this agent (Global/Project, base content and MCP, whichever the agent
/// actually supports), selectable with ↑/↓ — the highlighted one's resolved
/// root path, discovered file counts, and pending-plan summary are shown
/// below. `y` opens a Diff Preview for whichever sub-row is highlighted.
pub fn draw(frame: &mut Frame, app: &App) {
    let Some(detail) = &app.detail else {
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1),
                Constraint::Length(3),
                Constraint::Min(3),
            ])
            .split(frame.area());
        frame.render_widget(screens::header_line("agent detail"), chunks[0]);
        frame.render_widget(Paragraph::new("no agent selected"), chunks[2]);
        return;
    };

    // header + border-top + border-bottom, plus one line per sub-row.
    let sub_table_height = detail.sub_rows.len() as u16 + 3;
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(3),
            Constraint::Length(sub_table_height),
            Constraint::Min(3),
            Constraint::Length(4),
        ])
        .split(frame.area());

    frame.render_widget(screens::header_line("agent detail"), chunks[0]);

    frame.render_widget(
        Paragraph::new(detail.display_name.as_str()).block(screens::panel("agent")),
        chunks[1],
    );

    let sub_rows: Vec<Row> = detail
        .sub_rows
        .iter()
        .enumerate()
        .map(|(i, sub)| {
            let mut spans = Vec::new();
            if let Some(glyph) = sub.content_glyph {
                spans.push(Span::styled(
                    format!("content {} {glyph}", theme::glyph_icon(glyph)),
                    Style::default().fg(theme::glyph_color(glyph)),
                ));
            }
            if let Some(glyph) = sub.mcp_glyph {
                if !spans.is_empty() {
                    spans.push(Span::raw("   "));
                }
                spans.push(Span::styled(
                    format!("mcp {} {glyph}", theme::glyph_icon(glyph)),
                    Style::default().fg(theme::glyph_color(glyph)),
                ));
            }
            Row::new(vec![
                Cell::from(format!("{:?}", sub.scope)),
                Cell::from(Line::from(spans)),
            ])
            .style(screens::row_style(i == detail.selected_sub_row))
        })
        .collect();
    let table = Table::new(sub_rows, [Constraint::Length(10), Constraint::Min(24)])
        .header(
            Row::new(vec!["scope", "status"]).style(Style::default().add_modifier(Modifier::BOLD)),
        )
        .block(screens::panel_focused("scope"));
    frame.render_widget(table, chunks[2]);

    let items: Vec<ListItem> = detail
        .sub_rows
        .get(detail.selected_sub_row)
        .map(|sub| sub.lines.clone())
        .unwrap_or_default()
        .into_iter()
        .map(|line| {
            let color = detail_line_color(&line);
            ListItem::new(Line::styled(line, Style::default().fg(color)))
        })
        .collect();
    frame.render_widget(List::new(items).block(screens::panel("details")), chunks[3]);

    frame.render_widget(
        screens::footer(
            &app.message,
            "   [↑/↓ select scope  y=content diff  m=mcp diff  ?=help  Esc=back]",
        ),
        chunks[4],
    );
}

/// Colors a detail line by what it reports. Same "read the message we
/// ourselves produced" heuristic as `theme::message_color`, matched against
/// the plan-summary strings `App::build_*_detail_sub_row` builds.
fn detail_line_color(line: &str) -> Color {
    if line.contains("could not") {
        theme::message_color(line)
    } else if line.contains("up to date") {
        theme::glyph_color("in-sync")
    } else if line.contains("not yet pushed") {
        theme::glyph_color("drift")
    } else {
        Color::Reset
    }
}

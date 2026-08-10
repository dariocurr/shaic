use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, BorderType, Borders, Clear, Paragraph, Wrap};

use crate::theme;

/// Full key-binding reference plus the one concept that trips people up:
/// the store (what `shaic` will sync out) and each agent's files already on
/// disk (what's there today, whether or not shaic put it there) are two
/// separate things — see `agents discover` / the item browser's "add" flow
/// to bring existing content in.
const BODY: &[&str] = &[
    "shaic tracks two separate things:",
    "  the store    — canonical skills/rules/commands/MCP servers you've",
    "                 added; this is what gets synced out to every agent.",
    "  agent files  — whatever's already on disk for an agent (may exist",
    "                 before shaic ever touched it). 'agents discover'",
    "                 (CLI) lists these; 'a' in the item browser imports one.",
    "",
    "Dashboard",
    "  one row per agent, showing its worst status across every scope and",
    "  content/MCP axis it supports. ↑/↓ select   Enter=agent detail",
    "  s=browse skills/rules/commands   p=push store   u=pull store",
    "  i=setup wizard   r=refresh   q=quit",
    "",
    "Skills / Rules / Commands browser",
    "  ↑/↓ move   a=add new   e=edit selected   d=delete selected   Esc=back",
    "",
    "Diff preview (shown before anything is written)",
    "  a=apply the listed changes   Esc=cancel, write nothing",
    "",
    "Agent detail",
    "  breaks the selected agent down by scope (Global/Project), each row",
    "  showing both content and MCP status together.",
    "  ↑/↓ select a row   y=content diff preview   m=mcp diff preview   Esc=back",
    "",
    "Setup wizard (first run, or 'i' from the dashboard)",
    "  type the git remote URL, Enter=clone/init it, Esc=skip for now",
];

pub fn draw(frame: &mut Frame) {
    let area = centered(80, 80, frame.area());
    frame.render_widget(Clear, area);

    let lines: Vec<Line> = BODY.iter().map(|l| Line::from(*l)).collect();
    let popup = Paragraph::new(lines).wrap(Wrap { trim: false }).block(
        Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(theme::ACCENT))
            .title("help — press ? or Esc to close")
            .title_style(
                Style::default()
                    .fg(theme::ACCENT)
                    .add_modifier(Modifier::BOLD),
            ),
    );
    frame.render_widget(popup, area);
}

fn centered(percent_x: u16, percent_y: u16, area: Rect) -> Rect {
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(area);
    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(vertical[1])[1]
}

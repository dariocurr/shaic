use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Paragraph, Wrap};

use crate::app::App;
use crate::screens;
use crate::theme;

/// First-run screen: a single input for the store's remote URL. Enter
/// validates reachability (non-destructive `git ls-remote`) and then clones
/// or initializes the store; Esc skips straight to the (mostly empty)
/// dashboard, which can always be revisited via `i`.
pub fn draw(frame: &mut Frame, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Length(3),
            Constraint::Min(3),
            Constraint::Length(3),
        ])
        .split(frame.area());

    let title = Paragraph::new(Line::from(vec![
        Span::styled(
            theme::WORDMARK,
            Style::default()
                .fg(theme::ACCENT)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(" — let's set up your canonical store."),
    ]))
    .block(screens::panel("setup"));
    frame.render_widget(title, chunks[0]);

    let input_line = Line::from(format!("remote url: {}_", app.wizard.remote_input));
    let input = Paragraph::new(input_line).block(screens::panel_focused(
        "git remote (e.g. git@github.com:you/shaic-store.git)",
    ));
    frame.render_widget(input, chunks[1]);

    let help = Paragraph::new(vec![
        Line::from("Type the remote, then press Enter to clone/init it."),
        Line::from("Press Esc to skip setup for now (you can run this later with 'i')."),
        Line::styled(
            app.wizard.status.as_str(),
            Style::default()
                .fg(theme::message_color(&app.wizard.status))
                .add_modifier(Modifier::BOLD),
        ),
    ])
    .wrap(Wrap { trim: false });
    frame.render_widget(help, chunks[2]);

    let footer = Paragraph::new(Line::styled(
        "Enter=go  Esc=skip  Backspace=edit  ?=help",
        Style::default().add_modifier(Modifier::DIM),
    ))
    .block(
        Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded),
    );
    frame.render_widget(footer, chunks[3]);
}

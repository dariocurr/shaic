mod app;
pub mod screens;
mod theme;

use crossterm::event::{self, Event, KeyCode, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;

use app::{App, PendingAction, Screen};

/// Dependency-free stand-in for `anyhow::Result`: this crate's failures are
/// all one-shot terminal/IO errors that just need to surface to the caller,
/// not be matched on, so a boxed trait object is enough.
pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

/// Entry point used by the `shaic tui` CLI command (and the default action
/// when `shaic` is run with no subcommand in an interactive terminal).
pub fn run() -> Result<()> {
    enable_raw_mode()?;
    let mut stdout = std::io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let mut app = App::new()?;
    let result = event_loop(&mut terminal, &mut app);

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;
    result
}

type Term = Terminal<CrosstermBackend<std::io::Stdout>>;

fn event_loop(terminal: &mut Term, app: &mut App) -> Result<()> {
    loop {
        terminal.draw(|frame| {
            match app.screen {
                Screen::SetupWizard => screens::setup_wizard::draw(frame, app),
                Screen::Dashboard => screens::dashboard::draw(frame, app),
                Screen::ItemBrowser => screens::item_browser::draw(frame, app),
                Screen::DiffPreview => screens::diff_preview::draw(frame, app),
                Screen::AgentDetail => screens::agent_detail::draw(frame, app),
            }
            if app.show_help {
                screens::help::draw(frame);
            }
        })?;

        let Event::Key(key) = event::read()? else {
            continue;
        };

        if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
            break;
        }

        if app.show_help {
            if matches!(key.code, KeyCode::Char('?') | KeyCode::Esc) {
                app.show_help = false;
            }
            continue;
        }
        if key.code == KeyCode::Char('?') {
            app.show_help = true;
            continue;
        }

        let pending = match app.screen {
            Screen::SetupWizard => handle_wizard_key(app, key.code),
            Screen::Dashboard => {
                if handle_dashboard_key(app, key.code) {
                    break;
                }
                PendingAction::None
            }
            Screen::ItemBrowser => handle_browser_key(app, key.code),
            Screen::DiffPreview => {
                handle_diff_preview_key(app, key.code);
                PendingAction::None
            }
            Screen::AgentDetail => {
                handle_agent_detail_key(app, key.code);
                PendingAction::None
            }
        };

        if let PendingAction::EditItem {
            kind,
            name,
            initial,
            is_new,
        } = pending
        {
            let edited = suspend_for_editor(terminal, &initial)?;
            app.finish_edit(kind, name, edited, is_new);
        }
    }
    Ok(())
}

/// Leave the alternate screen / raw mode, run `$EDITOR` with normal terminal
/// control, then restore the TUI. This is the one place shaic-tui hands the
/// real terminal back to another program.
fn suspend_for_editor(terminal: &mut Term, initial: &str) -> Result<crate::Result<String>> {
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;

    let edited = shaic_core::editor::edit_in_editor(initial)
        .map_err(|e| Box::new(e) as Box<dyn std::error::Error>);

    enable_raw_mode()?;
    execute!(terminal.backend_mut(), EnterAlternateScreen)?;
    terminal.clear()?;
    Ok(edited)
}

fn handle_wizard_key(app: &mut App, code: KeyCode) -> PendingAction {
    match code {
        KeyCode::Char(c) => app.wizard.remote_input.push(c),
        KeyCode::Backspace => {
            app.wizard.remote_input.pop();
        }
        KeyCode::Enter => app.run_wizard(),
        KeyCode::Esc => {
            app.screen = Screen::Dashboard;
            app.refresh_dashboard();
        }
        _ => {}
    }
    PendingAction::None
}

/// Returns `true` if the app should quit.
fn handle_dashboard_key(app: &mut App, code: KeyCode) -> bool {
    match code {
        KeyCode::Char('q') => return true,
        KeyCode::Char('p') => app.push(),
        KeyCode::Char('u') => app.pull(),
        KeyCode::Char('r') => app.refresh_dashboard(),
        KeyCode::Char('s') => app.open_browser(),
        KeyCode::Char('i') => app.screen = Screen::SetupWizard,
        KeyCode::Up | KeyCode::Char('k') => app.move_selection(-1),
        KeyCode::Down | KeyCode::Char('j') => app.move_selection(1),
        KeyCode::Enter => app.open_agent_detail(),
        _ => {}
    }
    false
}

fn handle_browser_key(app: &mut App, code: KeyCode) -> PendingAction {
    if app.browser.name_input.is_some() {
        match code {
            KeyCode::Char(c) => {
                if let Some(name) = &mut app.browser.name_input {
                    name.push(c);
                }
            }
            KeyCode::Backspace => {
                if let Some(name) = &mut app.browser.name_input {
                    name.pop();
                }
            }
            KeyCode::Tab => app.cycle_pending_kind(),
            KeyCode::Enter => {
                if let Some(action) = app.confirm_add_name() {
                    return action;
                }
            }
            KeyCode::Esc => app.cancel_add(),
            _ => {}
        }
        return PendingAction::None;
    }

    match code {
        KeyCode::Esc => {
            app.screen = Screen::Dashboard;
            app.refresh_dashboard();
        }
        KeyCode::Up | KeyCode::Char('k') => app.move_browser_selection(-1),
        KeyCode::Down | KeyCode::Char('j') => app.move_browser_selection(1),
        KeyCode::Char('a') => app.begin_add(),
        KeyCode::Char('e') => {
            if let Some(action) = app.load_selected_for_edit() {
                return action;
            }
        }
        KeyCode::Char('d') => app.remove_selected_item(),
        _ => {}
    }
    PendingAction::None
}

fn handle_diff_preview_key(app: &mut App, code: KeyCode) {
    match code {
        KeyCode::Esc => app.return_from_diff_preview(),
        KeyCode::Char('a') => app.apply_diff_preview(),
        _ => {}
    }
}

fn handle_agent_detail_key(app: &mut App, code: KeyCode) {
    match code {
        KeyCode::Esc => {
            app.detail = None;
            app.screen = Screen::Dashboard;
            app.refresh_dashboard();
        }
        KeyCode::Up | KeyCode::Char('k') => app.move_detail_selection(-1),
        KeyCode::Down | KeyCode::Char('j') => app.move_detail_selection(1),
        KeyCode::Char('y') => {
            if let Some(detail) = &app.detail
                && let Some(sub) = detail.sub_rows.get(detail.selected_sub_row)
                && sub.content_glyph.is_some()
            {
                let (agent, scope) = (detail.agent, sub.scope);
                app.open_diff_preview(agent, scope, false);
            }
        }
        KeyCode::Char('m') => {
            if let Some(detail) = &app.detail
                && let Some(sub) = detail.sub_rows.get(detail.selected_sub_row)
                && sub.mcp_glyph.is_some()
            {
                let (agent, scope) = (detail.agent, sub.scope);
                app.open_diff_preview(agent, scope, true);
            }
        }
        _ => {}
    }
}

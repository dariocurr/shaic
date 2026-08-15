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

use app::{App, PendingAction, PendingConfirm, Screen};

/// Dependency-free stand-in for `anyhow::Result`: this crate's failures are
/// all one-shot terminal/IO errors that just need to surface to the caller,
/// not be matched on, so a boxed trait object is enough.
pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

/// Restores the terminal even if the TUI panics or returns `Err`.
struct TerminalGuard {
    active: bool,
}

impl TerminalGuard {
    fn enter() -> Result<Self> {
        install_panic_hook();
        enable_raw_mode()?;
        execute!(std::io::stdout(), EnterAlternateScreen)?;
        Ok(Self { active: true })
    }

    fn suspend(&mut self) -> Result<()> {
        if self.active {
            restore_terminal();
            self.active = false;
        }
        Ok(())
    }

    fn resume(&mut self) -> Result<()> {
        if !self.active {
            enable_raw_mode()?;
            execute!(std::io::stdout(), EnterAlternateScreen)?;
            self.active = true;
        }
        Ok(())
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        restore_terminal();
        self.active = false;
    }
}

fn restore_terminal() {
    let _ = disable_raw_mode();
    let _ = execute!(
        std::io::stdout(),
        LeaveAlternateScreen,
        crossterm::cursor::Show
    );
}

fn install_panic_hook() {
    use std::sync::Once;
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let prev = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            restore_terminal();
            prev(info);
        }));
    });
}

/// Entry point used by the `shaic tui` CLI command (and the default action
/// when `shaic` is run with no subcommand in an interactive terminal).
pub fn run() -> Result<()> {
    let mut guard = TerminalGuard::enter()?;
    let backend = CrosstermBackend::new(std::io::stdout());
    let mut terminal = Terminal::new(backend)?;
    terminal.hide_cursor()?;

    let mut app = App::new()?;
    let result = event_loop(&mut terminal, &mut app, &mut guard);
    drop(guard);
    result
}

type Term = Terminal<CrosstermBackend<std::io::Stdout>>;

fn event_loop(terminal: &mut Term, app: &mut App, guard: &mut TerminalGuard) -> Result<()> {
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

        if app.pending_confirm.is_some() {
            match key.code {
                KeyCode::Char('y') | KeyCode::Char('Y') | KeyCode::Enter => {
                    if let Some(action) = app.take_confirm() {
                        match action {
                            PendingConfirm::DeleteItem => app.remove_selected_item(),
                            PendingConfirm::Push => app.push(),
                            PendingConfirm::Pull => app.pull(),
                            PendingConfirm::ApplyDiff => app.apply_diff_preview(),
                            PendingConfirm::ImportScope => app.import_selected_scope(),
                        }
                    }
                }
                KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => app.cancel_confirm(),
                _ => {}
            }
            continue;
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
            let edited = suspend_for_editor(terminal, guard, &initial)?;
            app.finish_edit(kind, name, edited, is_new);
        }
    }
    Ok(())
}

/// Leave the alternate screen / raw mode, run `$EDITOR` with normal terminal
/// control, then restore the TUI. This is the one place shaic-tui hands the
/// real terminal back to another program.
fn suspend_for_editor(
    terminal: &mut Term,
    guard: &mut TerminalGuard,
    initial: &str,
) -> Result<crate::Result<String>> {
    guard.suspend()?;
    terminal.show_cursor()?;

    let edited = shaic_core::editor::edit_in_editor(initial)
        .map_err(|e| Box::new(e) as Box<dyn std::error::Error>);

    guard.resume()?;
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
        KeyCode::Char('p') => app.request_confirm(PendingConfirm::Push),
        KeyCode::Char('u') => app.request_confirm(PendingConfirm::Pull),
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
        KeyCode::Char('d') => app.request_confirm(PendingConfirm::DeleteItem),
        _ => {}
    }
    PendingAction::None
}

fn handle_diff_preview_key(app: &mut App, code: KeyCode) {
    match code {
        KeyCode::Esc => app.return_from_diff_preview(),
        KeyCode::Char('a') => app.request_confirm(PendingConfirm::ApplyDiff),
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
        KeyCode::Char('o') => app.request_confirm(PendingConfirm::ImportScope),
        _ => {}
    }
}

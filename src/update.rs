//! Update step: translates input into state mutations.
//!
//! This is the intended home for future keybinding logic — mapping both
//! vim-style (`j`/`k`/`h`/`l`) and non-vim (arrow keys) input to the same
//! actions on [`App`], so both interaction styles are supported
//! simultaneously.

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::app::App;

pub fn update(app: &mut App, key_event: KeyEvent) {
    match key_event.code {
        KeyCode::Char('c') | KeyCode::Char('C') if key_event.modifiers == KeyModifiers::CONTROL => {
            app.quit()
        }
        KeyCode::Char('q') => app.quit(),
        KeyCode::Esc => {
            if app.scanner.is_some() || app.scanner_error.is_some() {
                app.go_back();
            } else {
                app.quit();
            }
        }
        KeyCode::Char('h') | KeyCode::Left | KeyCode::Backspace => {
            if app.scanner.is_some() || app.scanner_error.is_some() {
                app.go_back();
            }
        }
        KeyCode::Char('j') | KeyCode::Down => match &mut app.scanner {
            Some(scanner) if scanner.finished => scanner.select_next(),
            Some(_) => {}
            None => app.select_next(),
        },
        KeyCode::Char('k') | KeyCode::Up => match &mut app.scanner {
            Some(scanner) if scanner.finished => scanner.select_previous(),
            Some(_) => {}
            None => app.select_previous(),
        },
        KeyCode::Enter | KeyCode::Char('l') | KeyCode::Right => app.enter_selected(),
        KeyCode::Char('s') => app.start_scan(),
        _ => {}
    };
}

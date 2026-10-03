//! Update step: translates input into state mutations.
//!
//! This is the intended home for future keybinding logic — mapping both
//! vim-style (`j`/`k`/`h`/`l`) and non-vim (arrow keys) input to the same
//! actions on [`App`], so both interaction styles are supported
//! simultaneously.

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::app::{App, Choice};

pub fn update(app: &mut App, key_event: KeyEvent) {
    // A notice is dismissed by any key.
    if app.notice.take().is_some() {
        return;
    }

    if let Some(request) = &mut app.delete_confirmation {
        match key_event.code {
            KeyCode::Left | KeyCode::Char('h') => request.choice = Choice::Yes,
            KeyCode::Right | KeyCode::Char('l') => request.choice = Choice::No,
            KeyCode::Enter => app.confirm_delete(),
            KeyCode::Esc | KeyCode::Backspace => app.delete_confirmation = None,
            _ => {}
        }
        return;
    }

    if let Some(selected) = app.quit_confirmation {
        match key_event.code {
            KeyCode::Left | KeyCode::Char('h') => {
                app.quit_confirmation = Some(Choice::Yes);
            }
            KeyCode::Right | KeyCode::Char('l') => {
                app.quit_confirmation = Some(Choice::No);
            }
            KeyCode::Enter => {
                if selected == Choice::Yes {
                    app.quit();
                } else {
                    app.quit_confirmation = None;
                }
            }
            KeyCode::Esc | KeyCode::Backspace => {
                app.quit_confirmation = None;
            }
            _ => {}
        }
        return;
    }

    match key_event.code {
        KeyCode::Char('c') | KeyCode::Char('C') if key_event.modifiers == KeyModifiers::CONTROL => {
            app.quit()
        }
        KeyCode::Char('q') => {
            app.quit_confirmation = Some(Choice::No);
        }
        KeyCode::Esc => {
            if app.scanner.is_some() || app.scanner_error.is_some() {
                app.go_back();
            } else {
                app.quit_confirmation = Some(Choice::No);
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
        KeyCode::Char('d') | KeyCode::Delete => app.request_delete(),
        _ => {}
    };
}

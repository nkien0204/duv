//! Update step: translates input into state mutations.
//!
//! This is the intended home for future keybinding logic — mapping both
//! vim-style (`j`/`k`/`gg`/`G`) and non-vim (arrow keys, `Home`/`End`) input
//! to the same actions on [`App`], so both interaction styles are
//! supported simultaneously.

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::app::App;

pub fn update(app: &mut App, key_event: KeyEvent) {
    match key_event.code {
        KeyCode::Esc | KeyCode::Char('q') => app.quit(),
        KeyCode::Char('c') | KeyCode::Char('C') if key_event.modifiers == KeyModifiers::CONTROL => {
            app.quit()
        }
        KeyCode::Char('j') | KeyCode::Down => app.select_next(),
        KeyCode::Char('k') | KeyCode::Up => app.select_previous(),
        _ => {}
    };
}

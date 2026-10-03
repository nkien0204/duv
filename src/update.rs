//! Update step: translates input into state mutations.
//!
//! This is the home for keybinding logic — mapping both vim-style
//! (`j`/`k`/`h`/`l`, `gg`/`G`, `Ctrl+d`/`Ctrl+u`) and non-vim (arrow keys,
//! `Home`/`End`, `PgUp`/`PgDn`) input to the same actions on [`App`], so
//! both interaction styles are supported simultaneously.

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::app::{App, Choice, Jump};

pub fn update(app: &mut App, key_event: KeyEvent) {
    // `gg` needs the previous key; any other key in between cancels it.
    let pending_g = std::mem::take(&mut app.pending_g);

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
        // Checked before plain `d` (delete) and `u`.
        KeyCode::Char('d') if key_event.modifiers == KeyModifiers::CONTROL => {
            app.jump(Jump::HalfPageDown)
        }
        KeyCode::Char('u') if key_event.modifiers == KeyModifiers::CONTROL => {
            app.jump(Jump::HalfPageUp)
        }
        KeyCode::Char('g') if pending_g => app.jump(Jump::First),
        KeyCode::Char('g') => app.pending_g = true,
        KeyCode::Char('G') | KeyCode::End => app.jump(Jump::Last),
        KeyCode::Home => app.jump(Jump::First),
        KeyCode::PageDown => app.jump(Jump::PageDown),
        KeyCode::PageUp => app.jump(Jump::PageUp),
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::disks::{DiskInfo, DiskKind};
    use crate::scanner::DEFAULT_MEMORY_BUDGET;
    use std::{
        fs,
        path::PathBuf,
        thread,
        time::{Duration, Instant},
    };

    fn press(app: &mut App, code: KeyCode) {
        update(app, KeyEvent::new(code, KeyModifiers::NONE));
    }

    fn ctrl(app: &mut App, c: char) {
        update(app, KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL));
    }

    fn scan_selected(app: &App) -> usize {
        app.scanner.as_ref().unwrap().selected
    }

    /// An app showing the finished scan of a folder with 30 files.
    fn scanned_app(name: &str) -> (App, PathBuf) {
        let dir = std::env::temp_dir().join(format!("duv-update-{name}-{}", std::process::id()));
        fs::remove_dir_all(&dir).ok();
        fs::create_dir_all(&dir).unwrap();
        for i in 0..30 {
            fs::write(dir.join(format!("{i:02}.txt")), b"x").unwrap();
        }
        let mut app = App::with_start_path(dir.clone(), DEFAULT_MEMORY_BUDGET);
        let deadline = Instant::now() + Duration::from_secs(5);
        while !app.scanner.as_ref().unwrap().finished && Instant::now() < deadline {
            app.tick();
            thread::sleep(Duration::from_millis(10));
        }
        assert!(app.scanner.as_ref().unwrap().finished);
        app.page_size = 10;
        (app, dir)
    }

    #[test]
    fn jumps_to_ends_and_by_pages_in_scan_results() {
        let (mut app, dir) = scanned_app("jumps");

        press(&mut app, KeyCode::Char('G'));
        assert_eq!(scan_selected(&app), 29);
        press(&mut app, KeyCode::Home);
        assert_eq!(scan_selected(&app), 0);
        press(&mut app, KeyCode::End);
        assert_eq!(scan_selected(&app), 29);
        press(&mut app, KeyCode::Char('g'));
        press(&mut app, KeyCode::Char('g'));
        assert_eq!(scan_selected(&app), 0);

        // Pages stop at the ends instead of wrapping.
        press(&mut app, KeyCode::PageDown);
        assert_eq!(scan_selected(&app), 10);
        press(&mut app, KeyCode::PageDown);
        press(&mut app, KeyCode::PageDown);
        assert_eq!(scan_selected(&app), 29);
        press(&mut app, KeyCode::PageUp);
        assert_eq!(scan_selected(&app), 19);

        // Half pages.
        ctrl(&mut app, 'u');
        assert_eq!(scan_selected(&app), 14);
        ctrl(&mut app, 'd');
        assert_eq!(scan_selected(&app), 19);
        assert!(
            app.delete_confirmation.is_none(),
            "Ctrl+d moves, it doesn't delete"
        );

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn gg_needs_two_g_presses_in_a_row() {
        let (mut app, dir) = scanned_app("gg");
        press(&mut app, KeyCode::End);

        press(&mut app, KeyCode::Char('g'));
        assert_eq!(scan_selected(&app), 29, "a single g does nothing yet");
        press(&mut app, KeyCode::Char('k'));
        press(&mut app, KeyCode::Char('g'));
        assert_eq!(scan_selected(&app), 28, "another key in between cancels gg");
        press(&mut app, KeyCode::Char('g'));
        assert_eq!(scan_selected(&app), 0);

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn jumps_work_on_the_disk_list() {
        let mut app = App::new(DEFAULT_MEMORY_BUDGET);
        app.disks = (0..25)
            .map(|i| DiskInfo {
                name: format!("disk{i}"),
                mount_point: format!("/mnt/{i}"),
                file_system: "apfs".to_string(),
                total_space: 100,
                available_space: 50,
                is_removable: false,
                kind: DiskKind::Ssd,
            })
            .collect();
        app.page_size = 10;

        press(&mut app, KeyCode::Char('G'));
        assert_eq!(app.selected, 24);
        assert_eq!(app.disks_table_state.selected(), Some(24));
        press(&mut app, KeyCode::Char('g'));
        press(&mut app, KeyCode::Char('g'));
        assert_eq!(app.selected, 0);
        press(&mut app, KeyCode::PageDown);
        assert_eq!(app.selected, 10);
        ctrl(&mut app, 'u');
        assert_eq!(app.selected, 5);
    }

    #[test]
    fn jumps_are_ignored_while_scanning() {
        let dir = std::env::temp_dir().join(format!("duv-update-busy-{}", std::process::id()));
        fs::create_dir_all(dir.join("sub")).unwrap();
        fs::write(dir.join("a.txt"), b"x").unwrap();
        let mut app = App::with_start_path(dir.clone(), DEFAULT_MEMORY_BUDGET);
        assert!(!app.scanner.as_ref().unwrap().finished);

        press(&mut app, KeyCode::End);
        assert_eq!(scan_selected(&app), 0);

        fs::remove_dir_all(&dir).ok();
    }
}

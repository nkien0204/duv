/// Application.
pub mod app;

/// Command-line argument parsing.
pub mod cli;

/// Disk/volume enumeration.
pub mod disks;

/// Terminal events handler.
pub mod event;

/// In-memory tree of scanned paths and sizes.
pub mod model;

/// Headless scan statistics (`--stats`).
pub mod stats;

/// Directory-size scanning and in-scan navigation.
pub mod scanner;

/// Widget renderer.
pub mod ui;

/// Terminal user interface.
pub mod tui;

/// Application updater.
pub mod update;

use anyhow::Result;
use app::App;
use clap::Parser;
use cli::Cli;
use event::{Event, EventHandler};
use ratatui::{Terminal, backend::CrosstermBackend};
use tui::Tui;
use update::update;

fn main() -> Result<()> {
    // Parse and validate arguments before touching the terminal.
    let cli = Cli::parse();
    let start_path = cli.start_path().map_err(anyhow::Error::msg)?;

    if cli.stats
        && let Some(path) = start_path
    {
        return stats::run(path, cli.memory_budget_bytes());
    }

    // Create an application.
    let mut app = match start_path {
        Some(path) => App::with_start_path(path, cli.memory_budget_bytes()),
        None => App::new(cli.memory_budget_bytes()),
    };

    // Initialize the terminal user interface.
    let backend = CrosstermBackend::new(std::io::stderr());
    let terminal = Terminal::new(backend)?;
    let events = EventHandler::new(250);
    let mut tui = Tui::new(terminal, events);
    tui.enter()?;

    // Start the main loop.
    while !app.should_quit {
        // Render the user interface.
        tui.draw(&mut app)?;
        // Handle events.
        match tui.events.next()? {
            Event::Tick => app.tick(),
            Event::Key(key_event) => update(&mut app, key_event),
            Event::Mouse(_) => {}
            Event::Resize(_, _) => {}
        };
    }

    // Exit the user interface.
    tui.exit()?;
    Ok(())
}

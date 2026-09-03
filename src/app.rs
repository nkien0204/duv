//! Application state (the "Model" in this crate's Model-Update-View split).
//!
//! `App` holds all state and has no dependency on ratatui's rendering types
//! or crossterm's event types — it doesn't know how it's drawn or how input
//! arrives. See `update.rs` for how input mutates this state and `ui.rs` for
//! how it's rendered. See `../ARCHITECTURE.md` for the full picture.

#[derive(Default)]
pub struct App {
    pub should_quit: bool,
}

impl App {
    /// Constructs a new instance of [`App`].
    pub fn new() -> Self {
        Self::default()
    }

    /// Handles the tick event of the terminal.
    pub fn tick(&self) {}

    /// Set should_quit to true to quit the application.
    pub fn quit(&mut self) {
        self.should_quit = true;
    }
}

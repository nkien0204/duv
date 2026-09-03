//! View step: pure rendering of [`App`] state to a ratatui [`Frame`].
//!
//! Kept as a free function (rather than `impl Widget for &App`) so `app.rs`
//! stays fully decoupled from ratatui's rendering types.

use ratatui::{
    Frame,
    layout::Alignment,
    style::{Color, Style},
    widgets::{Block, BorderType, Borders, Paragraph},
};

use crate::app::App;

pub fn render(_app: &mut App, frame: &mut Frame) {
    frame.render_widget(
        Paragraph::new("duv is scanning nothing yet. Press q to quit.")
            .block(
                Block::default()
                    .title("DUV")
                    .title_alignment(Alignment::Center)
                    .borders(Borders::ALL)
                    .border_type(BorderType::Rounded),
            )
            .style(Style::default().fg(Color::Yellow))
            .alignment(Alignment::Center),
        frame.area(),
    )
}

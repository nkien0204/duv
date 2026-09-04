//! View step: pure rendering of [`App`] state to a ratatui [`Frame`].
//!
//! Kept as a free function (rather than `impl Widget for &App`) so `app.rs`
//! stays fully decoupled from ratatui's rendering types.

use ratatui::{
    Frame,
    layout::{Alignment, Constraint},
    style::{Color, Modifier, Style},
    text::Line,
    widgets::{Block, BorderType, Borders, Cell, Row, Table},
};

use crate::app::App;
use crate::disks::format_bytes;

pub fn render(app: &mut App, frame: &mut Frame) {
    let block = Block::default()
        .title("DUV — disk usage visualizer (j/k or ↑/↓ to move, q to quit)")
        .title_alignment(Alignment::Center)
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded);

    if app.disks.is_empty() {
        frame.render_widget(
            ratatui::widgets::Paragraph::new("No disks found.")
                .block(block)
                .style(Style::default().fg(Color::Yellow))
                .alignment(Alignment::Center),
            frame.area(),
        );
        return;
    }

    let header = Row::new(vec![
        Cell::from("Name"),
        Cell::from("Mount point"),
        Cell::from("Type"),
        Cell::from("Filesystem"),
        Cell::from("Used"),
        Cell::from("Available"),
        Cell::from("Total"),
    ])
    .style(Style::default().add_modifier(Modifier::BOLD));

    let rows = app.disks.iter().enumerate().map(|(i, disk)| {
        let style = if i == app.selected {
            Style::default()
                .fg(Color::Black)
                .bg(Color::Yellow)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default()
        };
        let name = if disk.name.is_empty() {
            disk.mount_point.clone()
        } else {
            disk.name.clone()
        };
        Row::new(vec![
            Cell::from(name),
            Cell::from(disk.mount_point.clone()),
            Cell::from(Line::from(disk.kind.to_string())),
            Cell::from(disk.file_system.clone()),
            Cell::from(format_bytes(disk.used_space())),
            Cell::from(format_bytes(disk.available_space)),
            Cell::from(format_bytes(disk.total_space)),
        ])
        .style(style)
    });

    let table = Table::new(
        rows,
        [
            Constraint::Percentage(15),
            Constraint::Percentage(20),
            Constraint::Percentage(10),
            Constraint::Percentage(10),
            Constraint::Percentage(15),
            Constraint::Percentage(15),
            Constraint::Percentage(15),
        ],
    )
    .header(header)
    .block(block);

    frame.render_widget(table, frame.area());
}

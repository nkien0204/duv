//! View step: pure rendering of [`App`] state to a ratatui [`Frame`].
//!
//! Kept as a free function (rather than `impl Widget for &App`) so `app.rs`
//! stays fully decoupled from ratatui's rendering types.

use ratatui::{
    Frame,
    layout::{Alignment, Constraint},
    style::{Color, Modifier, Style},
    text::Line,
    widgets::{Block, BorderType, Borders, Cell, Gauge, Paragraph, Row, Table},
};

use crate::app::App;
use crate::disks::format_bytes;

pub fn render(app: &mut App, frame: &mut Frame) {
    if let Some(error) = app.scanner_error.clone() {
        render_error(&error, frame);
        return;
    }
    if app.scanner.is_some() {
        render_scan(app, frame);
        return;
    }
    render_disks(app, frame);
}

fn render_error(error: &str, frame: &mut Frame) {
    let block = Block::default()
        .title("DUV — error (Esc/h to go back, q to quit)")
        .title_alignment(Alignment::Center)
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded);
    frame.render_widget(
        Paragraph::new(error.to_string())
            .block(block)
            .style(Style::default().fg(Color::Red))
            .alignment(Alignment::Center),
        frame.area(),
    );
}

fn render_disks(app: &mut App, frame: &mut Frame) {
    let block = Block::default()
        .title("DUV — disk usage visualizer (j/k or ↑/↓ to move, s to scan, q to quit)")
        .title_alignment(Alignment::Center)
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded);

    if app.disks.is_empty() {
        frame.render_widget(
            Paragraph::new("No disks found.")
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

    frame.render_stateful_widget(table, frame.area(), &mut app.disks_table_state);
}

fn render_scan(app: &mut App, frame: &mut Frame) {
    let Some(scanner) = &mut app.scanner else {
        return;
    };
    let root = scanner.root.display().to_string();

    if !scanner.finished {
        let block = Block::default()
            .title(format!("DUV — scanning {root} (Esc to cancel)"))
            .title_alignment(Alignment::Center)
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded);
        let ratio = scanner.progress_fraction().clamp(0.0, 1.0);
        let label = format!("{}/{} entries", scanner.entries.len(), scanner.total);
        let gauge = Gauge::default()
            .block(block)
            .gauge_style(Style::default().fg(Color::Yellow))
            .ratio(ratio)
            .label(label);
        frame.render_widget(gauge, frame.area());
        return;
    }

    let block = Block::default()
        .title(format!(
            "DUV — {root} (l/Enter to open, h/Esc to go back, s to rescan, q to quit)"
        ))
        .title_alignment(Alignment::Center)
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded);

    if scanner.entries.is_empty() {
        frame.render_widget(
            Paragraph::new("Empty directory.")
                .block(block)
                .style(Style::default().fg(Color::Yellow))
                .alignment(Alignment::Center),
            frame.area(),
        );
        return;
    }

    let header = Row::new(vec![
        Cell::from("Name"),
        Cell::from("Type"),
        Cell::from("Size"),
    ])
    .style(Style::default().add_modifier(Modifier::BOLD));

    let rows = scanner.entries.iter().enumerate().map(|(i, entry)| {
        let style = if i == scanner.selected {
            Style::default()
                .fg(Color::Black)
                .bg(Color::Yellow)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default()
        };
        Row::new(vec![
            Cell::from(entry.name.clone()),
            Cell::from(if entry.is_dir { "Dir" } else { "File" }),
            Cell::from(format_bytes(entry.size)),
        ])
        .style(style)
    });

    let table = Table::new(
        rows,
        [
            Constraint::Percentage(60),
            Constraint::Percentage(15),
            Constraint::Percentage(25),
        ],
    )
    .header(header)
    .block(block);

    frame.render_stateful_widget(table, frame.area(), &mut scanner.table_state);
}

//! View step: pure rendering of [`App`] state to a ratatui [`Frame`].
//!
//! Kept as a free function (rather than `impl Widget for &App`) so `app.rs`
//! stays fully decoupled from ratatui's rendering types.

use ratatui::{
    Frame,
    layout::{Alignment, Constraint},
    style::{Color, Modifier, Style},
    text::Line,
    widgets::{Block, BorderType, Borders, Cell, Clear, Gauge, Paragraph, Row, Table},
};

use crate::app::App;
use crate::disks::format_bytes;

pub fn render(app: &mut App, frame: &mut Frame) {
    if let Some(error) = app.scanner_error.clone() {
        render_error(&error, frame);
    } else if app.scanner.is_some() {
        render_scan(app, frame);
    } else {
        render_disks(app, frame);
    }

    if app.quit_confirmation.is_some() {
        render_quit_confirmation(app, frame);
    }
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
    let root = scanner.current_path().display().to_string();
    let is_scanning = !scanner.finished;

    let mut block = Block::default()
        .title(format!(
            "DUV — {root} (l/Enter to open, h/Esc to go back, s to rescan, q to quit)"
        ))
        .title_alignment(Alignment::Center)
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded);

    if is_scanning {
        block = block.border_style(Style::default().fg(Color::DarkGray));
    }

    if scanner.entry_count() == 0 {
        frame.render_widget(
            Paragraph::new("Empty directory.")
                .block(block)
                .style(if is_scanning {
                    Style::default().fg(Color::DarkGray)
                } else {
                    Style::default().fg(Color::Yellow)
                })
                .alignment(Alignment::Center),
            frame.area(),
        );
    } else {
        let header_style = if is_scanning {
            Style::default().fg(Color::DarkGray)
        } else {
            Style::default().add_modifier(Modifier::BOLD)
        };

        let header = Row::new(vec![
            Cell::from("Name"),
            Cell::from("Type"),
            Cell::from("Size"),
        ])
        .style(header_style);

        let selected = scanner.selected;
        let rows = scanner.entries().enumerate().map(|(i, entry)| {
            let style = if is_scanning {
                Style::default().fg(Color::DarkGray)
            } else if i == selected {
                Style::default()
                    .fg(Color::Black)
                    .bg(Color::Yellow)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };
            Row::new(vec![
                Cell::from(entry.name.to_string()),
                Cell::from(if entry.is_dir() { "Dir" } else { "File" }),
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

    if is_scanning {
        let bar_width = (frame.area().width as f32 * 0.5).max(40.0) as u16;
        let bar_height = 3;

        let x = (frame.area().width.saturating_sub(bar_width)) / 2;
        let y = frame.area().height.saturating_sub(bar_height + 2); // Bottom-ish
        let area = ratatui::layout::Rect::new(x, y, bar_width, bar_height);

        let ratio = scanner.progress_fraction().clamp(0.0, 1.0);
        let label = format!("{}/{} entries", scanner.measured, scanner.total);
        let gauge = Gauge::default()
            .gauge_style(Style::default().fg(Color::Yellow))
            .ratio(ratio)
            .label(label)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_type(BorderType::Rounded),
            );

        frame.render_widget(Clear, area);
        frame.render_widget(gauge, area);
    }
}

fn render_quit_confirmation(app: &App, frame: &mut Frame) {
    use ratatui::layout::Rect;

    let area = frame.area();
    let popup_width = 40;
    let popup_height = 5;
    let x = (area.width.saturating_sub(popup_width)) / 2;
    let y = (area.height.saturating_sub(popup_height)) / 2;
    let popup_area = Rect::new(x, y, popup_width, popup_height);

    let block = Block::default()
        .title(" Quit ")
        .title_alignment(Alignment::Center)
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded);

    let yes_style = if app.quit_confirmation == Some(crate::app::QuitOption::Yes) {
        Style::default()
            .bg(Color::Yellow)
            .fg(Color::Black)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default()
    };
    let no_style = if app.quit_confirmation == Some(crate::app::QuitOption::No) {
        Style::default()
            .bg(Color::Yellow)
            .fg(Color::Black)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default()
    };

    let text = vec![
        Line::from(vec![ratatui::text::Span::styled(
            " Are you sure you want to quit? ",
            Style::default().add_modifier(Modifier::BOLD),
        )]),
        Line::from(""),
        Line::from(vec![
            ratatui::text::Span::styled(" Yes ", yes_style),
            ratatui::text::Span::raw("   "),
            ratatui::text::Span::styled(" No ", no_style),
        ]),
    ];

    frame.render_widget(Clear, popup_area);
    frame.render_widget(
        Paragraph::new(text)
            .block(block)
            .alignment(Alignment::Center),
        popup_area,
    );
}

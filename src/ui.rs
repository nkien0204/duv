//! View step: pure rendering of [`App`] state to a ratatui [`Frame`].
//!
//! Kept as a free function (rather than `impl Widget for &App`) so `app.rs`
//! stays fully decoupled from ratatui's rendering types.

use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Cell, Clear, Gauge, Paragraph, Row, Table, Wrap},
};

use crate::app::{App, Choice, DeleteRequest};
use crate::disks::format_bytes;

pub fn render(app: &mut App, frame: &mut Frame) {
    // Both tables fill the screen: rows minus the border and header.
    app.page_size = usize::from(frame.area().height.saturating_sub(3)).max(1);

    if let Some(error) = app.scanner_error.clone() {
        render_error(&error, frame);
    } else if app.scanner.is_some() {
        render_scan(app, frame);
    } else {
        render_disks(app, frame);
    }

    if let Some(request) = &app.delete_confirmation {
        render_delete_confirmation(request, frame);
    }
    if app.quit_confirmation.is_some() {
        render_quit_confirmation(app, frame);
    }
    if let Some(notice) = &app.notice {
        render_notice(notice, frame);
    }
}

fn render_error(error: &str, frame: &mut Frame) {
    let block = Block::default()
        .title(" DUV — error ")
        .title_bottom(key_hints(&[("h", "back"), ("q", "quit")]))
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
        .title(" DUV — disk usage visualizer ")
        .title_bottom(match app.status.as_deref() {
            Some(status) => status_line(status),
            None => key_hints(&[
                ("j/k", "move"),
                ("s", "scan"),
                ("o", "reveal"),
                ("y", "copy"),
                ("q", "quit"),
            ]),
        })
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
    let typing_filter = app.filter_input;
    let status = app.status.clone();
    let status = status.as_deref();
    let Some(scanner) = &mut app.scanner else {
        return;
    };
    let root = scanner.current_path().display().to_string();
    let is_scanning = !scanner.finished;

    let mut block = Block::default()
        .title(format!(" DUV — {root} "))
        .title_alignment(Alignment::Center)
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded);

    if is_scanning {
        block = block.border_style(Style::default().fg(Color::DarkGray));
    }
    // The bottom border shows, in order of priority: a status message, the
    // filter while one is set, or key hints.
    block = block.title_bottom(match (status, scanner.filter_query()) {
        (Some(status), _) => status_line(status),
        (None, Some(query)) => filter_line(
            query,
            typing_filter,
            scanner.entry_count(),
            scanner.unfiltered_count(),
        ),
        (None, None) => key_hints(&[
            ("l", "open"),
            ("h", "back"),
            ("/", "filter"),
            ("d", "trash"),
            ("o", "reveal"),
            ("y", "copy"),
            ("s", "rescan"),
            ("q", "quit"),
        ]),
    });

    if scanner.entry_count() == 0 {
        let message = match scanner.filter_query() {
            Some(query) if scanner.unfiltered_count() > 0 => {
                format!("No entries match \"{query}\".")
            }
            _ => "Empty directory.".to_string(),
        };
        frame.render_widget(
            Paragraph::new(message)
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
        let label = format!("{}/{} folders", scanner.measured, scanner.total);
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

/// A left-aligned bottom border line of `(key, action)` hints, e.g.
/// "l open · h back · q quit".
fn key_hints(hints: &[(&str, &str)]) -> Line<'static> {
    let key = Style::default()
        .fg(Color::Yellow)
        .add_modifier(Modifier::BOLD);
    let mut spans = vec![Span::raw(" ")];
    for (i, (keys, action)) in hints.iter().enumerate() {
        if i > 0 {
            spans.push(Span::styled(" · ", Style::default().fg(Color::DarkGray)));
        }
        spans.push(Span::styled(keys.to_string(), key));
        spans.push(Span::raw(format!(" {action}")));
    }
    spans.push(Span::raw(" "));
    Line::from(spans).left_aligned()
}

/// A bottom border line confirming an action (e.g. "Copied /path").
fn status_line(status: &str) -> Line<'static> {
    Line::from(Span::styled(
        format!(" {status} "),
        Style::default()
            .fg(Color::Green)
            .add_modifier(Modifier::BOLD),
    ))
    .left_aligned()
}

/// The bottom border line for a name filter: the query being typed (with
/// a cursor) or applied, how many entries match, and the keys that matter.
fn filter_line(query: &str, typing: bool, matches: usize, total: usize) -> Line<'static> {
    let accent = Style::default()
        .fg(Color::Yellow)
        .add_modifier(Modifier::BOLD);
    let dim = Style::default().fg(Color::DarkGray);
    let (query_text, hint) = if typing {
        (format!(" /{query}▏"), "Enter to keep, Esc to cancel ")
    } else {
        (format!(" filter: {query}"), "/ to edit, Esc to clear ")
    };
    Line::from(vec![
        Span::styled(query_text, accent),
        Span::styled(format!("  {matches} of {total}  "), dim),
        Span::styled(hint, dim),
    ])
}

fn render_quit_confirmation(app: &App, frame: &mut Frame) {
    let Some(choice) = app.quit_confirmation else {
        return;
    };
    let message = vec![Line::from(Span::styled(
        " Are you sure you want to quit? ",
        Style::default().add_modifier(Modifier::BOLD),
    ))];
    render_confirmation(frame, " Quit ", message, choice);
}

fn render_delete_confirmation(request: &DeleteRequest, frame: &mut Frame) {
    let message = vec![Line::from(Span::styled(
        format!(" Move {} to the Trash? ", request.name),
        Style::default().add_modifier(Modifier::BOLD),
    ))];
    render_confirmation(frame, " Delete ", message, request.choice);
}

/// Minimum width of Yes/No popups, in columns.
const CONFIRMATION_WIDTH: u16 = 40;

/// A centered Yes/No popup: `message` lines, a blank line, then the two
/// buttons with `choice` highlighted.
fn render_confirmation(frame: &mut Frame, title: &str, mut message: Vec<Line>, choice: Choice) {
    let highlighted = Style::default()
        .bg(Color::Yellow)
        .fg(Color::Black)
        .add_modifier(Modifier::BOLD);
    let style = |button| {
        if choice == button {
            highlighted
        } else {
            Style::default()
        }
    };
    message.push(Line::from(""));
    message.push(Line::from(vec![
        Span::styled(" Yes ", style(Choice::Yes)),
        Span::raw("   "),
        Span::styled(" No ", style(Choice::No)),
    ]));

    let widest = message.iter().map(Line::width).max().unwrap_or(0) as u16;
    // Same width for every confirmation, widening only for long names.
    let width = (widest + 4).max(CONFIRMATION_WIDTH);
    let area = popup_area(frame.area(), width, message.len() as u16 + 2);
    let block = Block::default()
        .title(title.to_string())
        .title_alignment(Alignment::Center)
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded);

    frame.render_widget(Clear, area);
    frame.render_widget(
        Paragraph::new(message)
            .block(block)
            .alignment(Alignment::Center),
        area,
    );
}

/// A centered popup showing `notice` until the next key press.
fn render_notice(notice: &str, frame: &mut Frame) {
    let width = frame.area().width.saturating_sub(4).clamp(20, 70);
    let text_width = (width - 4).max(1) as usize;
    let text_lines = notice.chars().count().div_ceil(text_width).max(1) as u16;
    let area = popup_area(frame.area(), width, text_lines + 4);
    let block = Block::default()
        .title(" Error ")
        .title_alignment(Alignment::Center)
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(Color::Red));
    let text = vec![
        Line::from(Span::styled(
            notice.to_string(),
            Style::default().fg(Color::Red),
        )),
        Line::from(""),
        Line::from(Span::styled(
            "Press any key to close",
            Style::default().fg(Color::DarkGray),
        )),
    ];

    frame.render_widget(Clear, area);
    frame.render_widget(
        Paragraph::new(text)
            .block(block)
            .alignment(Alignment::Center)
            .wrap(Wrap { trim: true }),
        area,
    );
}

/// A `width` x `height` rectangle centered in `area`, shrunk to fit.
fn popup_area(area: Rect, width: u16, height: u16) -> Rect {
    let width = width.min(area.width);
    let height = height.min(area.height);
    Rect::new(
        area.x + (area.width - width) / 2,
        area.y + (area.height - height) / 2,
        width,
        height,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::DeleteRequest;
    use ratatui::{Terminal, backend::TestBackend};

    /// Width of the popup drawn over a blank screen: the span between the
    /// rounded top corners on the popup's first row.
    fn popup_width(draw: impl FnOnce(&mut Frame)) -> usize {
        let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
        terminal.draw(draw).unwrap();
        let buffer = terminal.backend().buffer();
        let width = buffer.area.width;
        for y in 0..buffer.area.height {
            let row: Vec<&str> = (0..width).map(|x| buffer[(x, y)].symbol()).collect();
            if let (Some(left), Some(right)) = (
                row.iter().position(|&s| s == "╭"),
                row.iter().position(|&s| s == "╮"),
            ) {
                return right - left + 1;
            }
        }
        panic!("no popup drawn");
    }

    fn delete_request(name: &str) -> DeleteRequest {
        DeleteRequest {
            path: format!("/tmp/{name}").into(),
            name: name.to_string(),
            size: 4096,
            is_dir: false,
            choice: Choice::No,
        }
    }

    #[test]
    fn delete_popup_is_as_wide_as_quit_popup() {
        let quit = popup_width(|frame| {
            let message = vec![Line::from(" Are you sure you want to quit? ")];
            render_confirmation(frame, " Quit ", message, Choice::No);
        });
        let delete =
            popup_width(|frame| render_delete_confirmation(&delete_request("a.txt"), frame));
        assert_eq!(quit, CONFIRMATION_WIDTH as usize);
        assert_eq!(delete, quit);
    }

    #[test]
    fn delete_popup_widens_for_long_names() {
        let name = "a-really-long-file-name-that-does-not-fit-in-forty-columns.tar.gz";
        let width = popup_width(|frame| render_delete_confirmation(&delete_request(name), frame));
        let question = format!(" Move {name} to the Trash? ");
        assert_eq!(width, question.len() + 4);
    }
}

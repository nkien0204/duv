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
use crate::scanner::SortBy;

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
        render_delete_confirmation(request, &app.trash_name, frame);
    }
    if app.quit_confirmation.is_some() {
        render_quit_confirmation(app, frame);
    }
    if app.show_help {
        render_help(frame);
    }
    if let Some(notice) = &app.notice {
        render_notice(notice, frame);
    }
}

fn render_error(error: &str, frame: &mut Frame) {
    let block = Block::default()
        .title(" DUV — error ")
        .title_bottom(key_hints(&[("h", "back"), ("?", "help"), ("q", "quit")]))
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
                ("?", "help"),
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
        Cell::from("FS"),
        Cell::from("Total"),
        Cell::from("Used"),
        Cell::from("Available"),
        Cell::from("Use"),
    ])
    .style(Style::default().add_modifier(Modifier::BOLD));

    // Name and mount point share what the fixed columns leave.
    // Filesystem types are short (apfs, ext4, btrfs, exfat, ntfs).
    const FILESYSTEM: u16 = 7;
    const SIZE: u16 = 10;
    let bar_cells = bar_width(
        frame.area().width,
        FILESYSTEM + 3 * SIZE + PERCENT_WIDTH + 6,
    );

    let rows = app.disks.iter().enumerate().map(|(i, disk)| {
        let selected = i == app.selected;
        let style = if selected {
            Style::default()
                .fg(Color::Black)
                .bg(Color::Yellow)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default()
        };
        // Green, then yellow and red as the disk fills up (not on the
        // highlighted row, which keeps its readable black on yellow).
        let fill = match disk.used_percent() {
            _ if selected => None,
            0..75 => Some(Color::Green),
            75..90 => Some(Color::Yellow),
            _ => Some(Color::Red),
        };
        let name = if disk.name.is_empty() {
            disk.mount_point.clone()
        } else {
            disk.name.clone()
        };
        Row::new(vec![
            Cell::from(name),
            Cell::from(disk.mount_point.clone()),
            Cell::from(disk.file_system.clone()),
            Cell::from(format_bytes(disk.total_space)),
            Cell::from(format_bytes(disk.used_space())),
            Cell::from(format_bytes(disk.available_space)),
            Cell::from(usage_bar(
                disk.used_fraction(),
                &format!("{:>3}%", disk.used_percent()),
                bar_cells,
                fill,
            )),
        ])
        .style(style)
    });

    let table = Table::new(
        rows,
        [
            Constraint::Fill(1),
            Constraint::Fill(1),
            Constraint::Length(FILESYSTEM),
            Constraint::Length(SIZE),
            Constraint::Length(SIZE),
            Constraint::Length(SIZE),
            Constraint::Length(bar_cells + PERCENT_WIDTH),
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
            ("s", "rescan"),
            ("t", "sort"),
            ("?", "help"),
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

        // The active sort's column is marked with its direction: ↑ when
        // values go up down the list (A to Z, smallest or oldest first).
        let sort = scanner.sort();
        let arrow = if sort.ascending() { "↑" } else { "↓" };
        let column = |title: &str, by: SortBy| {
            if sort.by == by {
                Cell::from(format!("{title} {arrow}"))
            } else {
                Cell::from(title.to_string())
            }
        };
        let header = Row::new(vec![
            column("Name", SortBy::Name),
            Cell::from("Type"),
            column("Size", SortBy::Size),
            Cell::from("Share"),
            column("Modified", SortBy::Modified),
        ])
        .style(header_style);

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |since| since.as_secs());
        // Name takes what the fixed columns leave; the bar adapts too.
        const TYPE: u16 = 4;
        const SIZE: u16 = 10;
        const MODIFIED: u16 = 14;
        let bar_cells = bar_width(
            frame.area().width,
            TYPE + SIZE + MODIFIED + PERCENT_WIDTH + 4,
        );
        // Shares are of the whole folder, even when a filter hides some
        // entries.
        let folder_size = scanner.current_size();
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
            // Colored only where the row doesn't set its own colors.
            let fill = (!is_scanning && i != selected).then_some(Color::Cyan);
            Row::new(vec![
                Cell::from(entry.name.to_string()),
                Cell::from(if entry.is_dir() { "Dir" } else { "File" }),
                Cell::from(format_bytes(entry.size)),
                Cell::from(usage_bar(
                    fraction(entry.size, folder_size),
                    &share_percent(entry.size, folder_size),
                    bar_cells,
                    fill,
                )),
                Cell::from(age(entry.modified, now)),
            ])
            .style(style)
        });

        let table = Table::new(
            rows,
            [
                Constraint::Fill(1),
                Constraint::Length(TYPE),
                Constraint::Length(SIZE),
                Constraint::Length(bar_cells + PERCENT_WIDTH),
                Constraint::Length(MODIFIED),
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

/// Width of the percentage after a usage bar: a space and up to "100%".
const PERCENT_WIDTH: u16 = 5;

/// The narrowest and widest a usage bar gets, in cells.
const BAR_MIN: u16 = 5;
const BAR_MAX: u16 = 20;

/// The flexible columns (names) keep at least this many cells.
const NAME_MIN: u16 = 24;

/// How wide a usage bar can be in a full-screen table `screen_width`
/// cells wide, given the cells the other fixed columns (including column
/// gaps) take: whatever's left after the names get [`NAME_MIN`], kept
/// within [`BAR_MIN`]..=[`BAR_MAX`].
fn bar_width(screen_width: u16, fixed: u16) -> u16 {
    // The table sits inside a border on each side.
    let inner = screen_width.saturating_sub(2);
    inner
        .saturating_sub(fixed + NAME_MIN)
        .clamp(BAR_MIN, BAR_MAX)
}

/// `part / whole`, or `0.0` when `whole` is zero.
fn fraction(part: u64, whole: u64) -> f64 {
    if whole == 0 {
        0.0
    } else {
        part as f64 / whole as f64
    }
}

/// `part`'s share of `whole` as a right-aligned percentage, "<1%" for a
/// tiny but nonzero share rather than a misleading "0%".
fn share_percent(part: u64, whole: u64) -> String {
    let percent = fraction(part, whole) * 100.0;
    if part > 0 && percent < 1.0 {
        " <1%".to_string()
    } else {
        format!("{:>3.0}%", percent.min(100.0))
    }
}

/// The filled part of a `cells`-wide bar for `fraction` (`0.0..=1.0`),
/// using eighth-block characters so the end is drawn to 1/8 of a cell.
fn bar_fill(fraction: f64, cells: u16) -> String {
    const PARTIAL: [&str; 8] = ["", "▏", "▎", "▍", "▌", "▋", "▊", "▉"];
    let eighths = (fraction.clamp(0.0, 1.0) * f64::from(cells) * 8.0).round() as usize;
    let mut fill = "█".repeat(eighths / 8);
    fill.push_str(PARTIAL[eighths % 8]);
    fill
}

/// A usage bar cell: the bar filled to `fraction`, then `percent`. With a
/// `fill` color the bar sits on a dim `░` track that shows its full
/// length; without one (the highlighted row, or while scanning) it takes
/// the row's own colors over a blank track, which stays readable on the
/// row's background.
fn usage_bar(fraction: f64, percent: &str, cells: u16, fill: Option<Color>) -> Line<'static> {
    let filled = bar_fill(fraction, cells);
    let rest = usize::from(cells).saturating_sub(filled.chars().count());
    let (fill_style, track) = match fill {
        Some(color) => (Style::default().fg(color), "░".repeat(rest)),
        None => (Style::default(), " ".repeat(rest)),
    };
    Line::from(vec![
        Span::styled(filled, fill_style),
        Span::styled(track, Style::default().fg(Color::DarkGray)),
        Span::raw(format!(" {percent}")),
    ])
}

/// How long ago `modified` (seconds since the Unix epoch) was, relative to
/// `now`, e.g. "3 days ago". Shown as an age rather than a date because
/// formatting local dates would need a timezone library.
fn age(modified: u32, now: u64) -> String {
    const MINUTE: u64 = 60;
    const HOUR: u64 = 60 * MINUTE;
    const DAY: u64 = 24 * HOUR;
    const MONTH: u64 = 30 * DAY;
    const YEAR: u64 = 365 * DAY;
    if modified == 0 {
        return "unknown".to_string();
    }
    let elapsed = now.saturating_sub(u64::from(modified));
    let (count, unit) = match elapsed {
        e if e < MINUTE => return "just now".to_string(),
        e if e < HOUR => (e / MINUTE, "minute"),
        e if e < DAY => (e / HOUR, "hour"),
        e if e < MONTH => (e / DAY, "day"),
        e if e < YEAR => (e / MONTH, "month"),
        e => (e / YEAR, "year"),
    };
    let plural = if count == 1 { "" } else { "s" };
    format!("{count} {unit}{plural} ago")
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

/// The delete confirmation: the question, then a note that the space is
/// only freed once `trash_name` (e.g. "the Trash (~/.local/share/Trash)")
/// is emptied, wrapped to fit the popup in evenly balanced lines.
fn render_delete_confirmation(request: &DeleteRequest, trash_name: &str, frame: &mut Frame) {
    let question = Line::from(Span::styled(
        format!(" Move {} to the Trash? ", request.name),
        Style::default().add_modifier(Modifier::BOLD),
    ));
    let note_width = question.width().max(usize::from(CONFIRMATION_WIDTH) - 4);
    let note = format!(
        "Frees {} only when {trash_name} is emptied.",
        format_bytes(request.size)
    );
    let mut message = vec![question, Line::from("")];
    message.extend(wrap_balanced(&note, note_width).into_iter().map(Line::from));
    render_confirmation(frame, " Delete ", message, request.choice);
}

/// Like [`wrap_words`], but with lines as even as possible: the narrowest
/// width that still needs no more lines than `width` does, so the last line
/// isn't left with a single word.
fn wrap_balanced(text: &str, width: usize) -> Vec<String> {
    let line_count = wrap_words(text, width).len();
    let mut narrowest = width;
    while narrowest > 1 && wrap_words(text, narrowest - 1).len() == line_count {
        narrowest -= 1;
    }
    wrap_words(text, narrowest)
}

/// Splits `text` into lines of at most `width` characters at spaces (a
/// single longer word gets a line of its own).
fn wrap_words(text: &str, width: usize) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    for word in text.split_whitespace() {
        match lines.last_mut() {
            Some(line) if line.chars().count() + 1 + word.chars().count() <= width => {
                line.push(' ');
                line.push_str(word);
            }
            _ => lines.push(word.to_string()),
        }
    }
    lines
}

/// Minimum width of Yes/No popups, in columns.
const CONFIRMATION_WIDTH: u16 = 40;

/// A centered Yes/No popup: `message` lines, a blank line, then the two
/// buttons with `choice` highlighted.
fn render_confirmation(frame: &mut Frame, title: &str, mut message: Vec<Line>, choice: Choice) {
    // Reverse video swaps the terminal's own text and background colors, so
    // the highlighted button keeps the same text color as the other one
    // and stays readable on any theme (fixed colors like black on yellow
    // can lose contrast on some).
    let highlighted = Style::default().add_modifier(Modifier::REVERSED | Modifier::BOLD);
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

/// Every key binding, grouped, as `(keys, action)` rows. Section headings
/// have no action. Kept short enough for a 24-row terminal.
const HELP: &[(&str, &str)] = &[
    ("Moving", ""),
    ("j k  ↓ ↑", "move down / up"),
    ("gg Home  G End", "first / last row"),
    ("PgDn PgUp", "page down / up"),
    ("Ctrl+d Ctrl+u", "half page down / up"),
    ("Folders", ""),
    ("l  Enter  →", "open folder"),
    ("h  Backspace  ←  Esc", "go back"),
    ("s", "scan disk / rescan folder"),
    ("Actions", ""),
    ("d  Delete", "move to Trash (asks first)"),
    ("o", "show in file manager"),
    ("y", "copy full path"),
    ("Filter and sort", ""),
    ("/", "filter this folder by name"),
    ("Enter  Esc", "keep / cancel (Esc clears)"),
    ("t", "sort by size / name / modified"),
    ("r", "reverse the sort order"),
    ("General", ""),
    ("?", "this help"),
    ("q  Ctrl+C", "quit (asks first) / at once"),
];

/// A centered popup listing every key binding, closed by any key.
fn render_help(frame: &mut Frame) {
    let keys_width = HELP
        .iter()
        .map(|(keys, _)| keys.chars().count())
        .max()
        .unwrap_or(0);
    let heading = Style::default()
        .fg(Color::Yellow)
        .add_modifier(Modifier::BOLD);
    let key = Style::default().add_modifier(Modifier::BOLD);

    let lines: Vec<Line> = HELP
        .iter()
        .map(|&(keys, action)| {
            if action.is_empty() {
                Line::from(Span::styled(format!(" {keys}"), heading))
            } else {
                Line::from(vec![
                    Span::styled(format!("   {keys:<keys_width$}  "), key),
                    Span::raw(format!("{action} ")),
                ])
            }
        })
        .collect();

    let width = lines.iter().map(Line::width).max().unwrap_or(0) as u16 + 2;
    let area = popup_area(frame.area(), width, lines.len() as u16 + 2);
    let block = Block::default()
        .title(" Keys ")
        .title_alignment(Alignment::Center)
        .title_bottom(Line::from(Span::styled(
            " any key to close ",
            Style::default().fg(Color::DarkGray),
        )))
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded);

    frame.render_widget(Clear, area);
    frame.render_widget(Paragraph::new(lines).block(block), area);
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
        let delete = popup_width(|frame| {
            render_delete_confirmation(&delete_request("a.txt"), "the Trash", frame)
        });
        assert_eq!(quit, CONFIRMATION_WIDTH as usize);
        assert_eq!(delete, quit);
    }

    #[test]
    fn bar_fill_draws_to_an_eighth_of_a_cell() {
        assert_eq!(bar_fill(0.0, 4), "");
        assert_eq!(bar_fill(1.0, 4), "████");
        assert_eq!(bar_fill(0.5, 4), "██");
        assert_eq!(bar_fill(1.0 / 32.0, 4), "▏", "one eighth of a cell");
        assert_eq!(bar_fill(0.5 + 3.0 / 32.0, 4), "██▍");
        assert_eq!(bar_fill(2.0, 4), "████", "clamped to full");
        assert_eq!(bar_fill(-1.0, 4), "", "clamped to empty");
    }

    #[test]
    fn share_percent_never_shows_a_misleading_zero() {
        assert_eq!(share_percent(0, 0), "  0%");
        assert_eq!(share_percent(0, 100), "  0%");
        assert_eq!(share_percent(1, 1_000), " <1%");
        assert_eq!(share_percent(50, 100), " 50%");
        assert_eq!(share_percent(100, 100), "100%");
    }

    #[test]
    fn bars_adapt_to_the_terminal_width() {
        // The scan table's fixed columns: type, size, modified, percent
        // and four gaps.
        let fixed = 4 + 10 + 14 + PERCENT_WIDTH + 4;
        assert_eq!(bar_width(80, fixed), 17);
        assert_eq!(bar_width(40, fixed), BAR_MIN, "never thinner");
        assert_eq!(bar_width(200, fixed), BAR_MAX, "never wider");
    }

    #[test]
    fn usage_bar_has_a_constant_width_and_colors_only_when_asked() {
        let colored = usage_bar(0.25, " 25%", 8, Some(Color::Cyan));
        assert_eq!(colored.width(), 8 + 1 + 4);
        assert_eq!(colored.spans[0].content, "██");
        assert_eq!(colored.spans[0].style.fg, Some(Color::Cyan));

        // On the highlighted row the bar takes the row's own colors, over a
        // blank track.
        let plain = usage_bar(0.25, " 25%", 8, None);
        assert_eq!(plain.width(), 8 + 1 + 4);
        assert!(plain.spans[0].style.fg.is_none());
        assert_eq!(plain.spans[1].content.trim(), "");
    }

    /// Every cell of a full render of `app`, as (symbol, foreground) rows.
    fn render_cells(app: &mut App, width: u16, height: u16) -> Vec<Vec<(String, Color)>> {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal.draw(|frame| render(app, frame)).unwrap();
        let buffer = terminal.backend().buffer();
        (0..height)
            .map(|y| {
                (0..width)
                    .map(|x| (buffer[(x, y)].symbol().to_string(), buffer[(x, y)].fg))
                    .collect()
            })
            .collect()
    }

    fn row_text(row: &[(String, Color)]) -> String {
        row.iter().map(|(symbol, _)| symbol.as_str()).collect()
    }

    #[test]
    fn scan_results_show_each_entrys_share_of_the_folder() {
        let dir = std::env::temp_dir().join(format!("duv-ui-share-{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("big.bin"), vec![0u8; 300_000]).unwrap();
        std::fs::write(dir.join("small.bin"), vec![0u8; 100_000]).unwrap();
        let mut app = App::with_start_path(dir.clone(), crate::scanner::DEFAULT_MEMORY_BUDGET);
        while !app.scanner.as_ref().unwrap().finished {
            app.tick();
            std::thread::sleep(std::time::Duration::from_millis(5));
        }

        let screen = render_cells(&mut app, 80, 6);
        assert!(row_text(&screen[1]).contains("Share"));
        let big = screen
            .iter()
            .find(|row| row_text(row).contains("big.bin"))
            .unwrap();
        let small = screen
            .iter()
            .find(|row| row_text(row).contains("small.bin"))
            .unwrap();
        // Roughly 3:1, as allocated sizes may round up to whole blocks.
        assert!(row_text(big).contains("75%") || row_text(big).contains("74%"));
        assert!(row_text(small).contains("25%") || row_text(small).contains("26%"));

        // The highlighted row's bar keeps the row's black; others are cyan.
        let bar_color = |row: &[(String, Color)]| {
            row.iter()
                .find(|(symbol, _)| symbol == "█")
                .map(|(_, fg)| *fg)
        };
        assert_eq!(bar_color(big), Some(Color::Black), "highlighted row");
        assert_eq!(bar_color(small), Some(Color::Cyan));

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn disk_list_shows_df_style_use_colored_by_fullness() {
        use crate::disks::{DiskInfo, DiskKind};
        let disk = |name: &str, available: u64| DiskInfo {
            name: name.to_string(),
            mount_point: format!("/{name}"),
            file_system: "ext4".to_string(),
            total_space: 1000,
            available_space: available,
            free_space: available,
            is_removable: false,
            kind: DiskKind::Ssd,
        };
        let mut app = App::new(crate::scanner::DEFAULT_MEMORY_BUDGET);
        app.disks = vec![
            disk("first", 500),
            disk("roomy", 600),
            disk("tight", 200),
            disk("full", 50),
        ];

        let screen = render_cells(&mut app, 80, 8);
        let row = |name: &str| {
            screen
                .iter()
                .find(|row| row_text(row).contains(name))
                .unwrap()
        };
        let bar_color = |name: &str| {
            row(name)
                .iter()
                .find(|(symbol, _)| symbol == "█")
                .map(|(_, fg)| *fg)
        };
        assert!(row_text(row("roomy")).contains(" 40%"));
        assert!(row_text(row("tight")).contains(" 80%"));
        assert!(row_text(row("full")).contains(" 95%"));
        assert_eq!(bar_color("roomy"), Some(Color::Green));
        assert_eq!(bar_color("tight"), Some(Color::Yellow));
        assert_eq!(bar_color("full"), Some(Color::Red));
        assert_eq!(bar_color("first"), Some(Color::Black), "highlighted row");
    }

    #[test]
    fn age_reads_naturally() {
        let now = 2_000_000_000;
        assert_eq!(age(0, now), "unknown");
        assert_eq!(age(1_999_999_990, now), "just now");
        assert_eq!(age(1_999_999_940, now), "1 minute ago");
        assert_eq!(age(1_999_999_000, now), "16 minutes ago");
        assert_eq!(age(1_999_996_400, now), "1 hour ago");
        assert_eq!(age(1_999_000_000, now), "11 days ago");
        assert_eq!(age(1_990_000_000, now), "3 months ago");
        assert_eq!(age(1_900_000_000, now), "3 years ago");
        // A timestamp in the future (clock skew) isn't negative.
        assert_eq!(age(2_000_000_100, now), "just now");
    }

    #[test]
    fn sort_status_fits_an_80_column_border() {
        use crate::scanner::{Sort, SortBy};
        for by in [SortBy::Size, SortBy::Name, SortBy::Modified] {
            for reversed in [false, true] {
                let status = crate::app::sort_status(Sort { by, reversed });
                // 78 columns inside the corners, minus the line's padding.
                assert!(status.chars().count() + 2 <= 78, "{status}");
            }
        }
    }

    #[test]
    fn help_fits_a_standard_terminal() {
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        terminal.draw(render_help).unwrap();
        let buffer = terminal.backend().buffer();
        let screen: String = (0..buffer.area.height)
            .flat_map(|y| (0..buffer.area.width).map(move |x| (x, y)))
            .map(|(x, y)| buffer[(x, y)].symbol().to_string())
            .collect();
        // Both the first and the last rows (and the closing hint) are shown.
        assert!(screen.contains("move down / up"));
        assert!(screen.contains("quit (asks first) / at once"));
        assert!(screen.contains("any key to close"));
    }

    #[test]
    fn delete_popup_says_space_is_freed_only_when_the_trash_is_emptied() {
        let linux_trash = "the Trash (~/.local/share/Trash)";
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        terminal
            .draw(|frame| render_delete_confirmation(&delete_request("a.txt"), linux_trash, frame))
            .unwrap();
        let buffer = terminal.backend().buffer();
        let text: String = (0..buffer.area.height)
            .map(|y| {
                (0..buffer.area.width)
                    .map(|x| buffer[(x, y)].symbol().to_string())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join(" ");
        let words = text.split_whitespace().collect::<Vec<_>>().join(" ");
        assert!(
            words.contains(
                "Frees 4.0 KiB only when the Trash │ │ (~/.local/share/Trash) is emptied."
            ),
            "{words}"
        );

        // Wrapped to fit, so the popup keeps the quit popup's width.
        let width = popup_width(|frame| {
            render_delete_confirmation(&delete_request("a.txt"), linux_trash, frame)
        });
        assert_eq!(width, CONFIRMATION_WIDTH as usize);
    }

    #[test]
    fn wrap_balanced_avoids_a_lonely_last_word() {
        let text = "Frees 1.2 GiB only when the Trash is emptied.";
        assert_eq!(
            wrap_words(text, 36),
            ["Frees 1.2 GiB only when the Trash is", "emptied."]
        );
        assert_eq!(
            wrap_balanced(text, 36),
            ["Frees 1.2 GiB only when", "the Trash is emptied."]
        );
        // The largest size label with the Linux folder still takes two lines.
        let linux = "Frees 1023.9 GiB only when the Trash (~/.local/share/Trash) is emptied.";
        let lines = wrap_balanced(linux, 36);
        assert_eq!(lines.len(), 2);
        assert!(lines.iter().all(|line| line.len() <= 36));
    }

    #[test]
    fn wrap_words_breaks_at_spaces() {
        assert_eq!(wrap_words("aa bb cc", 5), ["aa bb", "cc"]);
        assert_eq!(
            wrap_words("a verylongword b", 4),
            ["a", "verylongword", "b"]
        );
        assert!(wrap_words("", 10).is_empty());
    }

    #[test]
    fn highlighted_button_uses_reverse_video_not_fixed_colors() {
        let mut terminal = Terminal::new(TestBackend::new(60, 12)).unwrap();
        terminal
            .draw(|frame| {
                render_delete_confirmation(
                    &DeleteRequest {
                        choice: Choice::Yes,
                        ..delete_request("a.txt")
                    },
                    "the Trash",
                    frame,
                )
            })
            .unwrap();
        let buffer = terminal.backend().buffer();
        let cells: Vec<_> = (0..buffer.area.height)
            .flat_map(|y| (0..buffer.area.width).map(move |x| (x, y)))
            .map(|(x, y)| &buffer[(x, y)])
            .collect();
        let yes = cells
            .windows(3)
            .find(|w| w.iter().map(|c| c.symbol()).collect::<String>() == "Yes")
            .unwrap();
        let no = cells
            .windows(2)
            .find(|w| w.iter().map(|c| c.symbol()).collect::<String>() == "No")
            .unwrap();
        // Same (default) colors as the other button; only reversed.
        assert_eq!(yes[0].fg, no[0].fg);
        assert_eq!(yes[0].bg, no[0].bg);
        assert!(yes[0].modifier.contains(Modifier::REVERSED));
        assert!(!no[0].modifier.contains(Modifier::REVERSED));
    }

    #[test]
    fn delete_popup_widens_for_long_names() {
        let name = "a-really-long-file-name-that-does-not-fit-in-forty-columns.tar.gz";
        let width = popup_width(|frame| {
            render_delete_confirmation(&delete_request(name), "the Trash", frame)
        });
        let question = format!(" Move {name} to the Trash? ");
        assert_eq!(width, question.len() + 4);
    }
}

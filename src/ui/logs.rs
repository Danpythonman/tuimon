//! The system messages tab, streamed from the systemd journal.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Cell, Paragraph, Row, Table, Wrap};

use super::{ACCENT, command_line, filter_spans, panel, scrollbar, shell_quote};
use crate::app::App;
use crate::journal::{LogEntry, PRIORITY_NAMES};
use crate::view::LogsView;

const SOURCE_MAX_WIDTH: usize = 24;
const DETAIL_HEIGHT: u16 = 4;

pub fn draw(frame: &mut Frame, area: Rect, app: &mut App) {
    let view = &mut app.logs;
    let note = view
        .kernel_only
        .then_some("kernel only; also: sudo dmesg -w");
    let area = command_line(frame, area, &equivalent_command(view), note);

    let entries = view.visible();
    let mut title = vec![
        Span::raw(" System messages ").bold(),
        Span::raw(format!("· {} of {} ", entries.len(), view.total())).dark_gray(),
        Span::raw("· level ").dark_gray(),
        Span::styled(format!("{} ", view.level_label()), Style::new().fg(ACCENT)),
    ];
    if view.kernel_only {
        title.push(Span::styled("· kernel ", Style::new().fg(ACCENT)));
    }
    title.extend(filter_spans(&view.filter));
    let mut block = panel(Line::from(title));
    block = if view.follow {
        block.title(
            Line::from(Span::styled(" ● following ", Style::new().fg(Color::Green)))
                .right_aligned(),
        )
    } else {
        block.title(
            Line::from(Span::raw(" paused scroll · G to follow ").dark_gray()).right_aligned(),
        )
    };

    if let Some(error) = &view.error {
        let msg = Paragraph::new(error.as_str())
            .red()
            .wrap(Wrap { trim: true })
            .block(block);
        frame.render_widget(msg, area);
        return;
    }

    let inner = block.inner(area);
    frame.render_widget(block, area);
    let [table_area, detail_area] =
        Layout::vertical([Constraint::Fill(1), Constraint::Length(DETAIL_HEIGHT)]).areas(inner);

    let source_width = entries
        .iter()
        .map(|e| source_label(e).chars().count())
        .max()
        .unwrap_or(0)
        .clamp(6, SOURCE_MAX_WIDTH);
    let widths = [
        Constraint::Length(15),
        Constraint::Length(7),
        Constraint::Length(u16::try_from(source_width).unwrap_or(24)),
        Constraint::Fill(1),
    ];
    let header =
        Row::new(["Time", "Level", "Source", "Message"]).style(Style::new().fg(ACCENT).bold());
    let len = entries.len();
    let rows = entries.iter().map(|e| entry_row(e));
    let selected_entry = view
        .table
        .selected()
        .and_then(|i| entries.get(i))
        .map(|&e| e.clone());
    let table = Table::new(rows, widths)
        .header(header)
        .column_spacing(1)
        .row_highlight_style(Style::new().bg(Color::DarkGray).bold());
    frame.render_stateful_widget(table, table_area, &mut view.table);
    // The header takes one row.
    view.nav.page = usize::from(table_area.height.saturating_sub(1));
    // `area` still has its border, which the scrollbar sits on.
    scrollbar(frame, area, len, view.table.selected().unwrap_or(0));

    draw_detail(frame, detail_area, selected_entry.as_ref());
}

/// The full text of the selected message, which the table truncates.
fn draw_detail(frame: &mut Frame, area: Rect, entry: Option<&LogEntry>) {
    let block = Block::new()
        .borders(Borders::TOP)
        .border_style(Style::new().fg(Color::DarkGray));
    let text = entry.map_or_else(
        || Line::from("No messages").dark_gray(),
        |e| {
            Line::from(vec![
                Span::styled(
                    format!("{} ", e.time.format("%Y-%m-%d %H:%M:%S%.3f")),
                    Style::new().dark_gray(),
                ),
                Span::styled(format!("{}: ", source_label(e)), Style::new().bold()),
                Span::styled(e.message.clone(), priority_style(e.priority)),
            ])
        },
    );
    frame.render_widget(
        Paragraph::new(text).wrap(Wrap { trim: false }).block(block),
        area,
    );
}

fn entry_row(e: &LogEntry) -> Row<'static> {
    let style = priority_style(e.priority);
    Row::new([
        Cell::from(e.time.format("%b %d %H:%M:%S").to_string()).dark_gray(),
        Cell::from(e.priority_name()).style(style.bold()),
        Cell::from(source_label(e)),
        // Multi-line messages are shown in full in the detail pane.
        Cell::from(e.message.replace('\n', " ⏎ ")).style(style),
    ])
}

fn source_label(e: &LogEntry) -> String {
    match e.pid {
        Some(pid) => format!("{}[{pid}]", e.source),
        None => e.source.clone(),
    }
}

fn priority_style(priority: u8) -> Style {
    match priority {
        0..=3 => Style::new().fg(Color::Red),
        4 => Style::new().fg(Color::Yellow),
        5 => Style::new().fg(Color::White).bold(),
        6 => Style::new(),
        _ => Style::new().dark_gray(),
    }
}

/// The `journalctl` invocation that shows the same messages.
fn equivalent_command(view: &LogsView) -> String {
    let mut command = String::from("journalctl -f -n 1000");
    if view.kernel_only {
        command.push_str(" -k");
    }
    if view.min_priority < 7 {
        command.push_str(" -p ");
        command.push_str(PRIORITY_NAMES[usize::from(view.min_priority)]);
    }
    if view.filter.is_active() {
        let query = view.filter.query();
        command.push_str(" -g ");
        command.push_str(&shell_quote(query));
        // journalctl only matches case-insensitively for all-lowercase
        // patterns; our filter always ignores case.
        if query.chars().any(char::is_uppercase) {
            command.push_str(" --case-sensitive=false");
        }
    }
    command
}

//! The processes tab, like `top`.

use ratatui::Frame;
use ratatui::layout::{Constraint, Rect};
use ratatui::style::{Color, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Cell, Row, Table};

use super::{ACCENT, command_line, filter_spans, load_style, panel, scrollbar};
use crate::app::App;
use crate::format;
use crate::processes::{ProcessInfo, SortKey};

pub fn draw(frame: &mut Frame, area: Rect, app: &mut App) {
    let interval = app.interval_secs();
    let view = &mut app.processes;
    let processes = view.visible();

    // `top` sorts high-to-low by default; a leading `-` flips it.
    let direction = if view.descending() { "" } else { "-" };
    let command = format!("top -d {interval} -o {direction}{}", view.sort.top_field());
    let note = view.filter.is_active().then(|| {
        format!(
            "to filter, press o and enter COMMAND={}",
            view.filter.query()
        )
    });
    let area = command_line(frame, area, &command, note.as_deref());

    let mut title = vec![
        Span::raw(" Processes ").bold(),
        Span::raw(if view.filter.is_active() {
            format!("· {} of {} ", processes.len(), view.total())
        } else {
            format!("· {} ", view.total())
        })
        .dark_gray(),
    ];
    title.push(Span::raw("· sorted by ").dark_gray());
    title.push(Span::styled(
        format!("{} ", view.sort.label()),
        Style::new().fg(ACCENT),
    ));
    title.extend(filter_spans(&view.filter));
    let block = panel(Line::from(title));

    let arrow = if view.descending() { " ▼" } else { " ▲" };
    let header_cell = |label: &str, key: Option<SortKey>| {
        if key == Some(view.sort) {
            Cell::from(format!("{label}{arrow}")).fg(Color::Yellow)
        } else {
            Cell::from(label.to_string())
        }
    };
    let header = Row::new([
        header_cell("PID", Some(SortKey::Pid)),
        header_cell("User", None),
        header_cell("CPU%", Some(SortKey::Cpu)),
        header_cell("Memory", Some(SortKey::Memory)),
        header_cell("Mem%", None),
        header_cell("State", None),
        header_cell("Uptime", None),
        header_cell("Command", Some(SortKey::Name)),
    ])
    .style(Style::new().fg(ACCENT).bold());

    let user_width = processes
        .iter()
        .map(|p| p.user.len())
        .max()
        .unwrap_or(0)
        .clamp(4, 12);
    let widths = [
        Constraint::Length(7),
        Constraint::Length(u16::try_from(user_width).unwrap_or(12)),
        Constraint::Length(7),
        Constraint::Length(10),
        Constraint::Length(6),
        Constraint::Length(9),
        Constraint::Length(11),
        Constraint::Fill(1),
    ];

    let len = processes.len();
    let rows = processes.iter().map(|p| process_row(p));
    let table = Table::new(rows, widths)
        .header(header)
        .block(block)
        .column_spacing(1)
        .row_highlight_style(Style::new().bg(Color::DarkGray).bold())
        .highlight_symbol("▶ ");
    frame.render_stateful_widget(table, area, &mut view.table);
    view.nav.page = usize::from(area.height.saturating_sub(3));
    scrollbar(frame, area, len, view.table.selected().unwrap_or(0));
}

fn process_row(p: &ProcessInfo) -> Row<'static> {
    let dim = Style::new().dark_gray();
    let cpu_style = if p.cpu < 0.1 {
        dim
    } else {
        load_style(p.cpu.min(100.0))
    };
    let state_style = match p.status.as_str() {
        "Runnable" | "Run" => Style::new().fg(Color::Green),
        "Zombie" | "Dead" => Style::new().fg(Color::Red),
        _ => dim,
    };
    let command = if p.command.starts_with('[') {
        // Kernel threads have no command line; dim them like `ps` does.
        Span::styled(p.command.clone(), dim)
    } else {
        Span::raw(p.command.clone())
    };
    Row::new([
        Cell::from(Line::from(p.pid.to_string()).right_aligned()),
        Cell::from(p.user.clone()),
        Cell::from(Line::from(format!("{:.1}", p.cpu)).right_aligned()).style(cpu_style),
        Cell::from(Line::from(format::bytes(p.memory)).right_aligned()),
        Cell::from(Line::from(format!("{:.1}", p.mem_percent)).right_aligned()),
        Cell::from(p.status.clone()).style(state_style),
        Cell::from(format::duration(p.run_time)).style(dim),
        Cell::from(command),
    ])
}

//! Rendering of the dashboard. Each tab lives in its own submodule.

mod disks;
mod logs;
mod network;
mod overview;
mod ports;
mod processes;

use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Layout, Margin, Rect};
use ratatui::style::{Color, Modifier, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, BorderType, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState, Tabs,
};
use sysinfo::System;

use crate::app::{App, Tab};
use crate::format;
use crate::view::TextFilter;

const MIN_WIDTH: u16 = 60;
const MIN_HEIGHT: u16 = 22;

const ACCENT: Color = Color::Cyan;

pub fn draw(frame: &mut Frame, app: &mut App) {
    let area = frame.area();
    if area.width < MIN_WIDTH || area.height < MIN_HEIGHT {
        draw_too_small(frame, area);
        return;
    }

    let [header, tabs, body, footer] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Fill(1),
        Constraint::Length(1),
    ])
    .areas(area);

    draw_header(frame, header, app);
    draw_tabs(frame, tabs, app.tab);
    match app.tab {
        Tab::Overview => overview::draw(frame, body, app),
        Tab::Network => network::draw(frame, body, app),
        Tab::Disks => disks::draw(frame, body, app),
        Tab::Processes => processes::draw(frame, body, app),
        Tab::Ports => ports::draw(frame, body, app),
        Tab::Messages => logs::draw(frame, body, app),
    }
    draw_footer(frame, footer, app);
}

fn draw_too_small(frame: &mut Frame, area: Rect) {
    let msg = Paragraph::new(format!(
        "Terminal too small ({}x{})\nneed at least {MIN_WIDTH}x{MIN_HEIGHT}",
        area.width, area.height
    ))
    .alignment(Alignment::Center)
    .red();
    let [centered] = Layout::vertical([Constraint::Length(2)])
        .flex(ratatui::layout::Flex::Center)
        .areas(area);
    frame.render_widget(msg, centered);
}

fn draw_header(frame: &mut Frame, area: Rect, app: &App) {
    let load = System::load_average();
    let sep = Span::raw(" │ ").dark_gray();
    let mut spans = vec![
        Span::styled(" tuimon ", Style::new().fg(Color::Black).bg(ACCENT).bold()),
        Span::raw(" "),
        Span::raw(System::host_name().unwrap_or_else(|| "unknown".into())).bold(),
        sep.clone(),
        Span::raw(format!("up {}", format::duration(System::uptime()))),
        sep.clone(),
        Span::raw(format!(
            "load {:.2} {:.2} {:.2}",
            load.one, load.five, load.fifteen
        )),
        sep.clone(),
        Span::raw(format!("every {:.2}s", app.tick_rate.as_secs_f64())),
    ];
    if app.paused {
        spans.push(sep);
        spans.push(Span::styled(
            " PAUSED ",
            Style::new().fg(Color::Black).bg(Color::Yellow).bold(),
        ));
    }
    frame.render_widget(Line::from(spans), area);
}

fn draw_tabs(frame: &mut Frame, area: Rect, current: Tab) {
    let titles = Tab::ALL
        .iter()
        .enumerate()
        .map(|(i, tab)| format!(" {} {} ", i + 1, tab.title()));
    let tabs = Tabs::new(titles)
        .select(current.index())
        .style(Style::new().fg(Color::Gray))
        .highlight_style(Style::new().fg(Color::Black).bg(ACCENT).bold())
        .divider(" ")
        .padding("", "");
    frame.render_widget(tabs, area);
}

fn draw_footer(frame: &mut Frame, area: Rect, app: &App) {
    if let Some(status) = app.status() {
        let (symbol, color) = if status.is_error {
            ("✗", Color::Red)
        } else {
            ("✓", Color::Green)
        };
        let line = Line::from(vec![
            Span::styled(
                format!(" {symbol} "),
                Style::new().fg(Color::Black).bg(color).bold(),
            ),
            Span::styled(format!(" {}", status.text), Style::new().fg(color)),
        ]);
        frame.render_widget(line, area);
        return;
    }
    let keys: &[(&str, &str)] = if app.is_editing_filter() {
        &[("type", "to filter"), ("Enter", "apply"), ("Esc", "clear")]
    } else {
        match app.tab {
            Tab::Overview => &[
                ("q", "quit"),
                ("Tab", "tabs"),
                ("p", "pause"),
                ("+/-", "refresh rate"),
                ("n/N", "network interface"),
            ],
            Tab::Network => &[
                ("q", "quit"),
                ("Tab", "tabs"),
                ("p", "pause"),
                ("+/-", "refresh rate"),
                ("↑/↓", "select interface"),
                ("c/C", "copy IPv4/IPv6"),
            ],
            Tab::Disks => &[
                ("q", "quit"),
                ("Tab", "tabs"),
                ("p", "pause"),
                ("+/-", "refresh rate"),
                ("↑/↓", "select disk"),
            ],
            Tab::Processes => &[
                ("q", "quit"),
                ("Tab", "tabs"),
                ("p", "pause"),
                ("↑/↓", "select"),
                ("s", "sort"),
                ("r", "reverse"),
                ("/", "filter"),
            ],
            Tab::Ports => &[
                ("q", "quit"),
                ("Tab", "tabs"),
                ("p", "pause"),
                ("↑/↓", "select"),
                ("a", "listening/all"),
            ],
            Tab::Messages => &[
                ("q", "quit"),
                ("Tab", "tabs"),
                ("p", "pause"),
                ("l", "level"),
                ("d", "kernel only"),
                ("f", "follow"),
                ("/", "filter"),
            ],
        }
    };
    let spans: Vec<Span> = keys
        .iter()
        .flat_map(|(key, desc)| {
            [
                Span::styled(
                    format!(" {key} "),
                    Style::new().fg(Color::Black).bg(ACCENT).bold(),
                ),
                Span::styled(format!(" {desc}  "), Style::new().fg(Color::White)),
            ]
        })
        .collect();
    frame.render_widget(Line::from(spans), area);
}

/// Shows the shell command that produces the same data as the panel below
/// it, and returns the area left for the panel.
pub(super) fn command_line(
    frame: &mut Frame,
    area: Rect,
    command: &str,
    note: Option<&str>,
) -> Rect {
    let [line_area, rest] =
        Layout::vertical([Constraint::Length(1), Constraint::Fill(1)]).areas(area);
    let mut spans = vec![
        Span::styled(" $ ", Style::new().fg(Color::Green).bold()),
        Span::styled(command.to_owned(), Style::new().fg(Color::White)),
    ];
    if let Some(note) = note {
        spans.push(Span::raw(format!("  # {note}")).dark_gray());
    }
    frame.render_widget(Line::from(spans), line_area);
    rest
}

pub(super) fn panel(title: Line<'_>) -> Block<'_> {
    Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(Color::DarkGray))
        .title(title)
}

/// Green → yellow → red depending on how loaded something is.
pub(super) fn load_style(percent: f32) -> Style {
    let color = match percent {
        p if p >= 85.0 => Color::Red,
        p if p >= 60.0 => Color::Yellow,
        _ => Color::Green,
    };
    Style::new().fg(color).add_modifier(Modifier::BOLD)
}
/// Title spans describing an active or in-progress text filter.
pub(super) fn filter_spans(filter: &TextFilter) -> Vec<Span<'static>> {
    if filter.editing {
        vec![
            Span::raw("· filter: ").dark_gray(),
            Span::styled(
                format!("{}█ ", filter.query()),
                Style::new().fg(Color::Yellow).bold(),
            ),
        ]
    } else if filter.is_active() {
        vec![
            Span::raw("· filter: ").dark_gray(),
            Span::styled(
                format!("{} ", filter.query()),
                Style::new().fg(Color::Yellow).bold(),
            ),
        ]
    } else {
        Vec::new()
    }
}

/// Draws a vertical scrollbar along the right border of a bordered table.
pub(super) fn scrollbar(frame: &mut Frame, area: Rect, len: usize, position: usize) {
    let mut state = ScrollbarState::new(len).position(position);
    frame.render_stateful_widget(
        Scrollbar::new(ScrollbarOrientation::VerticalRight)
            .begin_symbol(None)
            .end_symbol(None),
        area.inner(Margin::new(0, 1)),
        &mut state,
    );
}

/// Quotes a string for a POSIX shell if it contains anything special.
pub(super) fn shell_quote(s: &str) -> String {
    let safe = !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_./:@%+=,".contains(c));
    if safe {
        s.to_string()
    } else {
        format!("'{}'", s.replace('\'', r"'\''"))
    }
}

#[cfg(test)]
mod tests {
    use super::shell_quote;

    #[test]
    fn quotes_for_shell() {
        assert_eq!(shell_quote("sshd"), "sshd");
        assert_eq!(shell_quote("two words"), "'two words'");
        assert_eq!(shell_quote("it's"), r"'it'\''s'");
        assert_eq!(shell_quote(""), "''");
    }
}

//! The ports tab: open sockets, like `ss -tulnp`.

use ratatui::Frame;
use ratatui::layout::{Constraint, Rect};
use ratatui::style::{Color, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Cell, Paragraph, Row, Table};

use super::{ACCENT, command_line, panel, scrollbar};
use crate::app::App;
use crate::ports::{Protocol, Socket};

pub fn draw(frame: &mut Frame, area: Rect, app: &mut App) {
    let ports = &mut app.ports;
    let sockets = ports.visible();
    let show_all = ports.show_all;

    let command = if show_all { "ss -tunap" } else { "ss -tulnp" };
    // Without root, sockets owned by other users show no process.
    let note = sockets
        .iter()
        .any(|s| s.process.is_none())
        .then_some("prefix with sudo to see other users' processes");
    let area = command_line(frame, area, command, note);

    let tcp = sockets
        .iter()
        .filter(|s| s.protocol == Protocol::Tcp)
        .count();
    let udp = sockets.len() - tcp;
    let block = panel(Line::from(vec![
        Span::raw(if show_all {
            " Sockets "
        } else {
            " Listening ports "
        })
        .bold(),
        Span::raw(format!("· {} ({tcp} tcp, {udp} udp) ", sockets.len())).dark_gray(),
    ]));

    if let Some(error) = &ports.error {
        let msg = Paragraph::new(format!("Could not read sockets: {error}"))
            .red()
            .block(block);
        frame.render_widget(msg, area);
        return;
    }

    // Size text columns to their widest value (but at least the header).
    let fit = |title: &str, label: fn(&Socket) -> String| {
        let widest = sockets.iter().map(|s| label(s).len()).max().unwrap_or(0);
        Constraint::Length(u16::try_from(widest.max(title.len())).unwrap_or(u16::MAX))
    };
    let mut header = vec!["Proto", "Local address", "Port"];
    let mut widths = vec![
        Constraint::Length(5),
        fit("Local address", local_label),
        Constraint::Length(5),
    ];
    if show_all {
        header.push("Remote address");
        widths.push(fit("Remote address", remote_label));
    }
    header.extend(["State", "User", "PID", "Process"]);
    widths.extend([
        Constraint::Length(10),
        fit("User", user_label),
        Constraint::Length(7),
        Constraint::Fill(1),
    ]);

    let rows = sockets.iter().map(|s| socket_row(s, show_all));
    let len = sockets.len();
    let table = Table::new(rows, widths)
        .header(Row::new(header).style(Style::new().fg(ACCENT).bold()))
        .block(block)
        .column_spacing(2)
        .row_highlight_style(Style::new().bg(Color::DarkGray).bold())
        .highlight_symbol("▶ ");
    frame.render_stateful_widget(table, area, &mut ports.table);
    ports.nav.page = usize::from(area.height.saturating_sub(3));

    scrollbar(frame, area, len, ports.table.selected().unwrap_or(0));
}

fn local_label(socket: &Socket) -> String {
    let ip = socket.local.ip();
    if ip.is_unspecified() {
        format!("{ip} (any)")
    } else {
        ip.to_string()
    }
}

fn remote_label(socket: &Socket) -> String {
    if socket.remote.port() == 0 && socket.remote.ip().is_unspecified() {
        "*".into()
    } else {
        socket.remote.to_string()
    }
}

fn user_label(socket: &Socket) -> String {
    socket
        .user
        .clone()
        .unwrap_or_else(|| socket.uid.to_string())
}

fn socket_row(socket: &Socket, show_all: bool) -> Row<'static> {
    let dim = Style::new().dark_gray();
    let local_ip = if socket.local.ip().is_unspecified() {
        Cell::from(Span::styled(local_label(socket), dim))
    } else {
        Cell::from(local_label(socket))
    };
    let mut cells = vec![
        Cell::from(socket.protocol.to_string()),
        local_ip,
        Cell::from(Span::styled(
            socket.local.port().to_string(),
            Style::new().fg(Color::Yellow).bold(),
        )),
    ];
    if show_all {
        let style = if socket.remote.ip().is_unspecified() {
            dim
        } else {
            Style::new()
        };
        cells.push(Cell::from(Span::styled(remote_label(socket), style)));
    }
    let state_style = match socket.state_name() {
        "LISTEN" | "UNCONN" => Style::new().fg(Color::Green),
        "ESTAB" => Style::new().fg(ACCENT),
        _ => Style::new().fg(Color::Gray),
    };
    let (pid, process) = socket.process.as_ref().map_or_else(
        || {
            (
                Cell::from(Span::styled("-", dim)),
                Cell::from(Span::styled("-", dim)),
            )
        },
        |p| {
            (
                Cell::from(p.pid.to_string()),
                Cell::from(p.name.clone()).bold(),
            )
        },
    );
    cells.extend([
        Cell::from(Span::styled(socket.state_name(), state_style)),
        Cell::from(user_label(socket)),
        pid,
        process,
    ]);
    Row::new(cells)
}

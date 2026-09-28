//! The network tab: every interface with its addresses and transfer rates,
//! plus details and a history chart for the selected one.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Cell, Paragraph, Row, Table};
use sysinfo::InterfaceOperationalState;

use super::overview::{RX_COLOR, TX_COLOR, draw_rate_chart, net_series};
use super::{ACCENT, command_line, panel, scrollbar};
use crate::app::App;
use crate::format;
use crate::metrics::InterfaceStats;

/// Rows the bottom panels need to be useful.
const MIN_DETAIL_HEIGHT: u16 = 12;
/// Width from which the interface table has room for the total columns.
const WIDE: u16 = 110;

pub fn draw(frame: &mut Frame, area: Rect, app: &mut App) {
    // Command line + borders + header + one row per interface, leaving the
    // rest for the selected interface.
    let rows = u16::try_from(app.metrics.interfaces.len()).unwrap_or(u16::MAX);
    let table_height = rows
        .saturating_add(4)
        .min(area.height.saturating_sub(MIN_DETAIL_HEIGHT))
        .max(5);
    let [table_area, bottom] =
        Layout::vertical([Constraint::Length(table_height), Constraint::Fill(1)]).areas(area);

    draw_table(frame, table_area, app);

    let selected = app
        .network
        .table
        .selected()
        .and_then(|i| app.metrics.interfaces.get(i));
    let Some(iface) = selected else {
        return;
    };
    let [detail_area, chart_area] =
        Layout::horizontal([Constraint::Percentage(45), Constraint::Percentage(55)]).areas(bottom);
    draw_details(frame, detail_area, iface);
    draw_chart(frame, chart_area, iface, app);
}

fn draw_table(frame: &mut Frame, area: Rect, app: &mut App) {
    let area = command_line(frame, area, "ip -brief address", None);
    let interfaces = &app.metrics.interfaces;
    let up = interfaces
        .iter()
        .filter(|i| i.state == Some(InterfaceOperationalState::Up))
        .count();
    let total = &app.metrics.net_total;
    let block = panel(Line::from(vec![
        Span::raw(" Interfaces ").bold(),
        Span::raw(format!("· {} ({up} up) · ", interfaces.len())).dark_gray(),
        Span::styled(
            format!("▼ {} ", format::rate(total.rx_rate)),
            Style::new().fg(RX_COLOR),
        ),
        Span::styled(
            format!("▲ {} ", format::rate(total.tx_rate)),
            Style::new().fg(TX_COLOR),
        ),
    ]));

    if interfaces.is_empty() {
        frame.render_widget(
            Paragraph::new("No network interfaces found.")
                .dark_gray()
                .block(block),
            area,
        );
        return;
    }

    let name_width = interfaces
        .iter()
        .map(|i| i.name.len())
        .max()
        .unwrap_or(0)
        .max("Interface".len());
    // On narrow terminals drop the totals (still in the details panel) so
    // the addresses stay visible.
    let show_totals = area.width >= WIDE;
    let mut widths = vec![
        Constraint::Length(u16::try_from(name_width).unwrap_or(u16::MAX)),
        Constraint::Length(7),
        Constraint::Length(10),
        Constraint::Length(10),
    ];
    let mut header = vec![
        Cell::from("Interface"),
        Cell::from("State"),
        Cell::from(Line::from("▼ RX").right_aligned()),
        Cell::from(Line::from("▲ TX").right_aligned()),
    ];
    if show_totals {
        widths.extend([Constraint::Length(10), Constraint::Length(10)]);
        header.extend([
            Cell::from(Line::from("Total RX").right_aligned()),
            Cell::from(Line::from("Total TX").right_aligned()),
        ]);
    }
    widths.push(Constraint::Fill(1));
    header.push(Cell::from("Addresses"));
    let header = Row::new(header).style(Style::new().fg(ACCENT).bold());

    let len = interfaces.len();
    let rows = interfaces.iter().map(|i| interface_row(i, show_totals));
    let table = Table::new(rows, widths)
        .header(header)
        .block(block)
        .column_spacing(2)
        .row_highlight_style(Style::new().bg(Color::DarkGray).bold())
        .highlight_symbol("▶ ");
    frame.render_stateful_widget(table, area, &mut app.network.table);
    app.network.nav.page = usize::from(area.height.saturating_sub(3));

    // Borders and header take three rows.
    if len > usize::from(area.height.saturating_sub(3)) {
        scrollbar(frame, area, len, app.network.table.selected().unwrap_or(0));
    }
}

fn interface_row(iface: &InterfaceStats, show_totals: bool) -> Row<'static> {
    let rate_cell = |rate: f64, color: Color| {
        let style = if rate < 1.0 {
            Style::new().dark_gray()
        } else {
            Style::new().fg(color)
        };
        Cell::from(Line::styled(format::rate(rate), style).right_aligned())
    };
    let total_cell = |bytes: u64| Cell::from(Line::from(format::bytes(bytes)).right_aligned());
    let addresses = if iface.addresses.is_empty() {
        Span::raw("-").dark_gray()
    } else {
        Span::raw(
            iface
                .addresses
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join("  "),
        )
    };
    let mut cells = vec![
        Cell::from(iface.name.clone()).bold(),
        Cell::from(state_span(iface.state)),
        rate_cell(iface.rx_rate, RX_COLOR),
        rate_cell(iface.tx_rate, TX_COLOR),
    ];
    if show_totals {
        cells.extend([total_cell(iface.total_rx), total_cell(iface.total_tx)]);
    }
    cells.push(Cell::from(addresses));
    Row::new(cells)
}

fn state_span(state: Option<InterfaceOperationalState>) -> Span<'static> {
    let Some(state) = state else {
        return Span::raw("-").dark_gray();
    };
    let color = match state {
        InterfaceOperationalState::Up => Color::Green,
        InterfaceOperationalState::Down | InterfaceOperationalState::LowerLayerDown => Color::Red,
        InterfaceOperationalState::Dormant | InterfaceOperationalState::Testing => Color::Yellow,
        _ => Color::Gray,
    };
    Span::styled(state.to_string(), Style::new().fg(color))
}

fn draw_details(frame: &mut Frame, area: Rect, iface: &InterfaceStats) {
    let area = command_line(
        frame,
        area,
        &format!("ip -s address show dev {}", iface.name),
        None,
    );
    let block = panel(Line::from(Span::styled(
        format!(" {} ", iface.name),
        Style::new().fg(ACCENT).bold(),
    )));

    let label = |text: &str| Span::raw(format!("{text:<6}")).dark_gray();
    let mut lines = vec![
        Line::from(vec![
            label("State"),
            state_span(iface.state),
            Span::raw("   "),
            label("MTU"),
            Span::raw(iface.mtu.to_string()),
        ]),
        Line::from(vec![
            label("MAC"),
            iface
                .mac
                .map_or_else(|| Span::raw("-").dark_gray(), |m| Span::raw(m.to_string())),
        ]),
        Line::default(),
    ];
    lines.extend(traffic_lines(
        "▼ RX",
        RX_COLOR,
        iface.rx_rate,
        iface.rx_packet_rate,
        iface.total_rx,
        iface.total_rx_errors,
    ));
    lines.extend(traffic_lines(
        "▲ TX",
        TX_COLOR,
        iface.tx_rate,
        iface.tx_packet_rate,
        iface.total_tx,
        iface.total_tx_errors,
    ));
    lines.push(Line::default());
    lines.push(Line::from(Span::raw("Addresses").dark_gray()));
    if iface.addresses.is_empty() {
        lines.push(Line::from(Span::raw("  none").dark_gray()));
    }
    for net in &iface.addresses {
        let family = if net.addr.is_ipv4() { "inet " } else { "inet6" };
        lines.push(Line::from(vec![
            Span::raw(format!("  {family} ")).dark_gray(),
            Span::styled(net.to_string(), Style::new().fg(Color::Yellow)),
        ]));
    }
    frame.render_widget(Paragraph::new(lines).block(block), area);
}

fn traffic_lines(
    label: &str,
    color: Color,
    rate: f64,
    packet_rate: f64,
    total: u64,
    errors: u64,
) -> [Line<'static>; 2] {
    let errors_style = if errors > 0 {
        Style::new().fg(Color::Red).bold()
    } else {
        Style::new().dark_gray()
    };
    [
        Line::from(vec![
            Span::styled(format!("{label:<6}"), Style::new().fg(color).bold()),
            Span::raw(format!("{:>11}", format::rate(rate))).bold(),
            Span::raw(format!("  {packet_rate:>6.0} pkt/s")).dark_gray(),
        ]),
        Line::from(vec![
            Span::raw(format!("{:6}total {}", "", format::bytes(total))).dark_gray(),
            Span::styled(format!(" · {errors} errors"), errors_style),
        ]),
    ]
}

fn draw_chart(frame: &mut Frame, area: Rect, iface: &InterfaceStats, app: &App) {
    let area = command_line(
        frame,
        area,
        &format!("sar -n DEV --iface={} {}", iface.name, app.interval_secs()),
        None,
    );
    let block = panel(Line::from(" Traffic ").bold());
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if let Some(history) = app.metrics.net_iface_history.get(&iface.name) {
        draw_rate_chart(frame, inner, &net_series(history), app);
    }
}

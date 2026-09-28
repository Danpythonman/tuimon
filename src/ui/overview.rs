//! The overview tab: CPU, memory, network and disk panels.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Style, Stylize};
use ratatui::symbols::Marker;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Axis, Chart, Dataset, GraphType, LineGauge, Paragraph};

use super::{ACCENT, command_line, load_style, panel};
use crate::app::App;
use crate::format;
use crate::metrics::{DiskHistory, History, NetHistory};

pub(super) const RX_COLOR: Color = Color::Green;
pub(super) const TX_COLOR: Color = Color::Magenta;
pub(super) const READ_COLOR: Color = Color::LightBlue;
pub(super) const WRITE_COLOR: Color = Color::LightRed;
const SWAP_COLOR: Color = Color::Yellow;
/// Width from which memory, network and disk fit side by side.
const THREE_COLUMNS: u16 = 120;

/// One line of a two-series rate chart.
pub(super) struct Series<'a> {
    pub name: &'static str,
    pub color: Color,
    pub history: &'a History,
}

pub fn draw(frame: &mut Frame, area: Rect, app: &App) {
    let (cpu_area, mem_area, net_area, disk_area) = if area.width >= THREE_COLUMNS {
        let [cpu, bottom] =
            Layout::vertical([Constraint::Percentage(50), Constraint::Percentage(50)]).areas(area);
        let [mem, net, disk] = Layout::horizontal([Constraint::Ratio(1, 3); 3]).areas(bottom);
        (cpu, mem, net, disk)
    } else {
        let [cpu, middle, disk] = Layout::vertical([Constraint::Ratio(1, 3); 3]).areas(area);
        let [mem, net] =
            Layout::horizontal([Constraint::Percentage(50), Constraint::Percentage(50)])
                .areas(middle);
        (cpu, mem, net, disk)
    };

    draw_cpu(frame, cpu_area, app);
    draw_memory(frame, mem_area, app);
    draw_network(frame, net_area, app);
    draw_disk(frame, disk_area, app);
}
fn draw_cpu(frame: &mut Frame, area: Rect, app: &App) {
    let area = command_line(
        frame,
        area,
        &format!("mpstat -P ALL {}", app.interval_secs()),
        Some("model/freq: lscpu"),
    );
    let cpu = &app.metrics.cpu;
    let title = Line::from(vec![
        Span::raw(" CPU ").bold(),
        Span::styled(format!("{:.1}% ", cpu.total), load_style(cpu.total)),
        Span::raw(format!("· {} · {} MHz ", cpu.brand, cpu.frequency)).dark_gray(),
    ]);
    let block = panel(title);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let [chart_area, cores_area] =
        Layout::horizontal([Constraint::Percentage(55), Constraint::Percentage(45)])
            .spacing(1)
            .areas(inner);

    let points = app.metrics.cpu_history.points();
    let dataset = Dataset::default()
        .name("total")
        .marker(Marker::Braille)
        .graph_type(GraphType::Line)
        .style(ACCENT)
        .data(&points);
    let chart = Chart::new(vec![dataset])
        .x_axis(time_axis(&app.metrics.cpu_history, app))
        .y_axis(percent_axis())
        .legend_position(None);
    frame.render_widget(chart, chart_area);

    draw_cores(frame, cores_area, &cpu.per_core);
}

/// Lays out one line gauge per core, wrapping into as many columns as
/// needed to fit the available height.
fn draw_cores(frame: &mut Frame, area: Rect, per_core: &[f32]) {
    if per_core.is_empty() || area.height == 0 {
        return;
    }
    let rows_available = usize::from(area.height);
    let columns = per_core.len().div_ceil(rows_available).max(1);
    let rows = per_core.len().div_ceil(columns);

    let column_areas = Layout::horizontal(vec![Constraint::Fill(1); columns])
        .spacing(2)
        .split(area);
    for (col, chunk) in per_core.chunks(rows).enumerate() {
        let row_areas =
            Layout::vertical(vec![Constraint::Length(1); chunk.len()]).split(column_areas[col]);
        for (row, &usage) in chunk.iter().enumerate() {
            let index = col * rows + row;
            let gauge = LineGauge::default()
                .ratio(f64::from(usage.clamp(0.0, 100.0)) / 100.0)
                .label(Line::from(vec![
                    Span::raw(format!("{index:>2} ")).dark_gray(),
                    Span::styled(format!("{usage:>5.1}%"), load_style(usage)),
                ]))
                .filled_style(load_style(usage))
                .unfilled_style(Style::new().fg(Color::DarkGray))
                .filled_symbol(ratatui::symbols::line::THICK_HORIZONTAL);
            frame.render_widget(gauge, row_areas[row]);
        }
    }
}

fn draw_memory(frame: &mut Frame, area: Rect, app: &App) {
    let area = command_line(
        frame,
        area,
        &format!("free -h -s {}", app.interval_secs()),
        None,
    );
    let mem = &app.metrics.mem;
    let block = panel(Line::from(vec![
        Span::raw(" Memory ").bold(),
        Span::raw(format!("· {} total ", format::bytes(mem.total))).dark_gray(),
    ]));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let [ram_area, swap_area, _, chart_area] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        spacer(inner),
        Constraint::Fill(1),
    ])
    .areas(inner);

    frame.render_widget(
        usage_gauge("RAM", mem.used, mem.total, mem.ram_percent(), ACCENT),
        ram_area,
    );
    frame.render_widget(
        usage_gauge(
            "Swap",
            mem.swap_used,
            mem.swap_total,
            mem.swap_percent(),
            SWAP_COLOR,
        ),
        swap_area,
    );

    let ram_points = app.metrics.ram_history.points();
    let swap_points = app.metrics.swap_history.points();
    let datasets = vec![
        Dataset::default()
            .name("RAM")
            .marker(Marker::Braille)
            .graph_type(GraphType::Line)
            .style(ACCENT)
            .data(&ram_points),
        Dataset::default()
            .name("Swap")
            .marker(Marker::Braille)
            .graph_type(GraphType::Line)
            .style(SWAP_COLOR)
            .data(&swap_points),
    ];
    let chart = Chart::new(datasets)
        .x_axis(time_axis(&app.metrics.ram_history, app))
        .y_axis(percent_axis());
    frame.render_widget(chart, chart_area);
}

fn usage_gauge(
    label: &str,
    used: u64,
    total: u64,
    percent: f64,
    color: Color,
) -> LineGauge<'static> {
    let detail = if total == 0 {
        format!("{:>21}", "n/a")
    } else {
        format!("{:>9} / {:<9}", format::bytes(used), format::bytes(total))
    };
    LineGauge::default()
        .ratio((percent / 100.0).clamp(0.0, 1.0))
        .label(Line::from(vec![
            Span::styled(format!("{label:<5}"), Style::new().fg(color).bold()),
            Span::raw(detail).dark_gray(),
            Span::raw(format!(" {percent:>5.1}% ")).bold(),
        ]))
        .filled_style(Style::new().fg(color))
        .unfilled_style(Style::new().fg(Color::DarkGray))
        .filled_symbol(ratatui::symbols::line::THICK_HORIZONTAL)
}

fn draw_network(frame: &mut Frame, area: Rect, app: &App) {
    let (iface, history) = app.network_view();
    let command = match app.selected_iface {
        None => format!("sar -n DEV {}", app.interval_secs()),
        Some(_) => format!("sar -n DEV --iface={} {}", iface.name, app.interval_secs()),
    };
    let area = command_line(frame, area, &command, None);
    let iface_count = app.metrics.interfaces.len();
    let position = app.selected_iface.map_or_else(
        || "non-loopback total".to_string(),
        |i| format!("{}/{iface_count}", i + 1),
    );

    let block = panel(Line::from(vec![
        Span::raw(" Network ").bold(),
        Span::raw("· ").dark_gray(),
        Span::styled(iface.name.clone(), Style::new().fg(ACCENT).bold()),
        Span::raw(format!(" ({position}) ")).dark_gray(),
    ]));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let [stats_area, _, chart_area] =
        Layout::vertical([Constraint::Length(2), spacer(inner), Constraint::Fill(1)]).areas(inner);

    let stats = vec![
        Line::from(vec![
            Span::styled("▼ RX ", Style::new().fg(RX_COLOR).bold()),
            Span::raw(format!("{:>12}", format::rate(iface.rx_rate))).bold(),
            Span::raw(format!("   total {}", format::bytes(iface.total_rx))).dark_gray(),
        ]),
        Line::from(vec![
            Span::styled("▲ TX ", Style::new().fg(TX_COLOR).bold()),
            Span::raw(format!("{:>12}", format::rate(iface.tx_rate))).bold(),
            Span::raw(format!("   total {}", format::bytes(iface.total_tx))).dark_gray(),
        ]),
    ];
    frame.render_widget(Paragraph::new(stats), stats_area);
    draw_rate_chart(frame, chart_area, &net_series(history), app);
}

pub(super) fn net_series(history: &NetHistory) -> [Series<'_>; 2] {
    [
        Series {
            name: "RX",
            color: RX_COLOR,
            history: &history.rx,
        },
        Series {
            name: "TX",
            color: TX_COLOR,
            history: &history.tx,
        },
    ]
}

pub(super) fn disk_series(history: &DiskHistory) -> [Series<'_>; 2] {
    [
        Series {
            name: "Read",
            color: READ_COLOR,
            history: &history.read,
        },
        Series {
            name: "Write",
            color: WRITE_COLOR,
            history: &history.write,
        },
    ]
}

fn draw_disk(frame: &mut Frame, area: Rect, app: &App) {
    let area = command_line(
        frame,
        area,
        &format!("iostat -dh {}", app.interval_secs()),
        Some("space: df -h"),
    );
    let m = &app.metrics;
    let block = panel(Line::from(vec![
        Span::raw(" Disk ").bold(),
        Span::raw(format!(
            "· {} disk{} ",
            m.disks.len(),
            if m.disks.len() == 1 { "" } else { "s" }
        ))
        .dark_gray(),
    ]));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let [space_area, rates_area, _, chart_area] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        spacer(inner),
        Constraint::Fill(1),
    ])
    .areas(inner);

    let (used, total) = m.space_totals();
    let percent = if total == 0 {
        0.0
    } else {
        used as f64 / total as f64 * 100.0
    };
    frame.render_widget(
        usage_gauge("Used", used, total, percent, ACCENT),
        space_area,
    );
    let rates = Line::from(vec![
        Span::styled("R ", Style::new().fg(READ_COLOR).bold()),
        Span::raw(format!("{:>11}", format::rate(m.disk_total.read_rate))).bold(),
        Span::raw("   "),
        Span::styled("W ", Style::new().fg(WRITE_COLOR).bold()),
        Span::raw(format!("{:>11}", format::rate(m.disk_total.write_rate))).bold(),
    ]);
    frame.render_widget(rates, rates_area);
    draw_rate_chart(frame, chart_area, &disk_series(&m.disk_total_history), app);
}

/// A blank row between a panel's stats and its chart, dropped when the
/// panel is too short to spare it.
fn spacer(inner: Rect) -> Constraint {
    Constraint::Length(u16::from(inner.height >= 10))
}

/// A two-series rate history chart with an auto-scaling y axis.
pub(super) fn draw_rate_chart(frame: &mut Frame, area: Rect, series: &[Series; 2], app: &App) {
    // Scale the y axis to the busiest sample, with a floor so an idle
    // device doesn't render noise as full-height spikes.
    let peak = series
        .iter()
        .map(|s| s.history.max())
        .fold(1024.0, f64::max)
        * 1.1;
    let points: Vec<_> = series.iter().map(|s| s.history.points()).collect();
    let datasets = series
        .iter()
        .zip(&points)
        .map(|(s, points)| {
            Dataset::default()
                .name(s.name)
                .marker(Marker::Braille)
                .graph_type(GraphType::Line)
                .style(s.color)
                .data(points)
        })
        .collect();
    let y_axis = Axis::default()
        .bounds([0.0, peak])
        .style(Style::new().dark_gray())
        .labels([
            Span::raw("0"),
            Span::raw(format::rate(peak / 2.0)),
            Span::raw(format::rate(peak)),
        ]);
    let chart = Chart::new(datasets)
        .x_axis(time_axis(series[0].history, app))
        .y_axis(y_axis);
    frame.render_widget(chart, area);
}

fn time_axis<'a>(history: &History, app: &App) -> Axis<'a> {
    let span = history.capacity() as f64 * app.tick_rate.as_secs_f64();
    Axis::default()
        .bounds([0.0, (history.capacity() - 1) as f64])
        .style(Style::new().dark_gray())
        .labels([Span::raw(format!("-{span:.0}s")), Span::raw("now")])
}

fn percent_axis<'a>() -> Axis<'a> {
    Axis::default()
        .bounds([0.0, 100.0])
        .style(Style::new().dark_gray())
        .labels(["0%", "50%", "100%"])
}

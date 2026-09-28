//! The disks tab: physical disks with their I/O, the filesystems mounted on
//! them, and a read/write chart for the selected disk.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Cell, Paragraph, Row, Table};

use super::overview::{READ_COLOR, WRITE_COLOR, disk_series, draw_rate_chart};
use super::{ACCENT, command_line, load_style, panel, scrollbar};
use crate::app::App;
use crate::disks::{DiskStats, Filesystem};
use crate::format;

/// Rows the bottom panels need to be useful.
const MIN_BOTTOM_HEIGHT: u16 = 10;
/// Width from which the disk table has room for the model column.
const WIDE: u16 = 110;
/// Width from which the filesystem table has room for device and free space.
const FS_WIDE: u16 = 70;
const BAR_WIDTH: usize = 10;

pub fn draw(frame: &mut Frame, area: Rect, app: &mut App) {
    // Command line + borders + header + one row per disk.
    let rows = u16::try_from(app.metrics.disks.len()).unwrap_or(u16::MAX);
    let table_height = rows
        .saturating_add(4)
        .min(area.height.saturating_sub(MIN_BOTTOM_HEIGHT))
        .max(5);
    let [table_area, bottom] =
        Layout::vertical([Constraint::Length(table_height), Constraint::Fill(1)]).areas(area);
    draw_disk_table(frame, table_area, app);

    let [fs_area, chart_area] =
        Layout::horizontal([Constraint::Percentage(55), Constraint::Percentage(45)]).areas(bottom);
    let selected = app.disks.selected(&app.metrics.disks);
    draw_filesystems(frame, fs_area, app, selected.map(|d| d.name.as_str()));
    if let Some(disk) = selected {
        draw_io(frame, chart_area, app, disk);
    }
}

fn draw_disk_table(frame: &mut Frame, area: Rect, app: &mut App) {
    let area = command_line(
        frame,
        area,
        "lsblk -e7 -o NAME,MODEL,SIZE,ROTA,MOUNTPOINTS",
        None,
    );
    let m = &app.metrics;
    let block = panel(Line::from(vec![
        Span::raw(" Disks ").bold(),
        Span::raw(format!("· {} · ", m.disks.len())).dark_gray(),
        Span::styled(
            format!("R {} ", format::rate(m.disk_total.read_rate)),
            Style::new().fg(READ_COLOR),
        ),
        Span::styled(
            format!("W {} ", format::rate(m.disk_total.write_rate)),
            Style::new().fg(WRITE_COLOR),
        ),
    ]));

    if m.disks.is_empty() {
        frame.render_widget(
            Paragraph::new("No disks found in /sys/block.")
                .dark_gray()
                .block(block),
            area,
        );
        return;
    }

    let show_model = area.width >= WIDE;
    let fit = |title: &str, width: usize| {
        Constraint::Length(u16::try_from(width.max(title.len())).unwrap_or(u16::MAX))
    };
    let mut widths = vec![fit(
        "Disk",
        m.disks.iter().map(|d| d.name.len()).max().unwrap_or(0),
    )];
    let mut header = vec![Cell::from("Disk")];
    if show_model {
        widths.push(fit(
            "Model",
            m.disks
                .iter()
                .filter_map(|d| d.model.as_ref().map(String::len))
                .max()
                .unwrap_or(0),
        ));
        header.push(Cell::from("Model"));
    }
    widths.extend([
        Constraint::Length(9),
        Constraint::Length(9),
        Constraint::Length(10),
        Constraint::Length(10),
        Constraint::Length(5),
        Constraint::Fill(1),
    ]);
    header.extend([
        Cell::from("Type"),
        Cell::from(Line::from("Size").right_aligned()),
        Cell::from(Line::from("Read").right_aligned()),
        Cell::from(Line::from("Write").right_aligned()),
        Cell::from(Line::from("Busy").right_aligned()),
        Cell::from("Mounted on"),
    ]);

    let rows = m
        .disks
        .iter()
        .map(|d| disk_row(d, &m.filesystems, show_model));
    let len = m.disks.len();
    let table = Table::new(rows, widths)
        .header(Row::new(header).style(Style::new().fg(ACCENT).bold()))
        .block(block)
        .column_spacing(2)
        .row_highlight_style(Style::new().bg(Color::DarkGray).bold())
        .highlight_symbol("▶ ");
    frame.render_stateful_widget(table, area, &mut app.disks.table);
    app.disks.nav.page = usize::from(area.height.saturating_sub(3));

    // Borders and header take three rows.
    if len > usize::from(area.height.saturating_sub(3)) {
        scrollbar(frame, area, len, app.disks.table.selected().unwrap_or(0));
    }
}

fn disk_row(disk: &DiskStats, filesystems: &[Filesystem], show_model: bool) -> Row<'static> {
    let rate_cell = |rate: f64, color: Color| {
        let style = if rate < 1.0 {
            Style::new().dark_gray()
        } else {
            Style::new().fg(color)
        };
        Cell::from(Line::styled(format::rate(rate), style).right_aligned())
    };
    let mounts: Vec<&str> = filesystems
        .iter()
        .filter(|fs| fs.disk.as_deref() == Some(disk.name.as_str()))
        .map(|fs| fs.mount_point.as_str())
        .collect();
    let mounts = if mounts.is_empty() {
        Span::raw("not mounted").dark_gray()
    } else {
        Span::raw(mounts.join("  "))
    };
    let busy_style = if disk.busy_percent < 1.0 {
        Style::new().dark_gray()
    } else {
        #[allow(clippy::cast_possible_truncation)] // 0–100
        load_style(disk.busy_percent as f32)
    };

    let mut cells = vec![Cell::from(disk.name.clone()).bold()];
    if show_model {
        cells.push(Cell::from(disk.model.clone().unwrap_or_default()));
    }
    cells.extend([
        Cell::from(disk.kind.label()),
        Cell::from(Line::from(format::bytes(disk.size)).right_aligned()),
        rate_cell(disk.read_rate, READ_COLOR),
        rate_cell(disk.write_rate, WRITE_COLOR),
        Cell::from(Line::styled(format!("{:.0}%", disk.busy_percent), busy_style).right_aligned()),
        Cell::from(mounts),
    ]);
    Row::new(cells)
}

fn draw_filesystems(frame: &mut Frame, area: Rect, app: &App, selected_disk: Option<&str>) {
    let area = command_line(frame, area, "df -hT -x tmpfs -x devtmpfs -x squashfs", None);
    let filesystems = &app.metrics.filesystems;
    let (used, total) = app.metrics.space_totals();
    let block = panel(Line::from(vec![
        Span::raw(" Filesystems ").bold(),
        Span::raw(format!(
            "· {} · {} / {} used ",
            filesystems.len(),
            format::bytes(used),
            format::bytes(total)
        ))
        .dark_gray(),
    ]));

    let wide = area.width >= FS_WIDE;
    let mount_width = filesystems
        .iter()
        .map(|fs| fs.mount_point.chars().count())
        .max()
        .unwrap_or(0)
        .clamp("Mount".len(), 24);
    let mut widths = vec![Constraint::Length(u16::try_from(mount_width).unwrap_or(24))];
    let mut header = vec![Cell::from("Mount")];
    if wide {
        let device_width = filesystems
            .iter()
            .map(|fs| short_device(fs).len())
            .max()
            .unwrap_or(0)
            .max("Device".len());
        widths.push(Constraint::Length(
            u16::try_from(device_width).unwrap_or(16),
        ));
        header.push(Cell::from("Device"));
    }
    let fs_width = filesystems
        .iter()
        .map(|fs| fs.fs_type.len())
        .max()
        .unwrap_or(0)
        .max("FS".len());
    widths.extend([
        Constraint::Length(u16::try_from(fs_width).unwrap_or(8)),
        Constraint::Length(9),
    ]);
    header.extend([
        Cell::from("FS"),
        Cell::from(Line::from("Size").right_aligned()),
    ]);
    if wide {
        widths.push(Constraint::Length(9));
        header.push(Cell::from(Line::from("Free").right_aligned()));
    }
    widths.push(Constraint::Fill(1));
    header.push(Cell::from("Used"));

    let rows = filesystems.iter().map(|fs| {
        // Highlight the selected disk's filesystems; dim the rest.
        let on_selected = selected_disk.is_some() && fs.disk.as_deref() == selected_disk;
        let mut cells = vec![Cell::from(fs.mount_point.clone()).bold()];
        if wide {
            cells.push(Cell::from(short_device(fs).to_owned()));
        }
        cells.extend([
            Cell::from(fs.fs_type.clone()),
            Cell::from(Line::from(format::bytes(fs.total)).right_aligned()),
        ]);
        if wide {
            cells.push(Cell::from(
                Line::from(format::bytes(fs.available)).right_aligned(),
            ));
        }
        cells.push(Cell::from(usage_bar(fs.percent(), on_selected)));
        let row = Row::new(cells);
        if on_selected { row } else { row.dark_gray() }
    });
    let table = Table::new(rows, widths)
        .header(Row::new(header).style(Style::new().fg(ACCENT).bold()))
        .block(block)
        .column_spacing(2);
    frame.render_widget(table, area);
}

fn short_device(fs: &Filesystem) -> &str {
    fs.device.strip_prefix("/dev/").unwrap_or(&fs.device)
}

/// The percentage followed by a usage bar, e.g. ` 84% ━━━━━━━━──`. The
/// number comes first so a narrow column cuts off the bar instead.
fn usage_bar(percent: f64, colored: bool) -> Line<'static> {
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // 0–BAR_WIDTH
    let filled = ((percent / 100.0 * BAR_WIDTH as f64).round() as usize).min(BAR_WIDTH);
    #[allow(clippy::cast_possible_truncation)] // 0–100
    let style = if colored {
        load_style(percent as f32)
    } else {
        Style::new()
    };
    Line::from(vec![
        Span::styled(format!("{percent:>3.0}% "), style),
        Span::styled("━".repeat(filled), style),
        Span::raw("─".repeat(BAR_WIDTH - filled)).dark_gray(),
    ])
}

fn draw_io(frame: &mut Frame, area: Rect, app: &App, disk: &DiskStats) {
    let area = command_line(
        frame,
        area,
        &format!("iostat -dxh {} {}", disk.name, app.interval_secs()),
        None,
    );
    let block = panel(Line::from(vec![
        Span::raw(" I/O ").bold(),
        Span::styled(format!("{} ", disk.name), Style::new().fg(ACCENT).bold()),
    ]));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let [stats_area, _, chart_area] = Layout::vertical([
        Constraint::Length(2),
        Constraint::Length(1),
        Constraint::Fill(1),
    ])
    .areas(inner);
    // Totals only when they fit, rather than cutting them off mid-word.
    let show_totals = inner.width >= 45;
    let line = |label: &str, color: Color, rate: f64, ops: f64, total: u64| {
        let mut spans = vec![
            Span::styled(format!("{label:<6}"), Style::new().fg(color).bold()),
            Span::raw(format!("{:>10}", format::rate(rate))).bold(),
            Span::raw(format!("  {ops:>4.0} op/s")).dark_gray(),
        ];
        if show_totals {
            spans.push(Span::raw(format!("  total {}", format::bytes(total))).dark_gray());
        }
        Line::from(spans)
    };
    frame.render_widget(
        Paragraph::new(vec![
            line(
                "Read",
                READ_COLOR,
                disk.read_rate,
                disk.read_ops,
                disk.total_read,
            ),
            line(
                "Write",
                WRITE_COLOR,
                disk.write_rate,
                disk.write_ops,
                disk.total_written,
            ),
        ]),
        stats_area,
    );
    if let Some(history) = app.metrics.disk_history.get(&disk.name) {
        draw_rate_chart(frame, chart_area, &disk_series(history), app);
    }
}

//! `tuimon` — a terminal dashboard for CPU, memory, network, disks, processes, ports and system messages.

mod app;
mod clipboard;
mod disks;
mod format;
mod journal;
mod metrics;
mod ports;
mod processes;
mod ui;
mod view;

use std::time::Duration;

use anyhow::Result;
use clap::Parser;

use crate::app::App;

/// A terminal dashboard showing live CPU, memory, network, disks, processes, ports and system messages.
#[derive(Debug, Parser)]
#[command(version, about)]
struct Cli {
    /// Refresh interval in milliseconds (250–10000).
    #[arg(short, long, default_value_t = 1000)]
    tick_rate: u64,

    /// Number of samples kept for the history charts.
    #[arg(short = 'H', long, default_value_t = 120, value_parser = clap::value_parser!(u16).range(10..))]
    history: u16,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let app = App::new(
        Duration::from_millis(cli.tick_rate),
        usize::from(cli.history),
    );
    // `ratatui::run` sets up the terminal and restores it afterwards,
    // including on panic.
    ratatui::run(|terminal| app.run(terminal))
}

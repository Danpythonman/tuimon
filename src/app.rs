//! Application state and the main event loop.

use std::time::{Duration, Instant};

use anyhow::Result;
use ratatui::DefaultTerminal;
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEvent, KeyModifiers};

use crate::clipboard::{self, Method};
use crate::metrics::{InterfaceStats, Metrics, NetHistory};
use crate::ui;
use crate::view::{LogsView, NamedList, PortsView, ProcessesView};

const TICK_STEP: Duration = Duration::from_millis(250);
const MIN_TICK: Duration = Duration::from_millis(250);
const MAX_TICK: Duration = Duration::from_secs(10);
/// How often new journal entries are picked up while the messages tab is
/// open, independent of the refresh rate.
const LOG_POLL: Duration = Duration::from_millis(200);
/// How long a status message stays in the footer.
const STATUS_DURATION: Duration = Duration::from_secs(3);

/// A short-lived message shown in place of the footer's key hints.
pub struct Status {
    pub text: String,
    pub is_error: bool,
    shown_at: Instant,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Overview,
    Network,
    Disks,
    Processes,
    Ports,
    Messages,
}

impl Tab {
    pub const ALL: [Self; 6] = [
        Self::Overview,
        Self::Network,
        Self::Disks,
        Self::Processes,
        Self::Ports,
        Self::Messages,
    ];

    pub fn title(self) -> &'static str {
        match self {
            Self::Overview => "Overview",
            Self::Network => "Network",
            Self::Disks => "Disks",
            Self::Processes => "Processes",
            Self::Ports => "Ports",
            Self::Messages => "Messages",
        }
    }

    pub fn index(self) -> usize {
        Self::ALL.iter().position(|&t| t == self).unwrap_or(0)
    }

    fn cycle(self, forward: bool) -> Self {
        let len = Self::ALL.len();
        let step = if forward { 1 } else { len - 1 };
        Self::ALL[(self.index() + step) % len]
    }
}

pub struct App {
    pub metrics: Metrics,
    pub network: NamedList,
    pub disks: NamedList,
    pub processes: ProcessesView,
    pub ports: PortsView,
    pub logs: LogsView,
    pub tab: Tab,
    pub tick_rate: Duration,
    pub paused: bool,
    /// `None` shows the aggregate of all interfaces; `Some(i)` indexes
    /// into `metrics.interfaces`.
    pub selected_iface: Option<usize>,
    status: Option<Status>,
    running: bool,
}

impl App {
    pub fn new(tick_rate: Duration, history_len: usize) -> Self {
        let metrics = Metrics::new(history_len);
        let mut network = NamedList::default();
        network.sync(&metrics.interfaces);
        let mut disks = NamedList::default();
        disks.sync(&metrics.disks);
        Self {
            metrics,
            network,
            disks,
            processes: ProcessesView::new(),
            ports: PortsView::default(),
            logs: LogsView::new(),
            tab: Tab::Overview,
            tick_rate: tick_rate.clamp(MIN_TICK, MAX_TICK),
            paused: false,
            selected_iface: None,
            status: None,
            running: true,
        }
    }

    pub fn run(mut self, terminal: &mut DefaultTerminal) -> Result<()> {
        let mut last_tick = Instant::now();
        while self.running {
            terminal.draw(|frame| ui::draw(frame, &mut self))?;

            let mut timeout = self.tick_rate.saturating_sub(last_tick.elapsed());
            if self.tab == Tab::Messages {
                timeout = timeout.min(LOG_POLL);
            }
            if let Some(status) = &self.status {
                // Wake up to clear the message once it expires.
                timeout = timeout.min(STATUS_DURATION.saturating_sub(status.shown_at.elapsed()));
            }
            if event::poll(timeout)? {
                self.handle_event(&event::read()?);
            }
            if !self.paused {
                self.logs.poll();
            }
            if last_tick.elapsed() >= self.tick_rate {
                if !self.paused {
                    self.refresh();
                }
                last_tick = Instant::now();
            }
        }
        Ok(())
    }

    fn refresh(&mut self) {
        self.metrics.refresh();
        self.clamp_iface();
        self.network.sync(&self.metrics.interfaces);
        self.disks.sync(&self.metrics.disks);
        // Walking /proc for processes and socket owners isn't free, so only
        // do it while the relevant tab is visible.
        match self.tab {
            Tab::Processes => self.processes.refresh(self.metrics.mem.total),
            Tab::Ports => self.ports.refresh(),
            Tab::Overview | Tab::Network | Tab::Disks | Tab::Messages => {}
        }
    }

    /// Interval in whole seconds, as the equivalent CLI tools expect.
    pub fn interval_secs(&self) -> u64 {
        (self.tick_rate + Duration::from_millis(500))
            .as_secs()
            .max(1)
    }

    /// The network interface currently shown, with its history.
    pub fn network_view(&self) -> (&InterfaceStats, &NetHistory) {
        let m = &self.metrics;
        self.selected_iface
            .and_then(|i| m.interfaces.get(i))
            .and_then(|iface| Some((iface, m.net_iface_history.get(&iface.name)?)))
            .unwrap_or((&m.net_total, &m.net_total_history))
    }

    /// The status message, if it hasn't expired yet.
    pub fn status(&self) -> Option<&Status> {
        self.status
            .as_ref()
            .filter(|s| s.shown_at.elapsed() < STATUS_DURATION)
    }

    fn set_status(&mut self, text: String, is_error: bool) {
        self.status = Some(Status {
            text,
            is_error,
            shown_at: Instant::now(),
        });
    }

    /// Copies the selected interface's IPv4 or IPv6 address.
    fn copy_address(&mut self, ipv6: bool) {
        let family = if ipv6 { "IPv6" } else { "IPv4" };
        let Some(iface) = self.network.selected(&self.metrics.interfaces) else {
            return;
        };
        let name = iface.name.clone();
        let Some(addr) = iface.primary_address(ipv6) else {
            self.set_status(format!("{name} has no {family} address"), true);
            return;
        };
        match clipboard::copy(&addr.to_string()) {
            Ok(Method::Tool(_)) => self.set_status(format!("Copied {addr} ({name})"), false),
            Ok(Method::Osc52) => self.set_status(
                format!("Sent {addr} to the terminal clipboard (needs OSC 52 support)"),
                false,
            ),
            Err(e) => self.set_status(format!("Could not copy {addr}: {e}"), true),
        }
    }

    /// Whether a text filter is being typed, so the footer can show that.
    pub fn is_editing_filter(&self) -> bool {
        match self.tab {
            Tab::Processes => self.processes.filter.editing,
            Tab::Messages => self.logs.filter.editing,
            Tab::Overview | Tab::Network | Tab::Disks | Tab::Ports => false,
        }
    }

    fn handle_event(&mut self, event: &Event) {
        if let Some(key) = event.as_key_press_event() {
            self.handle_key(&key);
        }
    }

    fn set_tab(&mut self, tab: Tab) {
        self.tab = tab;
        // Load immediately rather than waiting for the next tick.
        match tab {
            Tab::Processes => self.processes.refresh(self.metrics.mem.total),
            Tab::Ports => self.ports.refresh(),
            Tab::Messages => {
                self.logs.start();
                self.logs.poll();
            }
            Tab::Overview | Tab::Network | Tab::Disks => {}
        }
    }

    fn handle_key(&mut self, key: &KeyEvent) {
        if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
            self.running = false;
            return;
        }
        let handled = match self.tab {
            Tab::Network => match key.code {
                KeyCode::Char('c') => {
                    self.copy_address(false);
                    true
                }
                KeyCode::Char('C') => {
                    self.copy_address(true);
                    true
                }
                _ => self.network.handle_key(key, &self.metrics.interfaces),
            },
            Tab::Disks => self.disks.handle_key(key, &self.metrics.disks),
            Tab::Processes => self.processes.handle_key(key),
            Tab::Ports => self.ports.handle_key(key),
            Tab::Messages => self.logs.handle_key(key),
            Tab::Overview => false,
        };
        if handled {
            return;
        }
        match key.code {
            KeyCode::Char('q') | KeyCode::Esc => self.running = false,
            KeyCode::Char('p' | ' ') => self.paused = !self.paused,
            KeyCode::Char('+' | '=') => {
                self.tick_rate = (self.tick_rate + TICK_STEP).min(MAX_TICK);
            }
            KeyCode::Char('-' | '_') => {
                self.tick_rate = self.tick_rate.saturating_sub(TICK_STEP).max(MIN_TICK);
            }
            KeyCode::Tab => self.set_tab(self.tab.cycle(true)),
            KeyCode::BackTab => self.set_tab(self.tab.cycle(false)),
            KeyCode::Char(c @ '1'..='9') => {
                if let Some(&tab) = Tab::ALL.get(usize::from(c as u8 - b'1')) {
                    self.set_tab(tab);
                }
            }
            KeyCode::Char('n') if self.tab == Tab::Overview => self.cycle_iface(true),
            KeyCode::Char('N') if self.tab == Tab::Overview => self.cycle_iface(false),
            _ => {}
        }
    }

    /// Cycles through `all -> iface 0 -> iface 1 -> ... -> all`.
    fn cycle_iface(&mut self, forward: bool) {
        let count = self.metrics.interfaces.len();
        if count == 0 {
            self.selected_iface = None;
            return;
        }
        self.selected_iface = match (self.selected_iface, forward) {
            (None, true) => Some(0),
            (None, false) => Some(count - 1),
            (Some(i), true) if i + 1 < count => Some(i + 1),
            (Some(i), false) if i > 0 => Some(i - 1),
            (Some(_), _) => None,
        };
    }

    fn clamp_iface(&mut self) {
        if self
            .selected_iface
            .is_some_and(|i| i >= self.metrics.interfaces.len())
        {
            self.selected_iface = None;
        }
    }
}

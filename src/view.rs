//! State for the list-based tabs (network, disks, processes, ports, messages): selection,
//! filtering, sorting and key handling.

use std::collections::VecDeque;

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::widgets::TableState;

use crate::disks::DiskStats;
use crate::journal::{Journal, LogEntry, PRIORITY_NAMES};
use crate::metrics::{InterfaceStats, LOOPBACK};
use crate::ports::{self, Socket};
use crate::processes::{self, ProcessInfo, ProcessList, SortKey};

/// What a navigation key asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Nav {
    /// Move the selection by this many rows (saturating at either end).
    Move(isize),
    /// The first `g` of `gg`: consumed, waiting for the next key.
    Pending,
}

/// Vim-style list navigation shared by every list tab: `j`/`k`, `gg`/`G`,
/// `Ctrl-D`/`Ctrl-U` (half page), `Ctrl-F`/`Ctrl-B` (full page) and
/// `Ctrl-N`/`Ctrl-P`, alongside the arrow, paging, `Home` and `End` keys.
#[derive(Debug)]
pub struct ListNav {
    /// Rows the list shows at once, updated whenever it's drawn.
    pub page: usize,
    pending_g: bool,
}

impl Default for ListNav {
    fn default() -> Self {
        Self {
            page: 10,
            pending_g: false,
        }
    }
}

impl ListNav {
    /// Returns `None` for keys that aren't navigation, which also cancels a
    /// pending `g`.
    fn handle(&mut self, key: &KeyEvent) -> Option<Nav> {
        let pending_g = std::mem::take(&mut self.pending_g);
        let page = isize::try_from(self.page.max(1)).unwrap_or(isize::MAX);
        let half = (page / 2).max(1);
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let delta = match (key.code, ctrl) {
            (KeyCode::Down, _) | (KeyCode::Char('j'), false) | (KeyCode::Char('n'), true) => 1,
            (KeyCode::Up, _) | (KeyCode::Char('k'), false) | (KeyCode::Char('p'), true) => -1,
            (KeyCode::Char('d'), true) => half,
            (KeyCode::Char('u'), true) => -half,
            (KeyCode::PageDown, _) | (KeyCode::Char('f'), true) => page,
            (KeyCode::PageUp, _) | (KeyCode::Char('b'), true) => -page,
            (KeyCode::Home, _) => isize::MIN,
            (KeyCode::Char('g'), false) if pending_g => isize::MIN,
            (KeyCode::Char('g'), false) => {
                self.pending_g = true;
                return Some(Nav::Pending);
            }
            (KeyCode::End, _) | (KeyCode::Char('G'), false) => isize::MAX,
            _ => return None,
        };
        Some(Nav::Move(delta))
    }

    fn cancel(&mut self) {
        self.pending_g = false;
    }
}

fn move_selection(table: &mut TableState, len: usize, delta: isize) {
    if len == 0 {
        table.select(None);
        return;
    }
    let current = table.selected().unwrap_or(0);
    table.select(Some(current.saturating_add_signed(delta).min(len - 1)));
}

/// Keeps the selection inside `0..len`, selecting the first row if nothing
/// is selected yet.
fn clamp_selection(table: &mut TableState, len: usize) {
    let selected = match (len, table.selected()) {
        (0, _) => None,
        (_, None) => Some(0),
        (len, Some(i)) => Some(i.min(len - 1)),
    };
    table.select(selected);
}

/// A case-insensitive substring filter typed after pressing `/`.
#[derive(Debug, Default)]
pub struct TextFilter {
    query: String,
    lowercase: String,
    pub editing: bool,
}

impl TextFilter {
    pub fn query(&self) -> &str {
        &self.query
    }

    pub fn is_active(&self) -> bool {
        !self.query.is_empty()
    }

    pub fn matches(&self, fields: &[&str]) -> bool {
        self.lowercase.is_empty()
            || fields
                .iter()
                .any(|f| f.to_lowercase().contains(&self.lowercase))
    }

    /// Handles `/` to start editing, typing while editing, and `Esc` to
    /// clear an active filter. Returns whether the key was consumed.
    fn handle_key(&mut self, key: &KeyEvent) -> bool {
        if !self.editing {
            return match key.code {
                KeyCode::Char('/') => {
                    self.editing = true;
                    true
                }
                KeyCode::Esc if self.is_active() => {
                    self.set(String::new());
                    true
                }
                _ => false,
            };
        }
        if key.modifiers.contains(KeyModifiers::CONTROL) {
            return false; // Let Ctrl-C through.
        }
        match key.code {
            KeyCode::Enter => self.editing = false,
            KeyCode::Esc => {
                self.editing = false;
                self.set(String::new());
            }
            KeyCode::Backspace => {
                let mut query = std::mem::take(&mut self.query);
                query.pop();
                self.set(query);
            }
            KeyCode::Char(c) => {
                let mut query = std::mem::take(&mut self.query);
                query.push(c);
                self.set(query);
            }
            _ => {}
        }
        true
    }

    fn set(&mut self, query: String) {
        self.lowercase = query.to_lowercase();
        self.query = query;
    }
}

/// An item in a [`NamedList`].
pub trait Named {
    fn name(&self) -> &str;
    /// Bytes moved so far, used to pick the initial selection. `None`
    /// excludes the item from being picked.
    fn activity(&self) -> Option<u64>;
}

impl Named for InterfaceStats {
    fn name(&self) -> &str {
        &self.name
    }

    fn activity(&self) -> Option<u64> {
        (self.name != LOOPBACK).then_some(self.total_rx + self.total_tx)
    }
}

impl Named for DiskStats {
    fn name(&self) -> &str {
        &self.name
    }

    fn activity(&self) -> Option<u64> {
        Some(self.total_read + self.total_written)
    }
}

/// Selection for the network and disks tabs. The items themselves live in
/// `Metrics`; this tracks the selected one by name so it stays put as
/// others appear and disappear.
#[derive(Debug, Default)]
pub struct NamedList {
    pub table: TableState,
    pub nav: ListNav,
    selected_name: Option<String>,
}

impl NamedList {
    /// Re-selects the tracked item, or the nearest row if it's gone.
    pub fn sync<T: Named>(&mut self, items: &[T]) {
        let index = self
            .selected_name
            .as_deref()
            .and_then(|name| items.iter().position(|i| i.name() == name));
        if let Some(index) = index {
            self.table.select(Some(index));
        } else if self.table.selected().is_none() {
            // Start on the busiest item, which is usually the one people
            // care about.
            let busiest = items
                .iter()
                .enumerate()
                .filter_map(|(index, i)| Some((index, i.activity()?)))
                .max_by_key(|&(_, activity)| activity)
                .map(|(index, _)| index);
            self.table.select(busiest.or(items.first().map(|_| 0)));
            self.remember(items);
        } else {
            clamp_selection(&mut self.table, items.len());
            self.remember(items);
        }
    }

    pub fn selected<'a, T>(&self, items: &'a [T]) -> Option<&'a T> {
        self.table.selected().and_then(|i| items.get(i))
    }

    fn remember<T: Named>(&mut self, items: &[T]) {
        self.selected_name = self.selected(items).map(|i| i.name().to_owned());
    }

    /// Returns whether the key was handled.
    pub fn handle_key<T: Named>(&mut self, key: &KeyEvent, items: &[T]) -> bool {
        match self.nav.handle(key) {
            Some(Nav::Move(delta)) => {
                move_selection(&mut self.table, items.len(), delta);
                self.remember(items);
            }
            Some(Nav::Pending) => {}
            None => return false,
        }
        true
    }
}

/// The ports tab.
#[derive(Debug, Default)]
pub struct PortsView {
    sockets: Vec<Socket>,
    /// Show every socket rather than just listening ones.
    pub show_all: bool,
    pub table: TableState,
    pub nav: ListNav,
    pub error: Option<String>,
}

impl PortsView {
    /// Sockets matching the current filter.
    pub fn visible(&self) -> Vec<&Socket> {
        self.sockets
            .iter()
            .filter(|s| self.show_all || s.is_listening())
            .collect()
    }

    pub fn refresh(&mut self) {
        match ports::list() {
            Ok(sockets) => {
                self.sockets = sockets;
                self.error = None;
            }
            Err(e) => self.error = Some(e.to_string()),
        }
        let len = self.visible().len();
        clamp_selection(&mut self.table, len);
    }

    /// Returns whether the key was handled.
    pub fn handle_key(&mut self, key: &KeyEvent) -> bool {
        match self.nav.handle(key) {
            Some(Nav::Move(delta)) => {
                let len = self.visible().len();
                move_selection(&mut self.table, len, delta);
            }
            Some(Nav::Pending) => return true,
            None if key.code == KeyCode::Char('a') => {
                self.show_all = !self.show_all;
                self.table.select_first();
            }
            None => return false,
        }
        let len = self.visible().len();
        clamp_selection(&mut self.table, len);
        true
    }
}

/// The processes tab.
pub struct ProcessesView {
    list: ProcessList,
    /// All processes, kept sorted.
    processes: Vec<ProcessInfo>,
    pub sort: SortKey,
    /// Flip the sort key's natural direction.
    pub reversed: bool,
    pub filter: TextFilter,
    pub table: TableState,
    pub nav: ListNav,
    /// The selection tracks a PID so it stays on the same process as rows
    /// move around between refreshes.
    selected_pid: Option<u32>,
}

impl ProcessesView {
    pub fn new() -> Self {
        Self {
            list: ProcessList::new(),
            processes: Vec::new(),
            sort: SortKey::Cpu,
            reversed: false,
            filter: TextFilter::default(),
            table: TableState::default(),
            nav: ListNav::default(),
            selected_pid: None,
        }
    }

    pub fn descending(&self) -> bool {
        self.sort.descending_by_default() != self.reversed
    }

    pub fn total(&self) -> usize {
        self.processes.len()
    }

    pub fn visible(&self) -> Vec<&ProcessInfo> {
        self.processes
            .iter()
            .filter(|p| {
                self.filter
                    .matches(&[&p.name, &p.command, &p.user, &p.pid.to_string()])
            })
            .collect()
    }

    pub fn refresh(&mut self, total_memory: u64) {
        self.processes = self.list.refresh(total_memory);
        self.resort();
    }

    fn resort(&mut self) {
        let descending = self.descending();
        processes::sort(&mut self.processes, self.sort, descending);
        self.restore_selection();
    }

    /// Re-selects the tracked PID, or the nearest row if it's gone.
    fn restore_selection(&mut self) {
        let pids = self.visible_pids();
        let index = self
            .selected_pid
            .and_then(|pid| pids.iter().position(|&p| p == pid));
        if let Some(index) = index {
            self.table.select(Some(index));
        } else {
            clamp_selection(&mut self.table, pids.len());
            self.selected_pid = self.table.selected().map(|i| pids[i]);
        }
    }

    fn visible_pids(&self) -> Vec<u32> {
        self.visible().iter().map(|p| p.pid).collect()
    }

    /// Returns whether the key was handled.
    pub fn handle_key(&mut self, key: &KeyEvent) -> bool {
        if self.filter.handle_key(key) {
            self.nav.cancel();
            self.restore_selection();
            return true;
        }
        match self.nav.handle(key) {
            Some(Nav::Move(delta)) => {
                let pids = self.visible_pids();
                move_selection(&mut self.table, pids.len(), delta);
                self.selected_pid = self.table.selected().map(|i| pids[i]);
            }
            Some(Nav::Pending) => {}
            None => match key.code {
                KeyCode::Char('s') => {
                    self.sort = self.sort.next();
                    self.reversed = false;
                    self.resort();
                }
                KeyCode::Char('r') => {
                    self.reversed = !self.reversed;
                    self.resort();
                }
                _ => return false,
            },
        }
        true
    }
}

/// Entries requested from the journal on startup.
const BACKLOG: usize = 1_000;
/// Oldest entries are dropped beyond this.
const MAX_ENTRIES: usize = 10_000;
/// Minimum-priority filter levels cycled with `l` (7 = show everything).
const PRIORITY_LEVELS: [u8; 4] = [7, 5, 4, 3];

/// The system messages tab.
pub struct LogsView {
    journal: Option<Journal>,
    entries: VecDeque<LogEntry>,
    /// Only show entries at this priority or more severe.
    pub min_priority: u8,
    /// Only show kernel messages, like `dmesg`.
    pub kernel_only: bool,
    pub filter: TextFilter,
    pub table: TableState,
    pub nav: ListNav,
    /// Keep the newest entry selected as messages arrive, like `tail -f`.
    pub follow: bool,
    pub error: Option<String>,
}

impl LogsView {
    pub fn new() -> Self {
        Self {
            journal: None,
            entries: VecDeque::new(),
            min_priority: 7,
            kernel_only: false,
            filter: TextFilter::default(),
            table: TableState::default(),
            nav: ListNav::default(),
            follow: true,
            error: None,
        }
    }

    /// Starts `journalctl` the first time the tab is opened.
    pub fn start(&mut self) {
        if self.journal.is_some() || self.error.is_some() {
            return;
        }
        match Journal::spawn(BACKLOG) {
            Ok(journal) => self.journal = Some(journal),
            Err(e) => self.error = Some(e.to_string()),
        }
    }

    /// Pulls in any newly arrived entries.
    pub fn poll(&mut self) {
        let Some(journal) = &mut self.journal else {
            return;
        };
        self.entries.extend(journal.drain());
        let excess = self.entries.len().saturating_sub(MAX_ENTRIES);
        self.entries.drain(..excess);
        if journal.has_ended() && self.entries.is_empty() {
            self.error = Some(
                "journalctl exited without output. Your user may need to be in the \
                 `systemd-journal` or `adm` group to read system messages."
                    .into(),
            );
        }
        self.sync_selection();
    }

    pub fn total(&self) -> usize {
        self.entries.len()
    }

    pub fn visible(&self) -> Vec<&LogEntry> {
        self.entries
            .iter()
            .filter(|e| {
                e.priority <= self.min_priority
                    && (!self.kernel_only || e.kernel)
                    && self.filter.matches(&[&e.message, &e.source])
            })
            .collect()
    }

    /// Label for the current minimum-priority filter.
    pub fn level_label(&self) -> String {
        if self.min_priority >= 7 {
            "all".into()
        } else {
            format!("{}+", PRIORITY_NAMES[usize::from(self.min_priority)])
        }
    }

    fn sync_selection(&mut self) {
        let len = self.visible().len();
        if self.follow {
            self.table.select(len.checked_sub(1));
        } else {
            clamp_selection(&mut self.table, len);
        }
    }

    /// Returns whether the key was handled.
    pub fn handle_key(&mut self, key: &KeyEvent) -> bool {
        if self.filter.handle_key(key) {
            self.nav.cancel();
            self.sync_selection();
            return true;
        }
        match self.nav.handle(key) {
            Some(Nav::Move(delta)) => {
                let len = self.visible().len();
                move_selection(&mut self.table, len, delta);
                // Reaching the bottom resumes following; moving up pauses it.
                self.follow = self.table.selected().is_some_and(|i| i + 1 >= len);
            }
            Some(Nav::Pending) => return true,
            None => match key.code {
                KeyCode::Char('l') => {
                    let i = PRIORITY_LEVELS
                        .iter()
                        .position(|&p| p == self.min_priority)
                        .unwrap_or(0);
                    self.min_priority = PRIORITY_LEVELS[(i + 1) % PRIORITY_LEVELS.len()];
                }
                KeyCode::Char('d') => self.kernel_only = !self.kernel_only,
                KeyCode::Char('f') => self.follow = !self.follow,
                _ => return false,
            },
        }
        self.sync_selection();
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn press(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn type_str(filter: &mut TextFilter, s: &str) {
        for c in s.chars() {
            filter.handle_key(&press(KeyCode::Char(c)));
        }
    }

    #[test]
    fn filter_editing() {
        let mut filter = TextFilter::default();
        assert!(!filter.handle_key(&press(KeyCode::Char('x'))));
        assert!(filter.handle_key(&press(KeyCode::Char('/'))));
        type_str(&mut filter, "SshX");
        filter.handle_key(&press(KeyCode::Backspace));
        filter.handle_key(&press(KeyCode::Enter));
        assert!(!filter.editing);
        assert_eq!(filter.query(), "Ssh");
        assert!(filter.matches(&["nope", "/usr/sbin/sshd"]));
        assert!(!filter.matches(&["bash"]));
        // Esc clears an active filter even when not editing.
        assert!(filter.handle_key(&press(KeyCode::Esc)));
        assert!(!filter.is_active());
        assert!(filter.matches(&["anything"]));
    }

    fn ctrl(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
    }

    #[test]
    fn vim_navigation() {
        let mut nav = ListNav {
            page: 20,
            ..Default::default()
        };
        assert_eq!(nav.handle(&press(KeyCode::Char('j'))), Some(Nav::Move(1)));
        assert_eq!(nav.handle(&ctrl('d')), Some(Nav::Move(10)));
        assert_eq!(nav.handle(&ctrl('u')), Some(Nav::Move(-10)));
        assert_eq!(nav.handle(&ctrl('f')), Some(Nav::Move(20)));
        assert_eq!(nav.handle(&ctrl('b')), Some(Nav::Move(-20)));
        assert_eq!(
            nav.handle(&press(KeyCode::Char('G'))),
            Some(Nav::Move(isize::MAX))
        );
        // `gg` jumps to the top; a lone `g` waits.
        assert_eq!(nav.handle(&press(KeyCode::Char('g'))), Some(Nav::Pending));
        assert_eq!(
            nav.handle(&press(KeyCode::Char('g'))),
            Some(Nav::Move(isize::MIN))
        );
        // Any other key in between cancels the pending `g`.
        nav.handle(&press(KeyCode::Char('g')));
        assert_eq!(nav.handle(&press(KeyCode::Char('x'))), None);
        assert_eq!(nav.handle(&press(KeyCode::Char('g'))), Some(Nav::Pending));
        // Plain `d`/`f` are left for the tabs' own keys.
        assert_eq!(nav.handle(&press(KeyCode::Char('d'))), None);
        // Half a page is never zero rows.
        nav.page = 1;
        assert_eq!(nav.handle(&ctrl('d')), Some(Nav::Move(1)));
    }

    #[test]
    fn selection_moves_and_clamps() {
        let mut table = TableState::default();
        clamp_selection(&mut table, 5);
        assert_eq!(table.selected(), Some(0));
        move_selection(&mut table, 5, -1);
        assert_eq!(table.selected(), Some(0));
        move_selection(&mut table, 5, isize::MAX);
        assert_eq!(table.selected(), Some(4));
        clamp_selection(&mut table, 2);
        assert_eq!(table.selected(), Some(1));
        clamp_selection(&mut table, 0);
        assert_eq!(table.selected(), None);
    }

    fn iface(name: &str) -> InterfaceStats {
        InterfaceStats {
            name: name.into(),
            ..Default::default()
        }
    }

    #[test]
    fn network_selection_follows_name() {
        let mut view = NamedList::default();
        let mut busy = iface("wlan0");
        busy.total_rx = 1_000;
        let mut lo = iface("lo");
        lo.total_rx = 5_000;
        let before = [iface("eth0"), lo, busy];
        // Starts on the busiest non-loopback interface.
        view.sync(&before);
        assert_eq!(view.table.selected(), Some(2));
        // A new interface sorts ahead of the selected one.
        let after = [iface("docker0"), iface("eth0"), iface("lo"), iface("wlan0")];
        view.sync(&after);
        assert_eq!(view.table.selected(), Some(3));
        // The selected interface disappears: stay in range.
        view.sync(&after[..2]);
        assert_eq!(view.table.selected(), Some(1));
    }
}

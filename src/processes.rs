//! Running processes, similar to what `top` shows.

use std::cmp::Ordering;

use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, System, ThreadKind, UpdateKind, Users};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortKey {
    Cpu,
    Memory,
    Pid,
    Name,
}

impl SortKey {
    const ALL: [Self; 4] = [Self::Cpu, Self::Memory, Self::Pid, Self::Name];

    pub fn next(self) -> Self {
        let i = Self::ALL.iter().position(|&k| k == self).unwrap_or(0);
        Self::ALL[(i + 1) % Self::ALL.len()]
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Cpu => "CPU%",
            Self::Memory => "Memory",
            Self::Pid => "PID",
            Self::Name => "Name",
        }
    }

    /// The field name `top -o` uses for this sort.
    pub fn top_field(self) -> &'static str {
        match self {
            Self::Cpu => "%CPU",
            Self::Memory => "%MEM",
            Self::Pid => "PID",
            Self::Name => "COMMAND",
        }
    }

    /// Numeric columns read best biggest-first; text and IDs ascending.
    pub fn descending_by_default(self) -> bool {
        matches!(self, Self::Cpu | Self::Memory)
    }
}

#[derive(Debug, Clone)]
pub struct ProcessInfo {
    pub pid: u32,
    pub name: String,
    /// Full command line, or the name for kernel threads that have none.
    pub command: String,
    pub user: String,
    /// Percent of one core, so it can exceed 100 (same as `top`).
    pub cpu: f32,
    /// Resident memory in bytes.
    pub memory: u64,
    pub mem_percent: f64,
    pub status: String,
    /// Seconds since the process started.
    pub run_time: u64,
}

impl ProcessInfo {
    fn compare(&self, other: &Self, key: SortKey) -> Ordering {
        match key {
            SortKey::Cpu => self.cpu.total_cmp(&other.cpu),
            SortKey::Memory => self.memory.cmp(&other.memory),
            SortKey::Pid => self.pid.cmp(&other.pid),
            SortKey::Name => self.name.to_lowercase().cmp(&other.name.to_lowercase()),
        }
        // Tie-break on PID so equal rows don't shuffle between refreshes.
        .then(self.pid.cmp(&other.pid))
    }
}

pub fn sort(processes: &mut [ProcessInfo], key: SortKey, descending: bool) {
    processes.sort_by(|a, b| {
        let ord = a.compare(b, key);
        if descending { ord.reverse() } else { ord }
    });
}

pub struct ProcessList {
    sys: System,
    users: Users,
}

impl ProcessList {
    pub fn new() -> Self {
        Self {
            sys: System::new(),
            users: Users::new_with_refreshed_list(),
        }
    }

    /// Re-reads all processes. CPU usage is measured since the previous call,
    /// so the very first call reports 0% for everything.
    pub fn refresh(&mut self, total_memory: u64) -> Vec<ProcessInfo> {
        self.sys.refresh_processes_specifics(
            ProcessesToUpdate::All,
            true,
            ProcessRefreshKind::nothing()
                .with_cpu()
                .with_memory()
                .with_cmd(UpdateKind::OnlyIfNotSet)
                .with_user(UpdateKind::OnlyIfNotSet),
        );

        self.sys
            .processes()
            .values()
            // sysinfo can list a process's threads as processes on Linux;
            // `top` doesn't. Kernel threads (kworker etc.) are kept.
            .filter(|p| p.thread_kind() != Some(ThreadKind::Userland))
            .map(|p| {
                let name = p.name().to_string_lossy().into_owned();
                let command = if p.cmd().is_empty() {
                    format!("[{name}]")
                } else {
                    p.cmd()
                        .iter()
                        .map(|arg| arg.to_string_lossy())
                        .collect::<Vec<_>>()
                        .join(" ")
                };
                let user = p
                    .user_id()
                    .and_then(|uid| self.users.get_user_by_id(uid))
                    .map_or_else(|| "?".to_string(), |u| u.name().to_string());
                ProcessInfo {
                    pid: p.pid().as_u32(),
                    name,
                    command,
                    user,
                    cpu: p.cpu_usage(),
                    memory: p.memory(),
                    mem_percent: if total_memory == 0 {
                        0.0
                    } else {
                        p.memory() as f64 / total_memory as f64 * 100.0
                    },
                    status: p.status().to_string(),
                    run_time: p.run_time(),
                }
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn proc(pid: u32, name: &str, cpu: f32, memory: u64) -> ProcessInfo {
        ProcessInfo {
            pid,
            name: name.into(),
            command: name.into(),
            user: "me".into(),
            cpu,
            memory,
            mem_percent: 0.0,
            status: "Run".into(),
            run_time: 0,
        }
    }

    fn pids(list: &[ProcessInfo]) -> Vec<u32> {
        list.iter().map(|p| p.pid).collect()
    }

    #[test]
    fn sorts_by_each_key() {
        let mut list = vec![
            proc(3, "bash", 5.0, 100),
            proc(1, "Zsh", 50.0, 10),
            proc(2, "apache", 0.5, 1000),
        ];
        sort(&mut list, SortKey::Cpu, true);
        assert_eq!(pids(&list), [1, 3, 2]);
        sort(&mut list, SortKey::Memory, true);
        assert_eq!(pids(&list), [2, 3, 1]);
        sort(&mut list, SortKey::Pid, false);
        assert_eq!(pids(&list), [1, 2, 3]);
        // Name sort is case-insensitive.
        sort(&mut list, SortKey::Name, false);
        assert_eq!(pids(&list), [2, 3, 1]);
    }

    #[test]
    fn ties_break_on_pid() {
        let mut list = vec![proc(9, "a", 1.0, 1), proc(4, "b", 1.0, 1)];
        sort(&mut list, SortKey::Cpu, false);
        assert_eq!(pids(&list), [4, 9]);
    }

    #[test]
    fn sort_key_cycles_through_all() {
        let mut key = SortKey::Cpu;
        for _ in 0..SortKey::ALL.len() {
            key = key.next();
        }
        assert_eq!(key, SortKey::Cpu);
    }
}

//! System metric collection built on top of `sysinfo`, plus fixed-size
//! history buffers used to draw the charts.

use std::collections::{BTreeMap, VecDeque};
use std::net::IpAddr;
use std::time::Instant;

use crate::disks::{DiskMonitor, DiskStats, Filesystem};

use sysinfo::{
    CpuRefreshKind, InterfaceOperationalState, IpNetwork, MacAddr, MemoryRefreshKind, NetworkData,
    Networks, RefreshKind, System,
};

/// Interfaces excluded from the aggregated network totals.
pub const LOOPBACK: &str = "lo";

/// A ring buffer of the most recent `capacity` samples.
#[derive(Debug, Clone)]
pub struct History {
    samples: VecDeque<f64>,
    capacity: usize,
}

impl History {
    pub fn new(capacity: usize) -> Self {
        Self {
            samples: VecDeque::with_capacity(capacity),
            capacity,
        }
    }

    pub fn push(&mut self, value: f64) {
        if self.samples.len() == self.capacity {
            self.samples.pop_front();
        }
        self.samples.push_back(value);
    }

    pub fn capacity(&self) -> usize {
        self.capacity
    }

    pub fn max(&self) -> f64 {
        self.samples.iter().copied().fold(0.0, f64::max)
    }

    /// Chart points with the newest sample anchored at `x = capacity - 1`,
    /// so a partially filled buffer grows in from the right edge.
    pub fn points(&self) -> Vec<(f64, f64)> {
        let offset = self.capacity - self.samples.len();
        self.samples
            .iter()
            .enumerate()
            .map(|(i, &v)| ((offset + i) as f64, v))
            .collect()
    }
}

#[derive(Debug, Clone)]
pub struct NetHistory {
    pub rx: History,
    pub tx: History,
}

impl NetHistory {
    fn new(capacity: usize) -> Self {
        Self {
            rx: History::new(capacity),
            tx: History::new(capacity),
        }
    }
}

#[derive(Debug, Clone)]
pub struct DiskHistory {
    pub read: History,
    pub write: History,
}

impl DiskHistory {
    fn new(capacity: usize) -> Self {
        Self {
            read: History::new(capacity),
            write: History::new(capacity),
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct CpuStats {
    pub brand: String,
    pub total: f32,
    pub per_core: Vec<f32>,
    /// Average frequency across cores in MHz.
    pub frequency: u64,
}

#[derive(Debug, Clone, Default)]
pub struct MemStats {
    pub used: u64,
    pub total: u64,
    pub swap_used: u64,
    pub swap_total: u64,
}

impl MemStats {
    pub fn ram_percent(&self) -> f64 {
        percent(self.used, self.total)
    }

    pub fn swap_percent(&self) -> f64 {
        percent(self.swap_used, self.swap_total)
    }
}

#[derive(Debug, Clone, Default)]
pub struct InterfaceStats {
    pub name: String,
    pub rx_rate: f64,
    pub tx_rate: f64,
    pub total_rx: u64,
    pub total_tx: u64,
    /// Packets per second.
    pub rx_packet_rate: f64,
    pub tx_packet_rate: f64,
    pub total_rx_errors: u64,
    pub total_tx_errors: u64,
    /// Assigned addresses, IPv4 first. Empty for the aggregate.
    pub addresses: Vec<IpNetwork>,
    /// `None` for the aggregate and interfaces without a hardware address.
    pub mac: Option<MacAddr>,
    pub mtu: u64,
    /// `None` for the aggregate.
    pub state: Option<InterfaceOperationalState>,
}

impl InterfaceStats {
    fn from_data(name: &str, data: &NetworkData, elapsed_secs: f64) -> Self {
        let mut addresses = data.ip_networks().to_vec();
        addresses.sort_by_key(|n| (n.addr.is_ipv6(), n.addr, n.prefix));
        let mac = data.mac_address();
        Self {
            name: name.to_owned(),
            rx_rate: data.received() as f64 / elapsed_secs,
            tx_rate: data.transmitted() as f64 / elapsed_secs,
            total_rx: data.total_received(),
            total_tx: data.total_transmitted(),
            rx_packet_rate: data.packets_received() as f64 / elapsed_secs,
            tx_packet_rate: data.packets_transmitted() as f64 / elapsed_secs,
            total_rx_errors: data.total_errors_on_received(),
            total_tx_errors: data.total_errors_on_transmitted(),
            addresses,
            mac: (!mac.is_unspecified()).then_some(mac),
            mtu: data.mtu(),
            state: Some(data.operational_state()),
        }
    }

    /// The interface's main IPv4 or IPv6 address, preferring routable
    /// addresses over link-local ones.
    pub fn primary_address(&self, ipv6: bool) -> Option<IpAddr> {
        let is_link_local = |addr: &IpAddr| match addr {
            IpAddr::V4(v4) => v4.is_link_local(),
            IpAddr::V6(v6) => v6.is_unicast_link_local(),
        };
        let candidates = || {
            self.addresses
                .iter()
                .map(|n| n.addr)
                .filter(move |a| a.is_ipv6() == ipv6)
        };
        candidates()
            .find(|a| !is_link_local(a))
            .or_else(|| candidates().next())
    }

    /// Adds another interface's traffic into this aggregate.
    fn accumulate(&mut self, other: &Self) {
        self.rx_rate += other.rx_rate;
        self.tx_rate += other.tx_rate;
        self.total_rx += other.total_rx;
        self.total_tx += other.total_tx;
        self.rx_packet_rate += other.rx_packet_rate;
        self.tx_packet_rate += other.tx_packet_rate;
        self.total_rx_errors += other.total_rx_errors;
        self.total_tx_errors += other.total_tx_errors;
    }
}

/// Collects system metrics and keeps a rolling history of them.
pub struct Metrics {
    sys: System,
    networks: Networks,
    disk_monitor: DiskMonitor,
    last_refresh: Instant,
    history_len: usize,

    pub cpu: CpuStats,
    pub mem: MemStats,
    /// Per-interface stats, sorted by name.
    pub interfaces: Vec<InterfaceStats>,
    /// Sum of all non-loopback interfaces.
    pub net_total: InterfaceStats,
    /// Physical disks, sorted by name.
    pub disks: Vec<DiskStats>,
    /// Sum of all physical disks.
    pub disk_total: DiskStats,
    /// Mounted filesystems on block devices, sorted by mount point.
    pub filesystems: Vec<Filesystem>,

    pub cpu_history: History,
    pub ram_history: History,
    pub swap_history: History,
    pub net_total_history: NetHistory,
    pub net_iface_history: BTreeMap<String, NetHistory>,
    pub disk_total_history: DiskHistory,
    pub disk_history: BTreeMap<String, DiskHistory>,
}

impl Metrics {
    pub fn new(history_len: usize) -> Self {
        let sys = System::new_with_specifics(
            RefreshKind::nothing()
                .with_cpu(cpu_refresh_kind())
                .with_memory(MemoryRefreshKind::everything()),
        );
        let brand = sys
            .cpus()
            .first()
            .map(|c| c.brand().trim().to_string())
            .unwrap_or_default();

        let mut metrics = Self {
            sys,
            networks: Networks::new_with_refreshed_list(),
            disk_monitor: DiskMonitor::new(),
            last_refresh: Instant::now(),
            history_len,
            cpu: CpuStats {
                brand,
                ..Default::default()
            },
            mem: MemStats::default(),
            interfaces: Vec::new(),
            net_total: InterfaceStats {
                name: "all".into(),
                ..Default::default()
            },
            disks: Vec::new(),
            disk_total: DiskStats::default(),
            filesystems: Vec::new(),
            cpu_history: History::new(history_len),
            ram_history: History::new(history_len),
            swap_history: History::new(history_len),
            net_total_history: NetHistory::new(history_len),
            net_iface_history: BTreeMap::new(),
            disk_total_history: DiskHistory::new(history_len),
            disk_history: BTreeMap::new(),
        };
        // CPU usage is computed as a delta between two refreshes, so wait the
        // minimum interval before taking the first real sample.
        std::thread::sleep(sysinfo::MINIMUM_CPU_UPDATE_INTERVAL);
        metrics.refresh();
        metrics
    }

    pub fn refresh(&mut self) {
        let elapsed = self.last_refresh.elapsed().as_secs_f64().max(f64::EPSILON);
        self.last_refresh = Instant::now();

        self.refresh_cpu();
        self.refresh_memory();
        self.refresh_network(elapsed);
        self.refresh_disks(elapsed);
    }

    /// Used and total space across filesystems, counting each device once
    /// so bind mounts aren't double-counted.
    pub fn space_totals(&self) -> (u64, u64) {
        let mut seen = std::collections::HashSet::new();
        self.filesystems
            .iter()
            .filter(|fs| seen.insert(&fs.device))
            .fold((0, 0), |(used, total), fs| {
                (used + fs.used(), total + fs.total)
            })
    }

    fn refresh_cpu(&mut self) {
        self.sys.refresh_cpu_specifics(cpu_refresh_kind());
        let cpus = self.sys.cpus();
        self.cpu.total = self.sys.global_cpu_usage();
        self.cpu.per_core = cpus.iter().map(sysinfo::Cpu::cpu_usage).collect();
        self.cpu.frequency = if cpus.is_empty() {
            0
        } else {
            cpus.iter().map(sysinfo::Cpu::frequency).sum::<u64>() / cpus.len() as u64
        };
        self.cpu_history.push(f64::from(self.cpu.total));
    }

    fn refresh_memory(&mut self) {
        self.sys.refresh_memory();
        self.mem = MemStats {
            used: self.sys.used_memory(),
            total: self.sys.total_memory(),
            swap_used: self.sys.used_swap(),
            swap_total: self.sys.total_swap(),
        };
        self.ram_history.push(self.mem.ram_percent());
        self.swap_history.push(self.mem.swap_percent());
    }

    fn refresh_network(&mut self, elapsed_secs: f64) {
        self.networks.refresh(true);

        let mut total = InterfaceStats {
            name: "all".into(),
            ..Default::default()
        };
        let mut interfaces: Vec<InterfaceStats> = self
            .networks
            .iter()
            .map(|(name, data)| InterfaceStats::from_data(name, data, elapsed_secs))
            .collect();
        interfaces.sort_by(|a, b| a.name.cmp(&b.name));

        for iface in &interfaces {
            if iface.name != LOOPBACK {
                total.accumulate(iface);
            }
            let history = self
                .net_iface_history
                .entry(iface.name.clone())
                .or_insert_with(|| NetHistory::new(self.history_len));
            history.rx.push(iface.rx_rate);
            history.tx.push(iface.tx_rate);
        }
        // Drop history for interfaces that have disappeared.
        self.net_iface_history
            .retain(|name, _| interfaces.iter().any(|i| &i.name == name));

        self.net_total_history.rx.push(total.rx_rate);
        self.net_total_history.tx.push(total.tx_rate);
        self.net_total = total;
        self.interfaces = interfaces;
    }

    fn refresh_disks(&mut self, elapsed_secs: f64) {
        let (disks, filesystems) = self.disk_monitor.refresh(elapsed_secs);

        let mut total = DiskStats::default();
        for disk in &disks {
            total.read_rate += disk.read_rate;
            total.write_rate += disk.write_rate;
            total.read_ops += disk.read_ops;
            total.write_ops += disk.write_ops;
            total.total_read += disk.total_read;
            total.total_written += disk.total_written;
            total.size += disk.size;
            let history = self
                .disk_history
                .entry(disk.name.clone())
                .or_insert_with(|| DiskHistory::new(self.history_len));
            history.read.push(disk.read_rate);
            history.write.push(disk.write_rate);
        }
        self.disk_history
            .retain(|name, _| disks.iter().any(|d| &d.name == name));

        self.disk_total_history.read.push(total.read_rate);
        self.disk_total_history.write.push(total.write_rate);
        self.disk_total = total;
        self.disks = disks;
        self.filesystems = filesystems;
    }
}

fn cpu_refresh_kind() -> CpuRefreshKind {
    CpuRefreshKind::nothing().with_cpu_usage().with_frequency()
}

fn percent(used: u64, total: u64) -> f64 {
    if total == 0 {
        0.0
    } else {
        used as f64 / total as f64 * 100.0
    }
}

#[cfg(test)]
#[allow(clippy::float_cmp)] // values compared are exactly representable
mod tests {
    use super::*;

    #[test]
    fn history_evicts_oldest() {
        let mut h = History::new(3);
        for v in 1..=5 {
            h.push(f64::from(v));
        }
        assert_eq!(h.points(), vec![(0.0, 3.0), (1.0, 4.0), (2.0, 5.0)]);
        assert_eq!(h.max(), 5.0);
    }

    #[test]
    fn partial_history_is_right_aligned() {
        let mut h = History::new(4);
        h.push(7.0);
        assert_eq!(h.points(), vec![(3.0, 7.0)]);
    }

    #[test]
    fn primary_address_skips_link_local() {
        let net = |s: &str| s.parse::<IpNetwork>().unwrap();
        let mut iface = InterfaceStats {
            addresses: vec![
                net("192.168.2.25/24"),
                net("fe80::1/64"),
                net("fd7a::2/128"),
            ],
            ..Default::default()
        };
        assert_eq!(
            iface.primary_address(false),
            Some("192.168.2.25".parse().unwrap())
        );
        assert_eq!(
            iface.primary_address(true),
            Some("fd7a::2".parse().unwrap())
        );
        iface.addresses.truncate(2);
        assert_eq!(
            iface.primary_address(true),
            Some("fe80::1".parse().unwrap())
        );
        iface.addresses.remove(0);
        assert_eq!(iface.primary_address(false), None);
    }

    #[test]
    fn percent_handles_zero_total() {
        assert_eq!(percent(5, 0), 0.0);
        assert_eq!(percent(1, 4), 25.0);
    }
}

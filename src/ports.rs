//! Open TCP/UDP sockets, read straight from procfs — the same source
//! `ss -tulpn` uses. Linux only; other platforms get an error.

use std::collections::HashMap;
use std::fmt;
use std::fs;
use std::io;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Protocol {
    Tcp,
    Udp,
}

impl fmt::Display for Protocol {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Tcp => "tcp",
            Self::Udp => "udp",
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Process {
    pub pid: u32,
    pub name: String,
}

#[derive(Debug, Clone)]
pub struct Socket {
    pub protocol: Protocol,
    pub local: SocketAddr,
    pub remote: SocketAddr,
    /// Raw kernel state code (see `include/net/tcp_states.h`).
    pub state: u8,
    pub uid: u32,
    pub inode: u64,
    pub user: Option<String>,
    /// Owning process; `None` when it belongs to another user and we lack
    /// permission to inspect its file descriptors.
    pub process: Option<Process>,
}

const TCP_ESTABLISHED: u8 = 0x01;
const TCP_CLOSE: u8 = 0x07;
const TCP_LISTEN: u8 = 0x0A;

impl Socket {
    /// Whether this socket is waiting for incoming traffic, i.e. what
    /// `ss -l` would show: listening TCP and unconnected UDP.
    pub fn is_listening(&self) -> bool {
        match self.protocol {
            Protocol::Tcp => self.state == TCP_LISTEN,
            Protocol::Udp => self.state == TCP_CLOSE,
        }
    }

    /// State name as printed by `ss`.
    pub fn state_name(&self) -> &'static str {
        match (self.protocol, self.state) {
            (Protocol::Udp | Protocol::Tcp, TCP_ESTABLISHED) => "ESTAB",
            (Protocol::Udp, TCP_CLOSE) => "UNCONN",
            (_, 0x02) => "SYN-SENT",
            (_, 0x03) => "SYN-RECV",
            (_, 0x04) => "FIN-WAIT-1",
            (_, 0x05) => "FIN-WAIT-2",
            (_, 0x06) => "TIME-WAIT",
            (_, TCP_CLOSE) => "CLOSE",
            (_, 0x08) => "CLOSE-WAIT",
            (_, 0x09) => "LAST-ACK",
            (_, TCP_LISTEN) => "LISTEN",
            (_, 0x0B) => "CLOSING",
            _ => "UNKNOWN",
        }
    }
}

/// Lists all TCP and UDP sockets (IPv4 and IPv6), sorted by local port.
pub fn list() -> io::Result<Vec<Socket>> {
    let mut sockets = Vec::new();
    for (path, protocol) in [
        ("/proc/net/tcp", Protocol::Tcp),
        ("/proc/net/tcp6", Protocol::Tcp),
        ("/proc/net/udp", Protocol::Udp),
        ("/proc/net/udp6", Protocol::Udp),
    ] {
        match fs::read_to_string(path) {
            Ok(contents) => sockets.extend(parse_table(&contents, protocol)),
            // IPv6 may be disabled; that's not an error.
            Err(e) if e.kind() == io::ErrorKind::NotFound && path.ends_with('6') => {}
            Err(e) => return Err(io::Error::new(e.kind(), format!("{path}: {e}"))),
        }
    }

    let owners = socket_owners();
    let users = user_names();
    for socket in &mut sockets {
        socket.process = owners.get(&socket.inode).cloned();
        socket.user = users.get(&socket.uid).cloned();
    }
    sockets.sort_by(|a, b| {
        (a.local.port(), a.protocol, a.local.ip(), a.remote).cmp(&(
            b.local.port(),
            b.protocol,
            b.local.ip(),
            b.remote,
        ))
    });
    Ok(sockets)
}

/// Parses a `/proc/net/{tcp,udp}[6]` table, skipping malformed lines.
fn parse_table(contents: &str, protocol: Protocol) -> Vec<Socket> {
    contents
        .lines()
        .skip(1)
        .filter_map(|line| parse_line(line, protocol))
        .collect()
}

fn parse_line(line: &str, protocol: Protocol) -> Option<Socket> {
    // sl local rem st tx:rx tr:when retrnsmt uid timeout inode ...
    let fields: Vec<&str> = line.split_whitespace().collect();
    if fields.len() < 10 {
        return None;
    }
    Some(Socket {
        protocol,
        local: parse_addr(fields[1])?,
        remote: parse_addr(fields[2])?,
        state: u8::from_str_radix(fields[3], 16).ok()?,
        uid: fields[7].parse().ok()?,
        inode: fields[9].parse().ok()?,
        user: None,
        process: None,
    })
}

/// Parses `ADDR:PORT` where ADDR is 8 (IPv4) or 32 (IPv6) hex digits.
///
/// The kernel prints each 32-bit word of the network-order address as a
/// host-endian integer, so each word's native bytes are the address bytes.
fn parse_addr(s: &str) -> Option<SocketAddr> {
    let (addr, port) = s.split_once(':')?;
    let port = u16::from_str_radix(port, 16).ok()?;
    let word = |i: usize| -> Option<[u8; 4]> {
        let hex = addr.get(i * 8..(i + 1) * 8)?;
        Some(u32::from_str_radix(hex, 16).ok()?.to_ne_bytes())
    };
    let ip = match addr.len() {
        8 => IpAddr::V4(Ipv4Addr::from(word(0)?)),
        32 => {
            let mut bytes = [0u8; 16];
            for i in 0..4 {
                bytes[i * 4..(i + 1) * 4].copy_from_slice(&word(i)?);
            }
            let v6 = Ipv6Addr::from(bytes);
            // Show IPv4-mapped addresses (::ffff:a.b.c.d) as plain IPv4.
            v6.to_ipv4_mapped().map_or(IpAddr::V6(v6), IpAddr::V4)
        }
        _ => return None,
    };
    Some(SocketAddr::new(ip, port))
}

/// Maps socket inodes to the process holding them by scanning
/// `/proc/<pid>/fd`. Processes we can't inspect are silently skipped.
fn socket_owners() -> HashMap<u64, Process> {
    let mut owners = HashMap::new();
    let Ok(procs) = fs::read_dir("/proc") else {
        return owners;
    };
    for entry in procs.flatten() {
        let Some(pid) = entry.file_name().to_str().and_then(|s| s.parse().ok()) else {
            continue;
        };
        let Ok(fds) = fs::read_dir(entry.path().join("fd")) else {
            continue;
        };
        let mut name = None;
        for fd in fds.flatten() {
            let Ok(target) = fs::read_link(fd.path()) else {
                continue;
            };
            let Some(inode) = target
                .to_str()
                .and_then(|t| t.strip_prefix("socket:["))
                .and_then(|t| t.strip_suffix(']'))
                .and_then(|t| t.parse().ok())
            else {
                continue;
            };
            let name = name.get_or_insert_with(|| {
                fs::read_to_string(entry.path().join("comm"))
                    .map(|s| s.trim_end().to_string())
                    .unwrap_or_default()
            });
            owners.entry(inode).or_insert_with(|| Process {
                pid,
                name: name.clone(),
            });
        }
    }
    owners
}

/// uid → user name, from `/etc/passwd`.
fn user_names() -> HashMap<u32, String> {
    fs::read_to_string("/etc/passwd")
        .map(|s| parse_passwd(&s))
        .unwrap_or_default()
}

fn parse_passwd(contents: &str) -> HashMap<u32, String> {
    contents
        .lines()
        .filter_map(|line| {
            let mut fields = line.split(':');
            let name = fields.next()?;
            let uid = fields.nth(1)?.parse().ok()?;
            Some((uid, name.to_string()))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[cfg(target_endian = "little")]
    fn parses_ipv4_addr() {
        assert_eq!(
            parse_addr("0100007F:0CEA"),
            Some("127.0.0.1:3306".parse().unwrap())
        );
        assert_eq!(
            parse_addr("00000000:1F90"),
            Some("0.0.0.0:8080".parse().unwrap())
        );
    }

    #[test]
    #[cfg(target_endian = "little")]
    fn parses_ipv6_addr() {
        assert_eq!(
            parse_addr("00000000000000000000000001000000:0016"),
            Some("[::1]:22".parse().unwrap())
        );
        // ::ffff:192.168.1.10 is shown as plain IPv4.
        assert_eq!(
            parse_addr("0000000000000000FFFF00000A01A8C0:01BB"),
            Some("192.168.1.10:443".parse().unwrap())
        );
    }

    #[test]
    fn rejects_bad_addr() {
        assert_eq!(parse_addr("zz:0016"), None);
        assert_eq!(parse_addr("0100007F"), None);
    }

    #[test]
    #[cfg(target_endian = "little")]
    fn parses_table() {
        let table = "\
  sl  local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode
   0: 00000000:1F90 00000000:0000 0A 00000000:00000000 00:00000000 00000000     0        0 31142 1 0000000000000000 100 0 0 10 0
   1: 0100007F:0277 0100007F:A1B2 01 00000000:00000000 00:00000000 00000000  1000        0 1682139 1 0000000000000000 100 0 0 10 0
   garbage line
";
        let sockets = parse_table(table, Protocol::Tcp);
        assert_eq!(sockets.len(), 2);
        assert!(sockets[0].is_listening());
        assert_eq!(sockets[0].state_name(), "LISTEN");
        assert_eq!(sockets[0].local.port(), 8080);
        assert_eq!(sockets[0].inode, 31142);
        assert!(!sockets[1].is_listening());
        assert_eq!(sockets[1].state_name(), "ESTAB");
        assert_eq!(sockets[1].uid, 1000);
    }

    #[test]
    fn udp_unconnected_counts_as_listening() {
        let line = "544: 00000000:5223 00000000:0000 07 00000000:00000000 00:00000000 00000000  1000        0 1692776 2 0000000000000000 0";
        let socket = parse_line(line, Protocol::Udp).unwrap();
        assert!(socket.is_listening());
        assert_eq!(socket.state_name(), "UNCONN");
    }

    #[test]
    fn parses_passwd() {
        let users = parse_passwd(
            "root:x:0:0:root:/root:/bin/bash\nbad\nme:x:1000:1000::/home/me:/bin/sh\n",
        );
        assert_eq!(users.get(&0).map(String::as_str), Some("root"));
        assert_eq!(users.get(&1000).map(String::as_str), Some("me"));
        assert_eq!(users.len(), 2);
    }
}

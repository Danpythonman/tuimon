//! Streams system messages from the systemd journal by running
//! `journalctl --follow --output=json` in the background.

use std::io::{self, BufRead, BufReader};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::thread;

use chrono::{DateTime, Local};
use serde_json::Value;

/// Syslog priority names, indexed by level (0 = most severe).
pub const PRIORITY_NAMES: [&str; 8] = [
    "emerg", "alert", "crit", "err", "warning", "notice", "info", "debug",
];

/// Priority used when an entry doesn't specify one (journald's default).
const DEFAULT_PRIORITY: u8 = 6;

#[derive(Debug, Clone)]
pub struct LogEntry {
    pub time: DateTime<Local>,
    /// Syslog priority, 0 (emerg) to 7 (debug).
    pub priority: u8,
    /// Program name, e.g. `sshd` or `kernel`.
    pub source: String,
    pub pid: Option<u32>,
    pub message: String,
    /// Whether this came from the kernel ring buffer (what `dmesg` shows).
    pub kernel: bool,
}

impl LogEntry {
    pub fn priority_name(&self) -> &'static str {
        PRIORITY_NAMES
            .get(usize::from(self.priority))
            .copied()
            .unwrap_or("?")
    }
}

/// Parses one line of `journalctl --output=json`.
pub fn parse_entry(line: &str) -> Option<LogEntry> {
    let json: Value = serde_json::from_str(line).ok()?;
    let field = |name: &str| json.get(name).and_then(field_to_string);

    let micros: i64 = field("__REALTIME_TIMESTAMP")?.parse().ok()?;
    let time = DateTime::from_timestamp_micros(micros)?.with_timezone(&Local);
    let transport = field("_TRANSPORT");
    let kernel = transport.as_deref() == Some("kernel");
    let source = field("SYSLOG_IDENTIFIER")
        .or_else(|| field("_COMM"))
        .unwrap_or_else(|| if kernel { "kernel" } else { "?" }.into());

    Some(LogEntry {
        time,
        priority: field("PRIORITY")
            .and_then(|p| p.parse().ok())
            .unwrap_or(DEFAULT_PRIORITY),
        source,
        pid: field("_PID").and_then(|p| p.parse().ok()),
        message: field("MESSAGE")
            .map(|m| m.trim_end().to_string())
            .unwrap_or_default(),
        kernel,
    })
}

/// Journal fields are strings, except that values which aren't valid UTF-8
/// are encoded as arrays of bytes.
fn field_to_string(value: &Value) -> Option<String> {
    match value {
        Value::String(s) => Some(s.clone()),
        Value::Array(bytes) => {
            let bytes: Vec<u8> = bytes
                .iter()
                .filter_map(|b| b.as_u64().and_then(|b| u8::try_from(b).ok()))
                .collect();
            Some(String::from_utf8_lossy(&bytes).into_owned())
        }
        _ => None,
    }
}

enum Message {
    Entry(LogEntry),
    Ended,
}

/// A running `journalctl --follow` process whose entries arrive on a
/// channel. The child process is killed on drop.
pub struct Journal {
    child: Child,
    rx: Receiver<Message>,
    ended: bool,
}

impl Journal {
    /// Starts following the journal, beginning with the last `backlog` entries.
    pub fn spawn(backlog: usize) -> io::Result<Self> {
        let mut child = Command::new("journalctl")
            .args(["--follow", "--output=json", "--no-pager", "--lines"])
            .arg(backlog.to_string())
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| io::Error::new(e.kind(), format!("could not run journalctl: {e}")))?;
        let stdout = child.stdout.take().expect("stdout is piped");

        let (tx, rx) = mpsc::channel();
        thread::Builder::new()
            .name("journal-reader".into())
            .spawn(move || {
                for line in BufReader::new(stdout).lines() {
                    let Ok(line) = line else { break };
                    if let Some(entry) = parse_entry(&line)
                        && tx.send(Message::Entry(entry)).is_err()
                    {
                        return; // UI side hung up.
                    }
                }
                let _ = tx.send(Message::Ended);
            })?;

        Ok(Self {
            child,
            rx,
            ended: false,
        })
    }

    /// Returns all entries received since the last call.
    pub fn drain(&mut self) -> Vec<LogEntry> {
        let mut entries = Vec::new();
        loop {
            match self.rx.try_recv() {
                Ok(Message::Entry(entry)) => entries.push(entry),
                Ok(Message::Ended) | Err(TryRecvError::Disconnected) => {
                    self.ended = true;
                    break;
                }
                Err(TryRecvError::Empty) => break,
            }
        }
        entries
    }

    /// Whether `journalctl` has exited (it shouldn't while following).
    pub fn has_ended(&self) -> bool {
        self.ended
    }
}

impl Drop for Journal {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_regular_entry() {
        let line = r#"{"__REALTIME_TIMESTAMP":"1790486491718453","PRIORITY":"4","SYSLOG_IDENTIFIER":"sshd","_PID":"812","MESSAGE":"hello","_TRANSPORT":"syslog"}"#;
        let entry = parse_entry(line).unwrap();
        assert_eq!(entry.priority, 4);
        assert_eq!(entry.priority_name(), "warning");
        assert_eq!(entry.source, "sshd");
        assert_eq!(entry.pid, Some(812));
        assert_eq!(entry.message, "hello");
        assert!(!entry.kernel);
        assert_eq!(entry.time.timestamp_micros(), 1_790_486_491_718_453);
    }

    #[test]
    fn parses_kernel_entry_without_identifier() {
        let line =
            r#"{"__REALTIME_TIMESTAMP":"1","_TRANSPORT":"kernel","MESSAGE":"usb 1-1: new device"}"#;
        let entry = parse_entry(line).unwrap();
        assert!(entry.kernel);
        assert_eq!(entry.source, "kernel");
        assert_eq!(entry.priority, DEFAULT_PRIORITY);
        assert_eq!(entry.pid, None);
    }

    #[test]
    fn decodes_binary_message() {
        let line = r#"{"__REALTIME_TIMESTAMP":"1","MESSAGE":[104,105,255]}"#;
        assert_eq!(parse_entry(line).unwrap().message, "hi\u{FFFD}");
    }

    #[test]
    fn rejects_invalid_lines() {
        assert!(parse_entry("not json").is_none());
        assert!(parse_entry(r#"{"MESSAGE":"no timestamp"}"#).is_none());
    }
}

//! Human-readable formatting helpers for byte counts, rates and durations.

const UNITS: [&str; 6] = ["B", "KiB", "MiB", "GiB", "TiB", "PiB"];

/// Formats a byte count using binary (IEC) units, e.g. `1.50 GiB`.
pub fn bytes(value: u64) -> String {
    scaled(value as f64)
}

/// Formats a byte-per-second rate, e.g. `12.3 MiB/s`.
pub fn rate(bytes_per_sec: f64) -> String {
    format!("{}/s", scaled(bytes_per_sec))
}

fn scaled(mut value: f64) -> String {
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 || value >= 100.0 {
        format!("{value:.0} {}", UNITS[unit])
    } else if value >= 10.0 {
        format!("{value:.1} {}", UNITS[unit])
    } else {
        format!("{value:.2} {}", UNITS[unit])
    }
}

/// Formats a number of seconds as a compact uptime string, e.g. `3d 04h 12m`.
pub fn duration(secs: u64) -> String {
    let days = secs / 86_400;
    let hours = (secs % 86_400) / 3_600;
    let minutes = (secs % 3_600) / 60;
    if days > 0 {
        format!("{days}d {hours:02}h {minutes:02}m")
    } else if hours > 0 {
        format!("{hours}h {minutes:02}m")
    } else {
        format!("{minutes}m {:02}s", secs % 60)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_bytes() {
        assert_eq!(bytes(0), "0 B");
        assert_eq!(bytes(1023), "1023 B");
        assert_eq!(bytes(1024), "1.00 KiB");
        assert_eq!(bytes(1536), "1.50 KiB");
        assert_eq!(bytes(15 * 1024 * 1024), "15.0 MiB");
        assert_eq!(bytes(512 * 1024 * 1024 * 1024), "512 GiB");
    }

    #[test]
    fn formats_rate() {
        assert_eq!(rate(0.0), "0 B/s");
        assert_eq!(rate(2.5 * 1024.0 * 1024.0), "2.50 MiB/s");
    }

    #[test]
    fn formats_duration() {
        assert_eq!(duration(59), "0m 59s");
        assert_eq!(duration(3_661), "1h 01m");
        assert_eq!(duration(2 * 86_400 + 5 * 3_600 + 7 * 60), "2d 05h 07m");
    }
}
